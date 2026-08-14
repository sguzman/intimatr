use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use intimatr::{
    command::{Command, CommandError, CommandExecution, CommandExecutor, CommandResult, PostAction},
    config::{RpcConfig, RpcTransport},
    rpc::{PostActionHandler, RpcClient, RpcEndpoint, start_server},
};

struct TestExecutor;

impl CommandExecutor for TestExecutor {
    fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
        match command {
            Command::Ping => Ok(CommandExecution {
                result: CommandResult::Pong,
                post_action: None,
            }),
            Command::Shutdown => Ok(CommandExecution {
                result: CommandResult::ShutdownAccepted,
                post_action: Some(PostAction::Shutdown),
            }),
            _ => Err(CommandError::NotImplemented("test command")),
        }
    }
}

fn post_action_recorder(flag: Arc<AtomicBool>) -> PostActionHandler {
    Arc::new(move |action| match action {
        PostAction::Shutdown => flag.store(true, Ordering::Release),
    })
}

#[test]
fn loopback_tcp_client_and_server_round_trip_commands() {
    let mut config = RpcConfig::default();
    config.transport = RpcTransport::Tcp;
    config.bind = "127.0.0.1:0".to_owned();
    let shutdown_seen = Arc::new(AtomicBool::new(false));
    let mut server = start_server(
        config.clone(),
        Arc::new(TestExecutor),
        post_action_recorder(Arc::clone(&shutdown_seen)),
    )
    .expect("TCP RPC server should start");

    let address = match server.endpoint() {
        RpcEndpoint::Tcp(address) => *address,
        other => panic!("unexpected endpoint: {other:?}"),
    };
    let mut client = RpcClient::connect_tcp(
        address,
        config.max_request_bytes,
        config.max_response_bytes,
    )
    .expect("TCP client should connect");

    assert_eq!(client.call(Command::Ping).unwrap(), CommandResult::Pong);
    assert_eq!(
        client.call(Command::Shutdown).unwrap(),
        CommandResult::ShutdownAccepted
    );

    for _ in 0..100 {
        if shutdown_seen.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(shutdown_seen.load(Ordering::Acquire));
    server.stop().expect("server should stop cleanly");
}

#[cfg(windows)]
#[test]
fn windows_named_pipe_client_and_server_round_trip() {
    let mut config = RpcConfig::default();
    config.transport = RpcTransport::NamedPipe;
    config.pipe_name = format!(
        "intimatr-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let shutdown_seen = Arc::new(AtomicBool::new(false));
    let mut server = start_server(
        config.clone(),
        Arc::new(TestExecutor),
        post_action_recorder(shutdown_seen),
    )
    .expect("named-pipe RPC server should start");

    let pipe = match server.endpoint() {
        RpcEndpoint::NamedPipe(pipe) => pipe.clone(),
        other => panic!("unexpected endpoint: {other:?}"),
    };
    let mut client = RpcClient::connect_named_pipe(
        &pipe,
        config.max_request_bytes,
        config.max_response_bytes,
    )
    .expect("named-pipe client should connect");

    assert_eq!(client.call(Command::Ping).unwrap(), CommandResult::Pong);
    server.stop().expect("server should stop cleanly");
}
