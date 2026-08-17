from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def patch(path: str, old: str, new: str, expected: int = 1) -> None:
    target = ROOT / path
    text = target.read_text(encoding="utf-8")
    count = text.count(old)
    if count != expected:
        raise RuntimeError(f"{path}: expected {expected} occurrences, found {count}: {old[:100]!r}")
    target.write_text(text.replace(old, new), encoding="utf-8", newline="\n")


# Scoped scan workers use ? for ScanError-convertible operations, so state the
# closure result type explicitly instead of asking inference to choose among
# several From implementations.
patch(
    "src/scanner/engine.rs",
    "handles.push(scope.spawn(move || {",
    "handles.push(scope.spawn(move || -> Result<Vec<ScanCandidate>, ScanError> {",
    expected=2,
)

# Progress events use a rendezvous channel. A worker reaches the coordinator at
# a chunk/batch boundary before claiming more work, which keeps cancellation
# requested by a progress consumer responsive even when worker threads are much
# faster than the coordinator on tiny synthetic scans.
patch(
    "src/scanner/engine.rs",
    "mpsc::{self, Sender},",
    "mpsc::{self, SyncSender},",
)
patch(
    "src/scanner/engine.rs",
    "let (event_tx, event_rx) = mpsc::channel();",
    "let (event_tx, event_rx) = mpsc::sync_channel(0);",
    expected=2,
)
patch(
    "src/scanner/engine.rs",
    "struct WorkerDone(Sender<WorkerEvent>);",
    "struct WorkerDone(SyncSender<WorkerEvent>);",
)

# Inside each scoped worker these names are already references to the shared
# atomics; do not take a redundant second reference when reserving a result.
patch(
    "src/scanner/engine.rs",
    """                                    )? && reserve_result_slot(
                                        &accepted_results,
                                        options.max_results,
                                        &truncated,
                                        &stop,
                                    ) {""",
    """                                    )? && reserve_result_slot(
                                        accepted_results,
                                        options.max_results,
                                        truncated,
                                        stop,
                                    ) {""",
)
patch(
    "src/scanner/engine.rs",
    """                                    && reserve_result_slot(
                                        &accepted_results,
                                        options.max_results,
                                        &truncated,
                                        &stop,
                                    )""",
    """                                    && reserve_result_slot(
                                        accepted_results,
                                        options.max_results,
                                        truncated,
                                        stop,
                                    )""",
)

# The original transformer computed this inside egui's horizontal closure.
# Recompute it in the outer render_scan scope before rendering progress.
patch(
    "src/ui.rs",
    "\n        if scanning {\n            ui.horizontal(|ui| {\n                ui.spinner();",
    "\n        let scanning = self.pending.contains(&UiTaskKind::FirstScan)\n            || self.pending.contains(&UiTaskKind::NextScan);\n        if scanning {\n            ui.horizontal(|ui| {\n                ui.spinner();",
)

# Keep the human-readable task-label match exhaustive after adding progress
# polling as a frontend-neutral command.
patch(
    "src/ui.rs",
    '        UiTaskKind::CancelScan => "scan cancellation",\n',
    '        UiTaskKind::CancelScan => "scan cancellation",\n        UiTaskKind::ActiveScans => "active scan progress",\n',
)

print("field scan compile fixes applied")
