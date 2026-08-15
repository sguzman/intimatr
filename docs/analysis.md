# Advanced analysis

Milestone 6 adds frontend-neutral search and analysis primitives on top of Intimatr's existing `MemorySource`, scanner configuration, shared command dispatcher, and RPC protocol. The native DLL and arbitrary RPC clients therefore use one implementation of pattern scans, address resolution, pointer chains, saved analysis state, and structure inspection.

## Array-of-bytes scanning

The `aob_scan` analysis command scans the same filtered memory regions used by the scalar scanner. It preserves chunk overlap so patterns that cross a chunk boundary are not missed, and it applies explicit alignment and result limits.

Pattern tokens are two hexadecimal nibbles. Either nibble may be a wildcard:

```text
48 8B ?? FF
48 8B ?F A?
```

`??` matches any byte, `?F` matches any byte whose low nibble is `F`, and `A?` matches any byte whose high nibble is `A`. Matching is mask-based; the scanner does not expand wildcard patterns into many concrete byte strings.

The scan obeys the existing scanner region policy (`require_readable`, writable/executable filters, and guard-page behavior), configured chunk size, and `scanner.max_results`. An analysis request may choose a smaller result cap and its own positive alignment.

## Address expressions

Analysis commands that accept an address expression support:

```text
0x140001234
5368713780
Game.exe
Game.exe+0x1234
Game.exe-0x20
game-client.dll+0x40
```

Absolute decimal and hexadecimal addresses resolve directly. Module names and full module paths are matched case-insensitively against the current process module list. A module without an offset resolves to its base. The parser uses the rightmost numeric `+`/`-` suffix, so punctuation such as a hyphen inside a module filename does not prevent normal module-relative addressing.

Module-relative expressions are preferred for reusable state because they naturally follow ASLR when a module is loaded at a different base in a later process.

## Pointer-chain resolution

A pointer-chain specification contains a base expression, a pointer width (`4` or `8` bytes), and signed offsets. Resolution uses this convention for every offset:

1. read a pointer at the current address;
2. add the signed offset to that pointer;
3. use the result as the next current address.

For example, a base of `Game.exe+0x1000` with offsets `[0x20, 0x18]` means:

```text
current = resolve(Game.exe+0x1000)
current = *(ptr current) + 0x20
current = *(ptr current) + 0x18
```

The result reports the resolved base, each pointer value that was dereferenced, and the final address.

## Pointer-chain search

`search_pointer_chains` performs a bounded reverse search for pointer locations that can reach a target through small non-negative offsets. Its request controls:

- maximum depth (currently capped at 8);
- maximum positive offset;
- pointer width (`4` or `8` bytes);
- pointer alignment;
- maximum returned paths.

The search walks only scanner-eligible regions and respects the shared scanner result ceiling. It is intentionally bounded rather than pretending that an unconstrained whole-process pointer graph is cheap. Explicit pointer-chain resolution still supports signed offsets; the current reverse-search primitive narrows discovery to non-negative offsets for predictable breadth and cost.

Returned `PointerPath` values contain a root address and the ordered offsets needed to walk from that root toward the target using the same dereference-then-add convention as explicit pointer-chain resolution.

## Structure-oriented inspection

`inspect_structure` resolves one base expression and reads a list of named fields at signed offsets from that base. A field may be:

- any existing Intimatr scalar `ValueType`;
- a 4- or 8-byte pointer;
- a bounded raw byte range.

Each returned field includes its resolved address and typed value. Raw byte fields are constrained by the shared command memory-transfer limit rather than introducing a second unbounded read path.

Structure inspection is deliberately descriptive and read-oriented. Memory writes still go through the existing shared write commands and their policy gates.

## Saved scans and watch templates

The analysis workspace can name and retain current scalar scan sessions and reusable watch templates.

Saving a scan stores the complete `ScanSession`, including its candidates and snapshot history. Restoring it creates a new live scan ID in the shared dispatcher. Saved scan addresses are process snapshots: they can become stale after a process restart or layout change, so a restored scan should be treated as historical state until validated against the current process.

Saving a watch creates a reusable template. When the watch address belongs to a loaded module, Intimatr stores it as `ModuleName+0xOFFSET`; otherwise it stores an absolute hexadecimal address. This makes module-backed watch templates ASLR-friendly. Frozen values remain subject to `policy.allow_memory_write` when a template is instantiated.

A workspace is serialized as versioned JSON beneath:

```text
analysis/<ExecutableName>/<WorkspaceName>.json
```

beside `intimatr.dll`. Workspace names are validated so they cannot escape the per-target analysis directory.

## Shared command and RPC automation

All advanced analysis operations enter through:

```text
Command::Analysis { request }
```

The nested `AnalysisCommand` is serialized by the existing RPC framing; there is no second automation server or scripting engine. A representative RPC request is:

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

The command result keeps the protocol's normal outer `result` tag and places the advanced-analysis payload in its `analysis` field.

`batch` accepts up to 128 non-batch `AnalysisCommand` values and executes them sequentially through the same shared implementation. Nested batches are rejected. Policy checks, memory access, limits, scan/watch state, and workspace state are therefore identical whether a caller sends one operation or automates a sequence through RPC.

The subsystem remains scoped to offline/single-player memory research. It does not add anti-cheat bypass, stealth, protection evasion, or multiplayer-oriented behavior.
