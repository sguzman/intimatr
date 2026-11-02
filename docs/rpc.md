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

## Speed preview command client (Windows/Wine)

The release now bundles `intimatr-speed.exe` alongside the DLL. It uses the existing version-1 RPC framing and **only** connects to an explicit local endpoint; it never identifies or injects a game process. Example commands for an Intimatr instance with RPC enabled:

```powershell
./intimatr-speed.exe --tcp 127.0.0.1:31337 status
./intimatr-speed.exe --tcp 127.0.0.1:31337 increase
./intimatr-speed.exe --tcp 127.0.0.1:31337 preset 2.0
./intimatr-speed.exe --pipe intimatr reset
```

Use the endpoint actually configured by the target's `config/<Executable>.toml`; TCP is limited to numeric loopback and the pipe option to a simple local pipe name. The executable prints the exact `SpeedPreviewStatus` JSON, including `configured`, `preview_multiplier`, `focused`, `last_action` and `clock_applied_to_game`. With no explicitly enabled shared speed profile, `status` returns `configured=false` and speed mutations fail closed. In the current general-purpose DLL, `clock_applied_to_game=false` is expected: **changing preview speed does not accelerate a real game**. Real hooking remains restricted to Intimatr's opt-in disposable Wine fixtures. This command client must not be described as a production game speed controller until that limitation is lifted through verified offline-game QA.

The Windows MSVC release archive checks SHA-256 hashes for **both** `intimatr.dll` and `intimatr-speed.exe`. Linux GNU cross-build CI uploads a separate Windows PE `intimatr-speed.exe` artifact for Wine; the Linux-native Bottles inventory CLI is a different executable.

### Isolated Wine RPC client acceptance check

The disposable `wine-load-probe.exe` test fixture starts a **local-only TCP server** at `127.0.0.1:31379`, with all existing UI/debugger surfaces disabled. Before attempting experimental timer IAT hooks, the fixture runs the genuine packaged `intimatr-speed.exe` under the same Wine prefix. It checks an enabled preview `status` (1×, `clock_applied_to_game=false`), applies `increase` (1.5× preview), resets back to 1×, and ensures a non-loopback endpoint is rejected. This is independent of the separate four-API timer IAT proof; neither test targets an actual game executable. The fixed port belongs only to this disposable CI fixture, not to users' default RPC configuration.

### Timed speed holds through the explicit Linux bridge

The `intimatr-speed.exe` Windows RPC client and `intimatr-bottles speed` native Linux forwarder now both accept `hold MULTIPLIER DURATION_MS` (1–60,000 ms) and `release`, alongside preset/reset/status. The duration and multiplier are validated before launching the Windows client. For example, `intimatr-speed.exe --tcp 127.0.0.1:31379 hold 2.0 1800` commands the shared speed session in an **already running** fixture; `release` ends the temporary hold. In the allowlisted Wine test fixture, imported timer callbacks themselves expire that hold even if no further RPC requests arrive. Ordinary processes still report `clock_applied_to_game=false`; the Linux CLI's Wine prefix selection is **not** proof that a TCP endpoint belongs to that prefix, so use a trusted explicitly configured endpoint and never claim this is already a production game-speed controller.
