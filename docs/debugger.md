# Debugger

Intimatr's debugger is an in-process, frontend-neutral debugger service for offline single-player research. The native debugger window and RPC clients both issue the same shared commands; neither frontend owns an independent breakpoint, pause, or event model.

## Runtime model

The debugger is initialized after DLL bootstrap, configuration, and logging. Nothing in `DllMain` creates a window, suspends a thread, registers an exception handler, or decodes instructions.

When `[debugger].ui_enabled = true` and `policy.allow_debugger = true`, Intimatr starts a dedicated eframe/egui debugger tool window on its own Windows UI thread. The window has target-specific persisted state under `ui/<ExecutableName>/debugger` beside the DLL. The sample configuration toggles it with `F10`.

The debugger UI sends shared commands through worker threads so thread-context operations and memory-backed disassembly do not block the eframe event loop.

## Configuration

The `[debugger]` section controls:

```toml
[debugger]
enabled = true
break_on_start = false
prefer_hardware_breakpoints = true
max_disassembly_bytes = 65536
max_disassembly_instructions = 512
max_hardware_breakpoints = 32
max_events_per_poll = 256
ui_enabled = true
ui_initially_visible = false
ui_always_on_top = false
ui_toggle_key = "F10"
ui_width = 1160.0
ui_height = 760.0
event_poll_ms = 100
disassembly_default_bytes = 256
disassembly_default_instructions = 64
```

`policy.allow_debugger` is the shared outer policy gate. Disassembly also needs `policy.allow_memory_read` because it reads bytes through the normal memory backend.

## Thread contexts and control

`list_threads` enumerates threads owned by the current process. `read_thread_registers` captures an x64 register/context snapshot for a selected non-current thread.

Intimatr does not try to suspend the command worker that is currently executing a debugger request. For another running thread, a register read takes a temporary suspension, calls `GetThreadContext`, and restores that suspension immediately. If a thread is already explicitly paused by Intimatr, the existing owned suspension is reused for context access.

The x64 `CONTEXT` buffer passed to Windows is wrapped in explicit 16-byte alignment before `GetThreadContext` and `SetThreadContext` calls.

Explicit `pause_thread`/`resume_thread` state is conservative:

- only the selected thread is affected;
- Intimatr records only suspensions it created;
- a pre-existing non-zero suspend count causes the pause attempt to roll back and fail rather than claiming someone else's suspension;
- shutdown attempts to resume every thread still recorded as Intimatr-owned.

`single_step_thread` sets the trap flag in the selected thread's context. If the thread was explicitly paused by Intimatr, the command resumes it so exactly one instruction can execute. The resulting single-step exception is consumed only if Intimatr has registered that thread as an outstanding step.

## Disassembly

`disassemble` reads bytes through `MemorySource` and decodes them with `iced-x86`. The command supports 16-, 32-, and 64-bit decoding, although the current Windows process debugger is primarily x64.

Each result line contains:

- instruction address
- exact decoded bytes
- Intel-syntax formatted instruction text

The debugger UI can load disassembly at a typed address or directly from a captured RIP. Clicking a disassembly address copies it into the breakpoint form.

## Hardware breakpoints

Hardware breakpoints use DR0–DR3 on a specific target thread. Intimatr supports:

- execute breakpoints, size 1
- write breakpoints, sizes 1/2/4/8
- read/write breakpoints, sizes 1/2/4/8

Data breakpoint addresses must satisfy the architecture's size alignment. Each thread has four hardware address slots regardless of the global configured breakpoint limit.

Installing or clearing a hardware breakpoint temporarily suspends a running selected thread, reads its debug-register context, updates DR0–DR3/DR7, writes the context back, and resumes the temporary suspension. If the thread is already explicitly paused by Intimatr, that owned suspension is reused.

These breakpoints do not write trap opcodes into game code.

## Scoped vectored exception handling

The Windows backend acquires a vectored exception handler only when a hardware breakpoint or single-step operation needs one.

For `EXCEPTION_SINGLE_STEP`, the handler checks Intimatr's own per-thread registries before consuming anything:

- DR6 hit bits are accepted only for a DR slot registered by Intimatr on that thread;
- trap-flag single-step events are accepted only for a thread with an outstanding Intimatr step;
- unrelated single-step exceptions return `EXCEPTION_CONTINUE_SEARCH`.

Hardware-breakpoint and single-step hits are recorded into a bounded sequence-numbered event ring. Milestone 5 treats hardware-breakpoint hits as trace-style events that auto-continue; it does not claim an external-debugger-style global stopped process.

## Event feed

The shared command is:

```text
debugger_events(after_sequence, limit)
```

It returns ordered events plus the current latest sequence. The native debugger UI polls this feed using its configured `event_poll_ms`; RPC clients can use the same cursor protocol. The ring is bounded, so it is an incremental live feed rather than durable event storage.

Event kinds currently include:

- `hardware_breakpoint { slot }`
- `single_step`

Each event records sequence, thread ID, and instruction/exception address.

## Shutdown invariants

Debugger shutdown is best-effort but ordered around owned state. Intimatr removes registered hardware breakpoints, resumes threads it still owns, releases the scoped vectored exception handler, and then lets the surrounding runtime tear down the native windows/RPC/logging outside loader-lock work.

The debugger intentionally does not include anti-cheat bypass, stealth, protection evasion, or multiplayer-oriented behavior.
