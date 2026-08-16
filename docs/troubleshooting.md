# Intimatr troubleshooting

Use this guide from the outside in: first prove the DLL loaded, then prove configuration resolved, then prove the shared runtime started, and only after that debug a specific scanner/UI/debugger/RPC subsystem.

## No Intimatr log appears

Expected default path:

```text
<intimatr directory>/logs/intimatr.log
```

If no log file is created at all, the most likely problem is before normal Intimatr runtime startup: the DLL was not loaded, the host rejected the loading mechanism, or the bootstrap thread never reached logging initialization.

Confirm that you are using an authorized in-process DLL-loading path for the offline target and that the DLL architecture matches the Windows x86_64 target.

## The log reports a missing configuration

The config filename must be the executable filename, including `.exe`:

```text
config/MyGame.exe.toml
```

The file must also contain:

```toml
[target]
executable = "MyGame.exe"
```

The comparison is case-insensitive, but the executable identity still has to refer to the same host process. Do not use a generic `intimatr.toml` or reuse one target's file for another executable without changing `[target].executable`.

## The scanner UI does not appear

Check:

```toml
[ui]
enabled = true
initially_visible = true
toggle_key = "Insert"
```

With the example config, press `Insert`. Closing the UI window hides it, so pressing the configured toggle key should bring it back.

If logging shows runtime startup succeeded but the UI thread failed, keep the log around; UI startup and shutdown are intentionally separate from `DllMain` so failures should be visible in normal tracing.

## The debugger window does not appear

All relevant gates have to agree:

```toml
[debugger]
enabled = true
ui_enabled = true
ui_toggle_key = "F10"

[policy]
allow_debugger = true
```

The example debugger hotkey is `F10`.

If debugger access is intentionally disabled by policy, the scanner/memory UI can still be used independently.

## Reads work but writes are denied

Memory writes are a separate policy capability:

```toml
[policy]
allow_memory_read = true
allow_memory_write = true
```

Writes that touch executable memory additionally require:

```toml
allow_code_patch = true
```

Do not enable code patching merely to make an unrelated data write succeed. First confirm the target address and region classification.

## A frozen watch stops working after restart

Absolute addresses can become stale after process layout changes. Prefer reusable module-relative watch expressions such as:

```text
Game.exe+0x1234
```

Saved scalar scan sessions are snapshots of their original candidate addresses; they are not guaranteed to remain valid across process restarts. Re-scan or revalidate them.

## A scan returns no candidates

Check the scanner filters before assuming the value is absent:

```toml
[scanner]
require_readable = true
require_writable = false
require_executable = false
include_guard_pages = false
alignment = 1
```

Also verify the value type and width. A visible integer may be stored as an unsigned type, a wider integer, or a float. For uncertain values, an unknown-initial-value scan followed by changed/unchanged/increased/decreased filtering is often more appropriate than guessing an exact first value.

## A scan returns too many candidates or stops at a ceiling

The scanner is deliberately bounded:

```toml
[scanner]
max_results = 2000000
chunk_size_bytes = 1048576
alignment = 1
```

Narrow the region filters, choose the correct value type, increase alignment only when the target representation justifies it, or perform historical next scans to reduce candidates. Raising `max_results` increases retained in-process state and is not the first tuning knob to reach for.

## RPC cannot connect

The example config uses:

```toml
[rpc]
enabled = true
transport = "tcp"
bind = "127.0.0.1:31337"
```

TCP RPC is restricted to loopback. Confirm that the configured port is free and that the client is connecting to the same loopback address/port.

Intimatr RPC is not newline-delimited JSON. It uses a four-byte big-endian message length followed by a JSON request frame. A raw `telnet`/`nc` session will not behave like a valid client.

For named pipes, verify the configured `pipe_name` and use the Windows named-pipe transport described in `docs/rpc.md`.

## RPC responds with backpressure/busy behavior

Runtime command execution is intentionally bounded:

```toml
[runtime]
command_workers = 4
command_queue_capacity = 64
```

RPC also limits concurrent clients and frame sizes. A client should avoid flooding long scans or large reads faster than the bounded worker pool can service them. Treat backpressure as a signal to reduce request concurrency rather than as a reason to remove bounds.

## A debugger operation refuses a thread

Intimatr does not pretend to own pre-existing suspension state. If a selected thread already has a non-zero suspend count when Intimatr tries to take ownership, the operation is rejected instead of guessing who suspended it.

The command worker's current thread is also excluded from debugger context mutation/suspension.

## Hardware breakpoints do not behave like a global external debugger

Intimatr's debugger is intentionally narrower than x64dbg/WinDbg. DR0-DR3 breakpoints are per-thread. The scoped vectored exception handler records only Intimatr-owned single-step/hardware-breakpoint events and lets unrelated exceptions continue searching.

Breakpoint hits are trace-style events; do not assume a process-wide stop-the-world debugger model.

## The UI closed but Intimatr is still loaded

That is expected. Closing a native tool window hides it. Use the configured hotkey to show it again.

A full runtime shutdown is separate so Intimatr can reject new work, cancel active scans, release owned debugger state, stop RPC/UI threads, and flush tracing in a controlled order.

## Remote shutdown is denied

The example policy intentionally contains:

```toml
allow_remote_shutdown = false
```

Enable it only when you explicitly want an RPC client to be able to request runtime shutdown.

## The process terminated before logs finished flushing

Intimatr performs orderly shutdown and explicitly drains the non-blocking tracing worker, but an operating-system kill or abrupt process termination can preempt user-space cleanup. Treat the last complete log entry as the final reliable lifecycle boundary.

## Verify a packaged release

From a repository checkout:

```powershell
.\scripts\verify-release.ps1 -ArchivePath .\dist\intimatr-v0.1.0-windows-x86_64.zip -ExpectedVersion 0.1.0
```

The verifier checks the expected distribution files, `VERSION.txt`, a non-empty DLL, and the DLL SHA-256 recorded in `SHA256SUMS.txt`.
