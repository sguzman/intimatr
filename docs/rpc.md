# RPC protocol

Intimatr exposes the same shared command dispatcher used by the in-process UIs through a local RPC transport. RPC does not bypass policy: memory read/write, code patch, debugger, transfer-size, result-page, advanced-analysis, and remote-shutdown rules are enforced by the shared command layer before transport-specific code sees a result.

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

The command schema includes shared memory operations, scalar scan sessions/results/cancellation, watch management, module/thread queries, debugger operations, advanced analysis, and lifecycle operations. Watch definitions include an optional `frozen` scalar; enabling a freeze remains subject to `policy.allow_memory_write`.

## Advanced analysis commands

Advanced analysis is one ordinary serialized command:

```json
{
  "version": 1,
  "request_id": 42,
  "command": "analysis",
  "request": {
    "analysis": "aob_scan",
    "pattern": "48 8B ?? ?F",
    "alignment": 1,
    "max_results": 1024
  }
}
```

The outer `CommandResult` keeps the protocol's existing `"result"` tag, so an analysis response has `"result":"analysis"` and its nested payload in the separate `"analysis"` field. The nested `AnalysisCommand`/`AnalysisResult` types use their own `"analysis"`/`"analysis_result"` tags.

The current analysis request surface includes:

- `aob_scan`
- `resolve_address`
- `resolve_pointer_chain`
- `search_pointer_chains`
- `inspect_structure`
- `save_scan` / `restore_scan`
- `save_watch_template` / `add_watch_from_template`
- `list_saved`
- `save_workspace` / `load_workspace`
- `batch`

AOB, pointer, and structure reads require the shared memory-read policy. Instantiating a frozen watch template requires the shared memory-write policy. Result counts, pointer-search breadth, structure byte reads, and batch length are bounded before unbounded work is accepted.

`batch` accepts up to 128 non-batch analysis commands and executes them sequentially through exactly the same dispatcher implementation as individual calls. Nested batches are rejected. This is the first-party automation primitive rather than a second scripting server or alternate memory-access path.

Workspace state is per target executable and serialized by the DLL beneath `analysis/<ExecutableName>`. Saved watch templates prefer module-relative addresses when possible. Saved scalar scans retain their candidate snapshots and therefore may need revalidation after a process restart or layout change.

See `docs/analysis.md` for AOB wildcard syntax, address-expression grammar, pointer semantics, structure fields, and persistence details.

## Debugger commands

Protocol v1 debugger commands use the same serialized `Command`/`CommandResult` types as the native debugger UI. The current debugger surface includes `list_threads`, `read_thread_registers`, `disassemble`, status/control commands, hardware-breakpoint commands, and `debugger_events`.

All debugger commands require `policy.allow_debugger`. `disassemble` additionally requires memory-read permission and remains bounded by both the shared memory-transfer limit and debugger disassembly limits.

Debugger events use a cursor rather than transport-specific server push. A client remembers the largest sequence it has consumed and asks for later events:

```json
{
  "version": 1,
  "request_id": 43,
  "command": "debugger_events",
  "after_sequence": 120,
  "limit": 64
}
```

The response contains an ordered `events` array plus `latest_sequence`. Polling again with the last consumed sequence incrementally consumes the same bounded event feed used by the native debugger UI. A slow client may miss events that have already rolled out of the bounded in-process ring, so consumers should not treat the feed as durable storage.

Hardware-breakpoint and single-step events are trace-style notifications. The scoped Windows exception handler records Intimatr-owned events and resumes execution; RPC does not imply a global external-debugger stop state.

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

## Concurrency and shared state

Connection I/O runs on a dedicated Tokio runtime owned by an Intimatr RPC thread. Command execution runs through blocking worker tasks so long scalar scans, advanced analysis, or debugger operations do not block the I/O reactor. Scan cancellation tokens live in the shared dispatcher, allowing another connected frontend to request cancellation while a scalar scan is active.

Watch definitions, scalar scan sessions, analysis workspaces, debugger state, breakpoints, and debugger events live behind the same dispatcher used by native frontends. RPC therefore observes the same IDs and state rather than maintaining transport-specific copies.
