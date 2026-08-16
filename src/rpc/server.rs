use std::{
    io,
    net::SocketAddr,
    sync::{Arc, mpsc},
    thread,
};

use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    runtime::Builder,
    sync::{Semaphore, watch},
    task::JoinSet,
};
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::{
    command::{CommandExecution, CommandExecutor, PostAction},
    config::{RpcConfig, RpcTransport},
};

use super::protocol::{
    PROTOCOL_VERSION, ProtocolError, RpcRequest, RpcResponse, read_frame, write_frame,
};

pub type PostActionHandler = Arc<dyn Fn(PostAction) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcEndpoint {
    Tcp(SocketAddr),
    NamedPipe(String),
}

pub struct RpcServerHandle {
    endpoint: RpcEndpoint,
    shutdown: watch::Sender<bool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl RpcServerHandle {
    pub fn endpoint(&self) -> &RpcEndpoint {
        &self.endpoint
    }

    pub fn stop(&mut self) -> Result<(), RpcServerError> {
        let _ = self.shutdown.send(true);
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| RpcServerError::ThreadPanicked)?;
        }
        Ok(())
    }
}

impl Drop for RpcServerHandle {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            error!(error = %error, "failed to stop RPC server cleanly");
        }
    }
}

pub fn start_server(
    config: RpcConfig,
    executor: Arc<dyn CommandExecutor>,
    post_action_handler: PostActionHandler,
) -> Result<RpcServerHandle, RpcServerError> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (startup_tx, startup_rx) = mpsc::sync_channel(1);
    let thread_config = config.clone();

    let thread = thread::Builder::new()
        .name("intimatr-rpc".to_owned())
        .spawn(move || {
            let runtime = match Builder::new_current_thread().enable_io().build() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = startup_tx.send(Err(error.to_string()));
                    return;
                }
            };

            let result = runtime.block_on(run_server(
                thread_config,
                executor,
                post_action_handler,
                shutdown_rx,
                startup_tx,
            ));
            if let Err(error) = result {
                error!(error = %error, "RPC server terminated with an error");
            }
        })?;

    let endpoint = startup_rx
        .recv()
        .map_err(|_| RpcServerError::StartupChannelClosed)?
        .map_err(RpcServerError::Startup)?;

    Ok(RpcServerHandle {
        endpoint,
        shutdown: shutdown_tx,
        thread: Some(thread),
    })
}

async fn run_server(
    config: RpcConfig,
    executor: Arc<dyn CommandExecutor>,
    post_action_handler: PostActionHandler,
    shutdown: watch::Receiver<bool>,
    startup: mpsc::SyncSender<Result<RpcEndpoint, String>>,
) -> Result<(), RpcServerError> {
    match config.transport {
        RpcTransport::Tcp => {
            run_tcp(config, executor, post_action_handler, shutdown, startup).await
        }
        RpcTransport::NamedPipe => {
            run_named_pipe(config, executor, post_action_handler, shutdown, startup).await
        }
    }
}

async fn run_tcp(
    config: RpcConfig,
    executor: Arc<dyn CommandExecutor>,
    post_action_handler: PostActionHandler,
    mut shutdown: watch::Receiver<bool>,
    startup: mpsc::SyncSender<Result<RpcEndpoint, String>>,
) -> Result<(), RpcServerError> {
    let listener = match TcpListener::bind(&config.bind).await {
        Ok(listener) => listener,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error.into());
        }
    };
    let endpoint = listener.local_addr()?;
    if !endpoint.ip().is_loopback() {
        let message = format!("refusing non-loopback RPC endpoint {endpoint}");
        let _ = startup.send(Err(message.clone()));
        return Err(RpcServerError::NonLoopback(endpoint));
    }
    let _ = startup.send(Ok(RpcEndpoint::Tcp(endpoint)));
    info!(%endpoint, max_clients = config.max_clients, "TCP RPC server is listening");

    let permits = Arc::new(Semaphore::new(config.max_clients));
    let mut tasks = JoinSet::new();

    loop {
        reap_finished(&mut tasks);
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                if !peer.ip().is_loopback() {
                    warn!(%peer, "rejected non-loopback RPC client");
                    continue;
                }
                let permit = match Arc::clone(&permits).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        warn!(%peer, "rejected RPC client because max_clients is reached");
                        continue;
                    }
                };
                stream.set_nodelay(true)?;
                let executor = Arc::clone(&executor);
                let handler = Arc::clone(&post_action_handler);
                let connection_shutdown = shutdown.clone();
                let limits = ConnectionLimits::from(&config);
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_connection(
                        stream,
                        executor,
                        handler,
                        connection_shutdown,
                        limits,
                    ).await {
                        debug!(%peer, error = %error, "RPC client connection ended with an error");
                    }
                });
            }
        }
    }

    shutdown_connections(&mut tasks).await;
    info!(%endpoint, "TCP RPC server stopped");
    Ok(())
}

