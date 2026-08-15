use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::atomic::{AtomicU64, Ordering},
};

use thiserror::Error;

use crate::command::{Command, CommandResult};

use super::protocol::{
    PROTOCOL_VERSION, ProtocolError, RpcOutcome, RpcRequest, RpcResponse, read_frame_blocking,
    write_frame_blocking,
};

trait ClientStream: Read + Write + Send {}
impl<T: Read + Write + Send> ClientStream for T {}

pub struct RpcClient {
    stream: Box<dyn ClientStream>,
    max_request_bytes: usize,
    max_response_bytes: usize,
    next_request_id: AtomicU64,
}

impl RpcClient {
    pub fn connect_tcp(
        address: SocketAddr,
        max_request_bytes: usize,
        max_response_bytes: usize,
    ) -> Result<Self, RpcClientError> {
        let stream = TcpStream::connect(address)?;
        stream.set_nodelay(true)?;
        Ok(Self::from_stream(
            stream,
            max_request_bytes,
            max_response_bytes,
        ))
    }

    #[cfg(windows)]
    pub fn connect_named_pipe(
        pipe_name: &str,
        max_request_bytes: usize,
        max_response_bytes: usize,
    ) -> Result<Self, RpcClientError> {
        use std::fs::OpenOptions;

        let pipe_name = if pipe_name.starts_with(r"\\.\pipe\") {
            pipe_name.to_owned()
        } else {
            format!(r"\\.\pipe\{pipe_name}")
        };
        let stream = OpenOptions::new().read(true).write(true).open(pipe_name)?;
        Ok(Self::from_stream(
            stream,
            max_request_bytes,
            max_response_bytes,
        ))
    }

    pub fn call(&mut self, command: Command) -> Result<CommandResult, RpcClientError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let request = RpcRequest::new(request_id, command);
        write_frame_blocking(&mut self.stream, &request, self.max_request_bytes)?;

        let response: RpcResponse = read_frame_blocking(&mut self.stream, self.max_response_bytes)?
            .ok_or(RpcClientError::ConnectionClosed)?;
        if response.version != PROTOCOL_VERSION {
            return Err(RpcClientError::VersionMismatch {
                expected: PROTOCOL_VERSION,
                actual: response.version,
            });
        }
        if response.request_id != request_id {
            return Err(RpcClientError::RequestIdMismatch {
                expected: request_id,
                actual: response.request_id,
            });
        }

        match response.outcome {
            RpcOutcome::Success { result } => Ok(result),
            RpcOutcome::Error { error } => Err(RpcClientError::Remote {
                code: error.code,
                message: error.message,
            }),
        }
    }

    fn from_stream<S>(stream: S, max_request_bytes: usize, max_response_bytes: usize) -> Self
    where
        S: ClientStream + 'static,
    {
        Self {
            stream: Box::new(stream),
            max_request_bytes,
            max_response_bytes,
            next_request_id: AtomicU64::new(1),
        }
    }
}

#[derive(Debug, Error)]
pub enum RpcClientError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("RPC server closed the connection")]
    ConnectionClosed,
    #[error("RPC protocol version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u16, actual: u16 },
    #[error("RPC request ID mismatch: expected {expected}, got {actual}")]
    RequestIdMismatch { expected: u64, actual: u64 },
    #[error("remote RPC error {code}: {message}")]
    Remote { code: String, message: String },
}
