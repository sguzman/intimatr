# Intimatr quick start

This guide is for an already-authorized offline/single-player target that can load `intimatr.dll`. Intimatr does not provide an anti-cheat bypass, stealth loader, or protection-evasion path.

## 1. Start from the packaged distribution

A packaged Windows x86_64 release contains the DLL, example per-game configuration, documentation, version metadata, and checksums. Keep the distribution together while you do the first launch so paths remain predictable.

The important layout is:

```text
<intimatr directory>/
├── intimatr.dll
├── VERSION.txt
├── SHA256SUMS.txt
├── config/
│   └── ExampleGame.exe.toml
├── docs/
└── ...
```

`config`, `logs`, `ui`, and `analysis` are resolved relative to `intimatr.dll`, not the current shell directory.

## 2. Verify the DLL

`SHA256SUMS.txt` contains the SHA-256 digest of the packaged `intimatr.dll`.

```powershell
Get-FileHash .\intimatr.dll -Algorithm SHA256
Get-Content .\SHA256SUMS.txt
```

For a repository-built release archive, `scripts/verify-release.ps1` validates the archive layout, version metadata, and DLL checksum together.

## 3. Create the target configuration

Copy the example file and rename it to the exact executable filename, including `.exe`:

```powershell
Copy-Item .\config\ExampleGame.exe.toml .\config\MyGame.exe.toml
```

Then change:

```toml
[target]
executable = "MyGame.exe"
```

The filename and `[target].executable` must both match the host executable case-insensitively. One target executable gets one TOML file.

For the first launch, a conservative policy is useful while confirming bootstrap, logs, and scanning:

```toml
[policy]
allow_memory_read = true
allow_memory_write = false
allow_code_patch = false
allow_debugger = false
allow_remote_shutdown = false
```

After the read-only path works, enable write or debugger capabilities deliberately if the target and your workflow require them.

## 4. Load the DLL using the target's supported mechanism

Place or load `intimatr.dll` using the authorized DLL-loading mechanism for the target. Intimatr expects to execute in-process; it is not an external attach-based debugger.

On successful bootstrap, Intimatr resolves the host executable, loads `config/<ExecutableName>.toml`, initializes logging, creates the shared command runtime, then starts configured frontends.

If bootstrap fails, check `logs/intimatr.log` beside the DLL before changing anything else.

## 5. Open the native scanner UI

With the example UI configuration:

- `Insert` toggles the CE-style scanner/memory window.
- `F10` toggles the debugger window when debugger support and debugger policy are enabled.

The scanner window is initially visible in the example configuration. Closing the window hides it; it does not unload the DLL.

A basic first scan workflow is:

1. Choose the value type that matches the value you expect in memory.
2. Choose `Exact`, `Unknown initial value`, or another first-scan predicate.
3. Run the first scan.
4. Change the value in the target.
5. Run a next scan with `Changed`, `Increased`, `Decreased`, `Exact`, or another historical predicate.
6. Add a surviving address to the shared watch list.
7. Only enable freeze/write operations after memory writes are intentionally allowed in the target TOML.

## 6. Confirm RPC when you need automation

The example configuration enables loopback TCP RPC at `127.0.0.1:31337`. Intimatr RPC is local-only by design and uses versioned, four-byte big-endian length-prefixed JSON frames rather than newline-delimited JSON.

The repository includes `examples/rpc_client.rs` as the reference client. See `docs/rpc.md` before implementing another frontend.

If you do not need RPC, set:

```toml
[rpc]
enabled = false
```

## 7. Persist useful analysis state

Saved analysis workspaces live under `analysis/<ExecutableName>` beside the DLL. Reusable watch definitions should prefer module-relative expressions such as `Game.exe+0x1234` when possible because absolute addresses can become stale after ASLR or a restart.

UI state is stored separately under `ui/<ExecutableName>`.

## 8. Shut down cleanly

Closing a tool window only hides that frontend. Runtime shutdown is an explicit lifecycle operation so Intimatr can stop new work, cancel scans, release debugger-owned state, join frontend/RPC threads, and flush logs outside loader lock.

Remote shutdown remains disabled in the example policy. See `docs/rpc.md` and `docs/troubleshooting.md` for lifecycle and recovery details.
