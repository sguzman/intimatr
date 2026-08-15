from pathlib import Path

path = Path("src/analysis.rs")
before = path.read_text()
text = before

old_call = """        let candidates = scan_pointer_predecessors(
            source,
            current_target,
            filter,
            scanner_config.chunk_size_bytes,
            options.pointer_size,
            options.alignment,
            options.max_offset,
            options.max_results.saturating_sub(results.len()),
        )?;"""
new_call = """        let predecessor_options = PointerPredecessorScanOptions {
            filter,
            configured_chunk_size: scanner_config.chunk_size_bytes,
            pointer_size: options.pointer_size,
            alignment: options.alignment,
            max_offset: options.max_offset,
            max_results: options.max_results.saturating_sub(results.len()),
        };
        let candidates =
            scan_pointer_predecessors(source, current_target, predecessor_options)?;"""
if old_call not in text:
    raise SystemExit("pointer predecessor call marker missing")
text = text.replace(old_call, new_call, 1)

old_fn = """fn scan_pointer_predecessors<S: MemorySource + ?Sized>(
    source: &S,
    target: u64,
    filter: RegionFilter,
    configured_chunk_size: usize,
    pointer_size: u8,
    alignment: usize,
    max_offset: u64,
    max_results: usize,
) -> Result<Vec<(u64, i64)>, AnalysisError> {
    let width = pointer_size as usize;
    let chunk_size = configured_chunk_size.max(width);"""
new_fn = """#[derive(Debug, Clone, Copy)]
struct PointerPredecessorScanOptions {
    filter: RegionFilter,
    configured_chunk_size: usize,
    pointer_size: u8,
    alignment: usize,
    max_offset: u64,
    max_results: usize,
}

fn scan_pointer_predecessors<S: MemorySource + ?Sized>(
    source: &S,
    target: u64,
    options: PointerPredecessorScanOptions,
) -> Result<Vec<(u64, i64)>, AnalysisError> {
    let width = options.pointer_size as usize;
    let chunk_size = options.configured_chunk_size.max(width);"""
if old_fn not in text:
    raise SystemExit("pointer predecessor function marker missing")
text = text.replace(old_fn, new_fn, 1)

marker = "fn scan_pointer_predecessors<S: MemorySource + ?Sized>("
start = text.index(marker)
end_marker = "\n#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]\n#[serde(tag = \"kind\", rename_all = \"snake_case\")]\npub enum StructureFieldKind"
end = text.index(end_marker, start)
prefix, body, suffix = text[:start], text[start:end], text[end:]
body = body.replace("region.is_scannable(filter)", "region.is_scannable(options.filter)")
body = body.replace("% alignment != 0", "% options.alignment != 0")
body = body.replace("if pointer_size == 4", "if options.pointer_size == 4")
body = body.replace(
    "if delta <= max_offset && delta <= i64::MAX as u64",
    "if delta <= options.max_offset && delta <= i64::MAX as u64",
)
body = body.replace(
    "if results.len() >= max_results",
    "if results.len() >= options.max_results",
)
text = prefix + body + suffix

old_test = """        let mut config = ScannerConfig::default();
        config.chunk_size_bytes = 4;"""
new_test = """        let config = ScannerConfig {
            chunk_size_bytes: 4,
            ..ScannerConfig::default()
        };"""
if old_test not in text:
    raise SystemExit("test config marker missing")
text = text.replace(old_test, new_test, 1)

if text == before:
    raise SystemExit("analysis source did not change")
path.write_text(text)
