# Native CE-style UI

Intimatr's first-party UI is a native Windows tool window running inside the target process. It uses eframe/egui with the Glow renderer and is created from a dedicated post-bootstrap thread. `DllMain` does not create the window, initialize the renderer, or run the event loop.

The UI talks only to the shared `CommandExecutor`. It does not own a second scanner, watch list, memory backend, or policy layer. RPC and the UI therefore operate on the same scans and watches.

## Configuration

The per-game TOML file controls the UI:

```toml
[ui]
enabled = true
initially_visible = true
always_on_top = false
toggle_key = "Insert"
width = 1180.0
height = 760.0
watch_refresh_ms = 250
scan_page_size = 256
```

`toggle_key` accepts `Insert`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `Pause`, `F1` through `F24`, and single alphanumeric keys. The key toggles native window visibility. Closing the window hides it rather than unloading Intimatr.

`scan_page_size` must not exceed `rpc.max_scan_results_per_page`, because result paging is enforced by the shared command layer rather than bypassed by the UI.

## Panels

### Scan

The scan panel exposes every current typed predicate: unknown initial, exact/not-equal, numeric comparisons, inclusive range, changed/unchanged, increased/decreased, and delta scans. Supported value types are `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, and `f64`.

First and next scans run off the UI event loop through command-worker threads. While a scan is active, the UI polls frontend-neutral active-scan progress through the shared command layer and shows bytes scanned, total bytes, current result count, approximate throughput, and an in-place cancel action. Unrelated tab errors do not hide the separate running-scan indicator. Results are paged through `ScanResults`, and an address can be added directly to the shared watch list.

### Watches

Watches contain an address, scalar type, optional label, and optional frozen value. The UI refreshes watches at `watch_refresh_ms`. When a watch is frozen, the shared dispatcher writes the stored value through the normal write-policy path before reading the value back. Freeze is therefore visible to RPC and denied when memory writes are disabled.

### Memory

The memory panel accepts a hexadecimal address and byte count, displays reads in 16-byte hex/ASCII rows, and accepts raw hexadecimal bytes for policy-gated writes.

### Modules and threads

The module panel enumerates the current process's loaded modules with ToolHelp and displays module name, base, size, and path. The thread panel enumerates current-process thread IDs. Register/context snapshots and debugger controls are intentionally deferred to Milestone 5.

## Persistence and lifecycle

eframe persistence is scoped beneath `ui/<ExecutableName>` next to the DLL. Window placement/size and selected UI form state can therefore be retained separately for each target executable.

Runtime shutdown signals the UI context to close and joins the UI thread outside `DllMain`. The UI itself keeps blocking memory/scan commands off the render/event-loop thread and receives completed command results through an internal channel.
