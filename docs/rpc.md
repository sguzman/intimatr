# RPC protocol

Intimatr exposes the same shared command dispatcher used by the eventual in-process UI through a local RPC transport. RPC does not bypass policy: memory read/write, code patch, debugger, transfer-size, result-page, and remote-shutdown rules are enforced by the shared command layer before transport-specific code sees a result.

## Protocol version 1

Each message is a JSON document preceded by a four-byte unsigned big-endian payload length. Empty frames and frames larger than the configured limit are rejected before JSON deserialization.

A request contains:

```json
{
  "version": 1,
  "request_id": 1,
  "command": "ping"
}
```

A successful response contains the same `request_id`:

```json
{
  "version": 1,
  "request_id": 1,
  "outcome": {
    "status": "success",
    "result": {
      "result": "pong"
    }
  }
}
```

Errors use a stable string code and human-readable message. Unsupported protocol versions receive `version_mismatch` rather than being silently interpreted.

## TCP

TCP is restricted to IP loopback addresses. A configuration such as `0.0.0.0:31337` is rejected. The default is:

```toml
[rpc]
enabled = true
transport = "tcp"
bind = "127.0.0.1:31337"
max_clients = 4
max_request_bytes = 1048576
max_response_bytes = 4194304
max_memory_transfer_bytes = 262144
max_scan_results_per_page = 4096
```

## Windows named pipe

Set `transport = "named_pipe"` and choose `pipe_name`. Bare names are normalized beneath `\\.\pipe\`. The server explicitly rejects remote clients and limits the number of simultaneous pipe instances using `max_clients`.

```toml
[rpc]
enabled = true
transport = "named_pipe"
pipe_name = "intimatr"
max_clients = 4
```

## First-party client

The crate includes a small blocking Rust TCP client and an example frontend:

```powershell
cargo run --example rpc_client -- 127.0.0.1:31337
```

The client handles framing, protocol version checks, request-ID matching, and remote error conversion. On Windows it can also connect to the named-pipe transport through `RpcClient::connect_named_pipe`.

## Concurrency and scans

Connection I/O runs on a dedicated Tokio runtime owned by an Intimatr RPC thread. Command execution runs through blocking worker tasks so long memory scans do not block the I/O reactor. Scan cancellation tokens live in the shared dispatcher, allowing another connected frontend to request cancellation while a scan is active.