#[cfg(windows)]
async fn run_named_pipe(
    config: RpcConfig,
    executor: Arc<dyn CommandExecutor>,
    post_action_handler: PostActionHandler,
    mut shutdown: watch::Receiver<bool>,
    startup: mpsc::SyncSender<Result<RpcEndpoint, String>>,
) -> Result<(), RpcServerError> {
    let pipe_name = normalize_pipe_name(&config.pipe_name);
    let mut server = match make_pipe_server(&config, &pipe_name, true) {
        Ok(server) => server,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error.into());
        }
    };
    let _ = startup.send(Ok(RpcEndpoint::NamedPipe(pipe_name.clone())));
    info!(pipe = %pipe_name, max_clients = config.max_clients, "named-pipe RPC server is listening");

    let permits = Arc::new(Semaphore::new(config.max_clients));
    let mut tasks = JoinSet::new();
    loop {
        reap_finished(&mut tasks);
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            connected = server.connect() => {
                connected?;
                let connected_client = server;
                server = make_pipe_server(&config, &pipe_name, false)?;
                let permit = match Arc::clone(&permits).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        warn!(pipe = %pipe_name, "closed named-pipe client because max_clients is reached");
                        drop(connected_client);
                        continue;
                    }
                };
                let executor = Arc::clone(&executor);
                let handler = Arc::clone(&post_action_handler);
                let connection_shutdown = shutdown.clone();
                let limits = ConnectionLimits::from(&config);
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_connection(
                        connected_client,
                        executor,
                        handler,
                        connection_shutdown,
                        limits,
                    ).await {
                        debug!(error = %error, "named-pipe RPC connection ended with an error");
                    }
                });
            }
        }
    }

    shutdown_connections(&mut tasks).await;
    info!(pipe = %pipe_name, "named-pipe RPC server stopped");
    Ok(())
}

#[cfg(windows)]
fn make_pipe_server(
    config: &RpcConfig,
    pipe_name: &str,
    first: bool,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .max_instances(config.max_clients)
        .in_buffer_size(config.max_request_bytes.min(u32::MAX as usize) as u32)
        .out_buffer_size(config.max_response_bytes.min(u32::MAX as usize) as u32)
        .reject_remote_clients(true);
    options.create(pipe_name)
}

#[cfg(not(windows))]
async fn run_named_pipe(
    _config: RpcConfig,
    _executor: Arc<dyn CommandExecutor>,
    _post_action_handler: PostActionHandler,
    _shutdown: watch::Receiver<bool>,
    startup: mpsc::SyncSender<Result<RpcEndpoint, String>>,
) -> Result<(), RpcServerError> {
    let message = "Windows named-pipe RPC transport is unavailable on this platform".to_owned();
    let _ = startup.send(Err(message));
    Err(RpcServerError::UnsupportedTransport)
}

async fn serve_connection<S>(
    mut stream: S,
    executor: Arc<dyn CommandExecutor>,
    post_action_handler: PostActionHandler,
    mut shutdown: watch::Receiver<bool>,
    limits: ConnectionLimits,
) -> Result<(), RpcServerError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    loop {
        let request: RpcRequest = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
                continue;
            }
            request = read_frame(&mut stream, limits.max_request_bytes) => {
                match request? {
                    Some(request) => request,
                    None => return Ok(()),
                }
            }
        };

        let request_id = request.request_id;
        let command_name = request.command.name();
        let span = info_span!("rpc_request", request_id, command = command_name);
        let (response, post_action) = handle_request(request, Arc::clone(&executor))
            .instrument(span)
            .await;
        write_frame(&mut stream, &response, limits.max_response_bytes).await?;
        if let Some(post_action) = post_action {
            post_action_handler(post_action);
        }
    }
}

async fn handle_request(
    request: RpcRequest,
    executor: Arc<dyn CommandExecutor>,
) -> (RpcResponse, Option<PostAction>) {
    if request.version != PROTOCOL_VERSION {
        return (
            RpcResponse::error(
                request.request_id,
                "version_mismatch",
                format!(
                    "protocol version {} is unsupported; server uses {}",
                    request.version, PROTOCOL_VERSION
                ),
            ),
            None,
        );
    }

    info!("executing RPC command");
    let request_id = request.request_id;
    match tokio::task::spawn_blocking(move || executor.execute(request.command)).await {
        Ok(Ok(CommandExecution {
            result,
            post_action,
        })) => (RpcResponse::success(request_id, result), post_action),
        Ok(Err(error)) => (
            RpcResponse::error(request_id, error.code(), error.to_string()),
            None,
        ),
        Err(error) => (
            RpcResponse::error(request_id, "executor_join_error", error.to_string()),
            None,
        ),
    }
}

async fn shutdown_connections(tasks: &mut JoinSet<()>) {
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            warn!(error = %error, "RPC client task failed during shutdown");
        }
    }
}

fn reap_finished(tasks: &mut JoinSet<()>) {
    while let Some(result) = tasks.try_join_next() {
        if let Err(error) = result {
            warn!(error = %error, "RPC client task failed");
        }
    }
}

#[cfg(windows)]
fn normalize_pipe_name(name: &str) -> String {
    if name.starts_with(r"\\.\pipe\") {
        name.to_owned()
    } else {
        format!(r"\\.\pipe\{name}")
    }
}

#[derive(Debug, Clone, Copy)]
struct ConnectionLimits {
    max_request_bytes: usize,
    max_response_bytes: usize,
}

impl From<&RpcConfig> for ConnectionLimits {
    fn from(config: &RpcConfig) -> Self {
        Self {
            max_request_bytes: config.max_request_bytes,
            max_response_bytes: config.max_response_bytes,
        }
    }
}

#[derive(Debug, Error)]
pub enum RpcServerError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("RPC server startup failed: {0}")]
    Startup(String),
    #[error("RPC startup channel closed before an endpoint was reported")]
    StartupChannelClosed,
    #[error("RPC server thread panicked")]
    ThreadPanicked,
    #[error("refusing non-loopback TCP endpoint {0}")]
    NonLoopback(SocketAddr),
    #[error("requested RPC transport is not supported on this platform")]
    UnsupportedTransport,
}
