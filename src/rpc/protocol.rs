use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::command::{Command, CommandResult};

pub const PROTOCOL_VERSION: u16 = 1;
const FRAME_HEADER_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub version: u16,
    pub request_id: u64,
    pub command: Command,
}

impl RpcRequest {
    pub fn new(request_id: u64, command: Command) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub version: u16,
    pub request_id: u64,
    pub outcome: RpcOutcome,
}

impl RpcResponse {
    pub fn success(request_id: u64, result: CommandResult) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            outcome: RpcOutcome::Success { result },
        }
    }

    pub fn error(request_id: u64, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            outcome: RpcOutcome::Error {
                error: RpcErrorPayload {
                    code: code.into(),
                    message: message.into(),
                },
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RpcOutcome {
    Success { result: CommandResult },
    Error { error: RpcErrorPayload },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcErrorPayload {
    pub code: String,
    pub message: String,
}

pub async fn read_frame<R, T>(reader: &mut R, max_bytes: usize) -> Result<Option<T>, ProtocolError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }

    let length = u32::from_be_bytes(header) as usize;
    validate_frame_length(length, max_bytes)?;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(Some(serde_json::from_slice(&payload)?))
}

pub async fn write_frame<W, T>(
    writer: &mut W,
    value: &T,
    max_bytes: usize,
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = encode_payload(value, max_bytes)?;
    writer.write_all(&(payload.len() as u32).to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

pub fn read_frame_blocking<R, T>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<T>, ProtocolError>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }

    let length = u32::from_be_bytes(header) as usize;
    validate_frame_length(length, max_bytes)?;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Some(serde_json::from_slice(&payload)?))
}

pub fn write_frame_blocking<W, T>(
    writer: &mut W,
    value: &T,
    max_bytes: usize,
) -> Result<(), ProtocolError>
where
    W: Write,
    T: Serialize,
{
    let payload = encode_payload(value, max_bytes)?;
    writer.write_all(&(payload.len() as u32).to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

fn encode_payload<T: Serialize>(value: &T, max_bytes: usize) -> Result<Vec<u8>, ProtocolError> {
    let payload = serde_json::to_vec(value)?;
    validate_frame_length(payload.len(), max_bytes)?;
    if payload.len() > u32::MAX as usize {
        return Err(ProtocolError::FrameTooLarge {
            length: payload.len(),
            limit: u32::MAX as usize,
        });
    }
    Ok(payload)
}

fn validate_frame_length(length: usize, max_bytes: usize) -> Result<(), ProtocolError> {
    if length == 0 {
        return Err(ProtocolError::EmptyFrame);
    }
    if length > max_bytes {
        return Err(ProtocolError::FrameTooLarge {
            length,
            limit: max_bytes,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("RPC frame is empty")]
    EmptyFrame,
    #[error("RPC frame is {length} bytes, exceeding the {limit}-byte limit")]
    FrameTooLarge { length: usize, limit: usize },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::command::Command;

    use super::*;

    #[test]
    fn blocking_frame_round_trip_preserves_request() {
        let request = RpcRequest::new(17, Command::Ping);
        let mut bytes = Vec::new();
        write_frame_blocking(&mut bytes, &request, 1024).unwrap();

        let decoded: RpcRequest = read_frame_blocking(&mut Cursor::new(bytes), 1024)
            .unwrap()
            .expect("frame should be present");
        assert_eq!(decoded, request);
    }

    #[test]
    fn oversized_frame_is_rejected_before_write() {
        let request = RpcRequest::new(1, Command::Ping);
        let error = write_frame_blocking(&mut Vec::new(), &request, 1)
            .expect_err("tiny frame limit should reject request");

        assert!(matches!(error, ProtocolError::FrameTooLarge { .. }));
    }
}
