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
