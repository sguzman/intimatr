# Extension points and subsystem invariants

Intimatr is deliberately extensible at a small number of shared boundaries. A new frontend or analysis feature should reuse those boundaries rather than reaching around them into process memory, debugger state, or duplicated scan/watch storage.

## Primary extension boundary: `CommandExecutor`

`Command` and `CommandResult` are the frontend-neutral API. The native CE-style UI, debugger UI, RPC server, and external RPC clients all consume the same shared executor created during bootstrap.

When adding a capability:

1. define or extend the platform-neutral command/result DTO;
2. implement policy and limit enforcement in the shared dispatcher/core;
3. add deterministic tests at that layer;
4. expose the existing command from whichever frontend needs it.

Do not implement memory writes, watch freezing, scan state, debugger state, or analysis state independently in a frontend. That creates divergent policy behavior and breaks the guarantee that UI and RPC see the same process-research session.

## Runtime/backpressure invariant

The runtime wraps the shared dispatcher in one bounded command executor. `[runtime].command_workers` limits active command workers and `[runtime].command_queue_capacity` bounds queued work. Frontends may use their own lightweight response channels, but substantive command execution must pass through this shared pressure boundary.

Shutdown ownership is also centralized in the runtime. New long-lived subsystems need a handle whose stop/join operation can be called before the command pool is shut down. A subsystem must not independently tear down the shared executor just because its own transport or window is closing.

## Memory and scanner extension points

`MemorySource` is the read-side abstraction used by scalar scanning, AOB scanning, disassembly, pointer operations, and structure inspection. `MemoryTarget` adds writes. New analysis algorithms should be written against these traits unless they genuinely require a new platform primitive.

Keep Windows-specific unsafe/FFI work under the platform/debugger Windows modules. Pure traversal, predicate, serialization, address-expression, and state-machine logic should remain platform-neutral and synthetic-memory-testable.

Scanner traversal invariants include:

- only scanner-eligible normalized regions are visited;
- configured alignment and result ceilings are authoritative;
- chunk overlap must preserve cross-boundary matches;
- unreadable chunks/candidates are isolated rather than corrupting state;
- cancellation remains observable during long scans;
- historical predicates compare against the immediately previous snapshot.

## Analysis extension point

Advanced analysis enters through `Command::Analysis` and `AnalysisCommand`. `AnalysisCommand::Batch` is the automation composition primitive; it deliberately invokes normal analysis operations rather than creating a second scripting engine with separate policy semantics.

Persistent analysis state is versioned. Any incompatible change to serialized workspace meaning must advance the workspace format version and document migration/compatibility behavior.

## Debugger extension point

`DebuggerCore` owns debugger bookkeeping. The Windows backend owns thread-context FFI, hardware debug registers, the scoped vectored exception handler, and the fixed-capacity debugger event ring. New debugger UI controls or RPC operations should call the core through shared commands.

Exception handling must remain narrowly scoped to Intimatr-owned state. Unrelated exceptions continue searching. Do not turn the VEH layer into a generic exception swallow/hiding mechanism.

## RPC and external frontends

RPC is a transport for the command contract, not a parallel feature implementation. Protocol version 1 uses four-byte big-endian length-prefixed JSON. New compatible command variants can be added without changing the framing version; incompatible framing or envelope semantics require a protocol-version change.

TCP remains loopback-only and named pipes reject remote clients. Connection counts and frame sizes stay bounded by TOML. External frontends should treat stable command/error DTOs and explicit version fields as their compatibility surface.

## Configuration invariant

There is exactly one TOML file per target executable: `config/<ExecutableName>.toml`. Tunable runtime behavior belongs in that file rather than frontend-specific constants when it materially affects resource use, policy, transport, scanner behavior, debugger behavior, or UI behavior.

## Loader/lifecycle invariant

`DllMain` stays minimal. No filesystem parsing, GUI setup, RPC startup, scan allocation, logging initialization, thread joining, or debugger setup belongs under loader lock. Bootstrap and teardown happen on normal runtime threads.

During orderly shutdown, stop request producers first, then stop/join UI threads, cancel/drain shared command work and debugger ownership, emit the final lifecycle log, and explicitly drain the non-blocking logging worker. Process termination can always preempt user-space cleanup, so release notes and code should describe this as best-effort orderly/crash hardening rather than claiming impossible durability after an abrupt process kill.

## Scope invariant

Intimatr remains an offline/single-player memory-research toolkit. Extension work must not add anti-cheat bypass, stealth, protection evasion, or multiplayer-oriented behavior.
