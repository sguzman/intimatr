use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::Path,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::{
    config::ScannerConfig,
    memory::{MemoryError, MemorySource, RegionFilter},
    scanner::{ScanSession, ScalarValue, ValueType},
};

pub const WORKSPACE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternByte {
    pub value: u8,
    pub mask: u8,
}

impl PatternByte {
    fn matches(self, byte: u8) -> bool {
        byte & self.mask == self.value & self.mask
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytePattern {
    pub bytes: Vec<PatternByte>,
}

impl BytePattern {
    pub fn parse(input: &str) -> Result<Self, AnalysisError> {
        let mut bytes = Vec::new();
        for token in input.split_whitespace() {
            if token.len() != 2 {
                return Err(AnalysisError::InvalidPatternToken(token.to_owned()));
            }
            let raw = token.as_bytes();
            let (high_value, high_mask) = parse_pattern_nibble(raw[0], token)?;
            let (low_value, low_mask) = parse_pattern_nibble(raw[1], token)?;
            bytes.push(PatternByte {
                value: (high_value << 4) | low_value,
                mask: (high_mask << 4) | low_mask,
            });
        }
        if bytes.is_empty() {
            return Err(AnalysisError::EmptyPattern);
        }
        Ok(Self { bytes })
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn matches_at(&self, haystack: &[u8], offset: usize) -> bool {
        self.bytes
            .iter()
            .enumerate()
            .all(|(index, expected)| expected.matches(haystack[offset + index]))
    }
}

fn parse_pattern_nibble(byte: u8, token: &str) -> Result<(u8, u8), AnalysisError> {
    if byte == b'?' {
        return Ok((0, 0));
    }
    let value = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return Err(AnalysisError::InvalidPatternToken(token.to_owned())),
    };
    Ok((value, 0x0f))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternScanOptions {
    pub alignment: usize,
    pub max_results: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternScanResult {
    pub pattern: String,
    pub addresses: Vec<u64>,
    pub truncated: bool,
    pub read_failures: u64,
}

pub fn scan_pattern<S: MemorySource + ?Sized>(
    source: &S,
    pattern_text: &str,
    scanner_config: &ScannerConfig,
    options: PatternScanOptions,
) -> Result<PatternScanResult, AnalysisError> {
    if options.alignment == 0 {
        return Err(AnalysisError::InvalidLimit("pattern alignment"));
    }
    if options.max_results == 0 || options.max_results > scanner_config.max_results {
        return Err(AnalysisError::InvalidLimit("pattern max_results"));
    }
    let pattern = BytePattern::parse(pattern_text)?;
    let filter = RegionFilter::from(scanner_config);
    let chunk_size = scanner_config.chunk_size_bytes.max(pattern.len());
    let overlap = pattern.len().saturating_sub(1);
    let mut addresses = Vec::new();
    let mut read_failures = 0_u64;
    let mut truncated = false;

    let mut regions = source.regions()?;
    regions.sort_unstable_by_key(|region| region.base);
    for region in regions {
        if !region.is_scannable(filter) || region.size < pattern.len() {
            continue;
        }
        let region_end = region.end()?;
        let mut cursor = region.base;
        while cursor < region_end {
            let remaining = region_end - cursor;
            let read_len = remaining.min(chunk_size.saturating_add(overlap));
            if read_len < pattern.len() {
                break;
            }
            let mut buffer = vec![0_u8; read_len];
            if let Err(error) = source.read_exact(cursor, &mut buffer) {
                read_failures = read_failures.saturating_add(1);
                warn!(address = cursor, size = read_len, error = %error, "pattern scan skipped unreadable chunk");
                cursor = cursor.saturating_add(chunk_size);
                continue;
            }

            let max_offset = buffer.len() - pattern.len();
            for offset in 0..=max_offset {
                let address = cursor.saturating_add(offset);
                if (address - region.base) % options.alignment != 0 {
                    continue;
                }
                if pattern.matches_at(&buffer, offset) {
                    addresses.push(address as u64);
                    if addresses.len() >= options.max_results {
                        truncated = true;
                        break;
                    }
                }
            }
            if truncated {
                break;
            }
            cursor = cursor.saturating_add(chunk_size);
        }
        if truncated {
            break;
        }
    }

    info!(
        pattern = pattern_text,
        matches = addresses.len(),
        truncated,
        read_failures,
        "completed array-of-bytes scan"
    );
    Ok(PatternScanResult {
        pattern: pattern_text.to_owned(),
        addresses,
        truncated,
        read_failures,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDescriptor {
    pub name: String,
    pub path: String,
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddressExpression {
    Absolute { address: u64 },
    ModuleOffset { module: String, offset: i64 },
}

impl AddressExpression {
    pub fn parse(input: &str) -> Result<Self, AnalysisError> {
        let expression = input.trim();
        if expression.is_empty() {
            return Err(AnalysisError::InvalidAddressExpression(input.to_owned()));
        }
        if let Ok(address) = parse_unsigned(expression) {
            return Ok(Self::Absolute { address });
        }

        let mut split_index = None;
        for (index, character) in expression.char_indices().skip(1) {
            if character == '+' || character == '-' {
                split_index = Some((index, character));
                break;
            }
        }
        let Some((index, sign)) = split_index else {
            return Ok(Self::ModuleOffset {
                module: expression.to_owned(),
                offset: 0,
            });
        };
        let module = expression[..index].trim();
        let offset_text = expression[index + sign.len_utf8()..].trim();
        if module.is_empty() || offset_text.is_empty() {
            return Err(AnalysisError::InvalidAddressExpression(input.to_owned()));
        }
        let magnitude = parse_unsigned(offset_text)?;
        let magnitude = i64::try_from(magnitude)
            .map_err(|_| AnalysisError::InvalidAddressExpression(input.to_owned()))?;
        let offset = if sign == '-' { -magnitude } else { magnitude };
        Ok(Self::ModuleOffset {
            module: module.to_owned(),
            offset,
        })
    }

    pub fn resolve(&self, modules: &[ModuleDescriptor]) -> Result<u64, AnalysisError> {
        match self {
            Self::Absolute { address } => Ok(*address),
            Self::ModuleOffset { module, offset } => {
                let descriptor = modules
                    .iter()
                    .find(|candidate| {
                        candidate.name.eq_ignore_ascii_case(module)
                            || candidate.path.eq_ignore_ascii_case(module)
                    })
                    .ok_or_else(|| AnalysisError::ModuleNotFound(module.clone()))?;
                add_signed(descriptor.base, *offset)
            }
        }
    }
}

fn parse_unsigned(input: &str) -> Result<u64, AnalysisError> {
    let trimmed = input.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return u64::from_str_radix(hex, 16)
            .map_err(|_| AnalysisError::InvalidAddressExpression(input.to_owned()));
    }
    trimmed
        .parse::<u64>()
        .map_err(|_| AnalysisError::InvalidAddressExpression(input.to_owned()))
}

fn add_signed(base: u64, offset: i64) -> Result<u64, AnalysisError> {
    if offset >= 0 {
        base.checked_add(offset as u64)
            .ok_or(AnalysisError::AddressOverflow)
    } else {
        base.checked_sub(offset.unsigned_abs())
            .ok_or(AnalysisError::AddressOverflow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerChainSpec {
    pub base: String,
    pub offsets: Vec<i64>,
    pub pointer_size: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerChainResolution {
    pub base_address: u64,
    pub dereferenced: Vec<u64>,
    pub final_address: u64,
}

pub fn resolve_pointer_chain<S: MemorySource + ?Sized>(
    source: &S,
    modules: &[ModuleDescriptor],
    spec: &PointerChainSpec,
) -> Result<PointerChainResolution, AnalysisError> {
    validate_pointer_size(spec.pointer_size)?;
    let expression = AddressExpression::parse(&spec.base)?;
    let base_address = expression.resolve(modules)?;
    let mut current = base_address;
    let mut dereferenced = Vec::with_capacity(spec.offsets.len());
    for offset in &spec.offsets {
        let pointer = read_pointer(source, current, spec.pointer_size)?;
        dereferenced.push(pointer);
        current = add_signed(pointer, *offset)?;
    }
    Ok(PointerChainResolution {
        base_address,
        dereferenced,
        final_address: current,
    })
}

fn validate_pointer_size(pointer_size: u8) -> Result<(), AnalysisError> {
    if matches!(pointer_size, 4 | 8) {
        Ok(())
    } else {
        Err(AnalysisError::InvalidPointerSize(pointer_size))
    }
}

fn read_pointer<S: MemorySource + ?Sized>(
    source: &S,
    address: u64,
    pointer_size: u8,
) -> Result<u64, AnalysisError> {
    validate_pointer_size(pointer_size)?;
    let native = usize::try_from(address).map_err(|_| AnalysisError::AddressOverflow)?;
    let mut bytes = [0_u8; 8];
    source.read_exact(native, &mut bytes[..pointer_size as usize])?;
    Ok(if pointer_size == 4 {
        u64::from(u32::from_le_bytes(bytes[..4].try_into().expect("four-byte slice")))
    } else {
        u64::from_le_bytes(bytes)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerPath {
    pub root: u64,
    pub offsets: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerSearchOptions {
    pub max_depth: usize,
    pub max_offset: u64,
    pub pointer_size: u8,
    pub alignment: usize,
    pub max_results: usize,
}

pub fn search_pointer_chains<S: MemorySource + ?Sized>(
    source: &S,
    target: u64,
    scanner_config: &ScannerConfig,
    options: PointerSearchOptions,
) -> Result<Vec<PointerPath>, AnalysisError> {
    validate_pointer_size(options.pointer_size)?;
    if options.max_depth == 0 || options.max_depth > 8 {
        return Err(AnalysisError::InvalidLimit("pointer max_depth"));
    }
    if options.max_offset == 0 {
        return Err(AnalysisError::InvalidLimit("pointer max_offset"));
    }
    if options.alignment == 0 {
        return Err(AnalysisError::InvalidLimit("pointer alignment"));
    }
    if options.max_results == 0 || options.max_results > scanner_config.max_results {
        return Err(AnalysisError::InvalidLimit("pointer max_results"));
    }

    let filter = RegionFilter::from(scanner_config);
    let mut queue = VecDeque::from([(target, Vec::<i64>::new(), 0_usize)]);
    let mut visited_targets = HashSet::from([target]);
    let mut results = Vec::new();

    while let Some((current_target, suffix_offsets, depth)) = queue.pop_front() {
        if depth >= options.max_depth || results.len() >= options.max_results {
            continue;
        }
        let candidates = scan_pointer_predecessors(
            source,
            current_target,
            filter,
            scanner_config.chunk_size_bytes,
            options.pointer_size,
            options.alignment,
            options.max_offset,
            options.max_results.saturating_sub(results.len()),
        )?;
        for (root, offset) in candidates {
            let mut offsets = Vec::with_capacity(suffix_offsets.len() + 1);
            offsets.push(offset);
            offsets.extend_from_slice(&suffix_offsets);
            results.push(PointerPath {
                root,
                offsets: offsets.clone(),
            });
            if results.len() >= options.max_results {
                break;
            }
            if visited_targets.insert(root) {
                queue.push_back((root, offsets, depth + 1));
            }
        }
    }

    debug!(target, results = results.len(), "completed bounded pointer-chain search");
    Ok(results)
}

fn scan_pointer_predecessors<S: MemorySource + ?Sized>(
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
    let chunk_size = configured_chunk_size.max(width);
    let overlap = width.saturating_sub(1);
    let mut results = Vec::new();
    let mut regions = source.regions()?;
    regions.sort_unstable_by_key(|region| region.base);
    for region in regions {
        if !region.is_scannable(filter) || region.size < width {
            continue;
        }
        let end = region.end()?;
        let mut cursor = region.base;
        while cursor < end {
            let read_len = (end - cursor).min(chunk_size.saturating_add(overlap));
            if read_len < width {
                break;
            }
            let mut buffer = vec![0_u8; read_len];
            if source.read_exact(cursor, &mut buffer).is_err() {
                cursor = cursor.saturating_add(chunk_size);
                continue;
            }
            for offset in 0..=buffer.len() - width {
                let address = cursor + offset;
                if (address - region.base) % alignment != 0 {
                    continue;
                }
                let pointer = if pointer_size == 4 {
                    u64::from(u32::from_le_bytes(
                        buffer[offset..offset + 4].try_into().expect("four-byte slice"),
                    ))
                } else {
                    u64::from_le_bytes(
                        buffer[offset..offset + 8].try_into().expect("eight-byte slice"),
                    )
                };
                if pointer > target {
                    continue;
                }
                let delta = target - pointer;
                if delta <= max_offset && delta <= i64::MAX as u64 {
                    results.push((address as u64, delta as i64));
                    if results.len() >= max_results {
                        return Ok(results);
                    }
                }
            }
            cursor = cursor.saturating_add(chunk_size);
        }
    }
    Ok(results)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructureFieldKind {
    Scalar { value_type: ValueType },
    Pointer { pointer_size: u8 },
    Bytes { size: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureFieldSpec {
    pub name: String,
    pub offset: i64,
    pub kind: StructureFieldKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructureValue {
    Scalar { value: ScalarValue },
    Pointer { value: u64 },
    Bytes { value: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructureFieldValue {
    pub name: String,
    pub address: u64,
    pub value: StructureValue,
}

pub fn inspect_structure<S: MemorySource + ?Sized>(
    source: &S,
    modules: &[ModuleDescriptor],
    base: &str,
    fields: &[StructureFieldSpec],
    max_bytes_per_field: usize,
) -> Result<Vec<StructureFieldValue>, AnalysisError> {
    let base_address = AddressExpression::parse(base)?.resolve(modules)?;
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        if field.name.trim().is_empty() {
            return Err(AnalysisError::InvalidStructureField("empty field name"));
        }
        let address = add_signed(base_address, field.offset)?;
        let native = usize::try_from(address).map_err(|_| AnalysisError::AddressOverflow)?;
        let value = match field.kind {
            StructureFieldKind::Scalar { value_type } => {
                let width = value_type.byte_width();
                let mut bytes = [0_u8; 8];
                source.read_exact(native, &mut bytes[..width])?;
                StructureValue::Scalar {
                    value: value_type.decode(&bytes[..width])?,
                }
            }
            StructureFieldKind::Pointer { pointer_size } => StructureValue::Pointer {
                value: read_pointer(source, address, pointer_size)?,
            },
            StructureFieldKind::Bytes { size } => {
                if size == 0 || size > max_bytes_per_field {
                    return Err(AnalysisError::InvalidLimit("structure field bytes"));
                }
                let mut bytes = vec![0_u8; size];
                source.read_exact(native, &mut bytes)?;
                StructureValue::Bytes { value: bytes }
            }
        };
        values.push(StructureFieldValue {
            name: field.name.clone(),
            address,
            value,
        });
    }
    Ok(values)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedScan {
    pub name: String,
    pub session: ScanSession,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedWatchTemplate {
    pub name: String,
    pub address: String,
    pub value_type: ValueType,
    pub frozen: Option<ScalarValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedAnalysisSummary {
    pub scans: Vec<String>,
    pub watch_templates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisWorkspaceFile {
    pub version: u32,
    pub scans: Vec<SavedScan>,
    pub watch_templates: Vec<SavedWatchTemplate>,
}

#[derive(Debug, Default)]
pub struct AnalysisWorkspace {
    scans: HashMap<String, ScanSession>,
    watch_templates: HashMap<String, SavedWatchTemplate>,
}

impl AnalysisWorkspace {
    pub fn save_scan(&mut self, name: String, session: ScanSession) -> Result<(), AnalysisError> {
        validate_saved_name(&name)?;
        self.scans.insert(name, session);
        Ok(())
    }

    pub fn scan(&self, name: &str) -> Result<ScanSession, AnalysisError> {
        self.scans
            .get(name)
            .cloned()
            .ok_or_else(|| AnalysisError::SavedScanNotFound(name.to_owned()))
    }

    pub fn save_watch_template(
        &mut self,
        template: SavedWatchTemplate,
    ) -> Result<(), AnalysisError> {
        validate_saved_name(&template.name)?;
        AddressExpression::parse(&template.address)?;
        self.watch_templates.insert(template.name.clone(), template);
        Ok(())
    }

    pub fn watch_template(&self, name: &str) -> Result<SavedWatchTemplate, AnalysisError> {
        self.watch_templates
            .get(name)
            .cloned()
            .ok_or_else(|| AnalysisError::SavedWatchNotFound(name.to_owned()))
    }

    pub fn summary(&self) -> SavedAnalysisSummary {
        let mut scans: Vec<_> = self.scans.keys().cloned().collect();
        scans.sort_unstable();
        let mut watch_templates: Vec<_> = self.watch_templates.keys().cloned().collect();
        watch_templates.sort_unstable();
        SavedAnalysisSummary {
            scans,
            watch_templates,
        }
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), AnalysisError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| AnalysisError::WorkspaceIo {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut scans: Vec<_> = self
            .scans
            .iter()
            .map(|(name, session)| SavedScan {
                name: name.clone(),
                session: session.clone(),
            })
            .collect();
        scans.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        let mut watch_templates: Vec<_> = self.watch_templates.values().cloned().collect();
        watch_templates.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        let file = AnalysisWorkspaceFile {
            version: WORKSPACE_VERSION,
            scans,
            watch_templates,
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        fs::write(path, bytes).map_err(|source| AnalysisError::WorkspaceIo {
            path: path.to_path_buf(),
            source,
        })?;
        info!(path = %path.display(), "saved analysis workspace");
        Ok(())
    }

    pub fn load_from_path(&mut self, path: &Path) -> Result<(), AnalysisError> {
        let bytes = fs::read(path).map_err(|source| AnalysisError::WorkspaceIo {
            path: path.to_path_buf(),
            source,
        })?;
        let file: AnalysisWorkspaceFile = serde_json::from_slice(&bytes)?;
        if file.version != WORKSPACE_VERSION {
            return Err(AnalysisError::WorkspaceVersion {
                expected: WORKSPACE_VERSION,
                actual: file.version,
            });
        }
        let mut scans = HashMap::new();
        for saved in file.scans {
            validate_saved_name(&saved.name)?;
            scans.insert(saved.name, saved.session);
        }
        let mut watch_templates = HashMap::new();
        for template in file.watch_templates {
            validate_saved_name(&template.name)?;
            AddressExpression::parse(&template.address)?;
            watch_templates.insert(template.name.clone(), template);
        }
        self.scans = scans;
        self.watch_templates = watch_templates;
        info!(path = %path.display(), "loaded analysis workspace");
        Ok(())
    }
}

pub fn validate_workspace_name(name: &str) -> Result<(), AnalysisError> {
    validate_saved_name(name)
}

fn validate_saved_name(name: &str) -> Result<(), AnalysisError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed == "."
        || trimmed == ".."
    {
        return Err(AnalysisError::InvalidSavedName(name.to_owned()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "analysis", rename_all = "snake_case")]
pub enum AnalysisCommand {
    AobScan {
        pattern: String,
        alignment: usize,
        max_results: usize,
    },
    ResolveAddress {
        expression: String,
    },
    ResolvePointerChain {
        spec: PointerChainSpec,
    },
    SearchPointerChains {
        target: u64,
        options: PointerSearchOptions,
    },
    InspectStructure {
        base: String,
        fields: Vec<StructureFieldSpec>,
    },
    SaveScan {
        scan_id: u64,
        name: String,
    },
    RestoreScan {
        name: String,
    },
    SaveWatchTemplate {
        watch_id: u64,
        name: String,
    },
    AddWatchFromTemplate {
        name: String,
        label: Option<String>,
    },
    ListSaved,
    SaveWorkspace {
        name: String,
    },
    LoadWorkspace {
        name: String,
    },
    Batch {
        commands: Vec<AnalysisCommand>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "analysis_result", rename_all = "snake_case")]
pub enum AnalysisResult {
    PatternScan { scan: PatternScanResult },
    Address { expression: String, address: u64 },
    PointerChain { resolution: PointerChainResolution },
    PointerPaths { paths: Vec<PointerPath> },
    Structure { fields: Vec<StructureFieldValue> },
    ScanSaved { name: String },
    ScanRestored { name: String, scan_id: u64 },
    WatchTemplateSaved { name: String },
    WatchAdded { name: String, watch_id: u64 },
    Saved { summary: SavedAnalysisSummary },
    WorkspaceSaved { name: String },
    WorkspaceLoaded { name: String, summary: SavedAnalysisSummary },
    Batch { results: Vec<AnalysisResult> },
}

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("array-of-bytes pattern must contain at least one byte")]
    EmptyPattern,
    #[error("invalid array-of-bytes token {0:?}; use hex bytes or ? wildcards")]
    InvalidPatternToken(String),
    #[error("invalid address expression {0:?}")]
    InvalidAddressExpression(String),
    #[error("module {0:?} is not loaded")]
    ModuleNotFound(String),
    #[error("address arithmetic overflow")]
    AddressOverflow,
    #[error("pointer size must be 4 or 8 bytes, got {0}")]
    InvalidPointerSize(u8),
    #[error("invalid or out-of-range {0}")]
    InvalidLimit(&'static str),
    #[error("invalid structure field: {0}")]
    InvalidStructureField(&'static str),
    #[error("invalid saved analysis name {0:?}")]
    InvalidSavedName(String),
    #[error("saved scan {0:?} does not exist")]
    SavedScanNotFound(String),
    #[error("saved watch template {0:?} does not exist")]
    SavedWatchNotFound(String),
    #[error("analysis workspace version mismatch: expected {expected}, got {actual}")]
    WorkspaceVersion { expected: u32, actual: u32 },
    #[error("analysis workspace I/O failed for {path}: {source}")]
    WorkspaceIo {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Value(#[from] crate::scanner::ValueError),
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::memory::{MemoryRegion, MemorySource};

    struct Bytes {
        base: usize,
        bytes: Mutex<Vec<u8>>,
    }

    impl Bytes {
        fn new(base: usize, bytes: Vec<u8>) -> Self {
            Self {
                base,
                bytes: Mutex::new(bytes),
            }
        }
    }

    impl MemorySource for Bytes {
        fn regions(&self) -> Result<Vec<MemoryRegion>, MemoryError> {
            Ok(vec![MemoryRegion {
                base: self.base,
                size: self.bytes.lock().unwrap().len(),
                committed: true,
                readable: true,
                writable: true,
                executable: false,
                guard: false,
            }])
        }

        fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError> {
            let offset = address - self.base;
            let bytes = self.bytes.lock().unwrap();
            buffer.copy_from_slice(&bytes[offset..offset + buffer.len()]);
            Ok(())
        }
    }

    #[test]
    fn parses_full_and_nibble_wildcards() {
        let pattern = BytePattern::parse("48 8B ?? ?F A?").unwrap();
        assert_eq!(pattern.len(), 5);
        assert!(pattern.matches_at(&[0x48, 0x8B, 0x12, 0x3F, 0xA9], 0));
        assert!(!pattern.matches_at(&[0x48, 0x8B, 0x12, 0x30, 0xA9], 0));
    }

    #[test]
    fn pattern_scan_finds_matches_across_chunk_overlap() {
        let memory = Bytes::new(0x1000, vec![0x90, 0x48, 0x8B, 0x01, 0x90, 0x48, 0x8B, 0xFF]);
        let mut config = ScannerConfig::default();
        config.chunk_size_bytes = 4;
        let result = scan_pattern(
            &memory,
            "48 8B ??",
            &config,
            PatternScanOptions {
                alignment: 1,
                max_results: 16,
            },
        )
        .unwrap();
        assert_eq!(result.addresses, vec![0x1001, 0x1005]);
    }

    #[test]
    fn resolves_module_relative_expression_case_insensitively() {
        let modules = vec![ModuleDescriptor {
            name: "Game.exe".to_owned(),
            path: "C:\\Games\\Game.exe".to_owned(),
            base: 0x140000000,
            size: 0x100000,
        }];
        assert_eq!(
            AddressExpression::parse("game.exe+0x1234")
                .unwrap()
                .resolve(&modules)
                .unwrap(),
            0x140001234
        );
    }

    #[test]
    fn resolves_pointer_chain_using_little_endian_pointers() {
        let mut bytes = vec![0_u8; 0x80];
        bytes[0x00..0x08].copy_from_slice(&0x1020_u64.to_le_bytes());
        bytes[0x28..0x30].copy_from_slice(&0x1050_u64.to_le_bytes());
        let memory = Bytes::new(0x1000, bytes);
        let result = resolve_pointer_chain(
            &memory,
            &[],
            &PointerChainSpec {
                base: "0x1000".to_owned(),
                offsets: vec![8, 4],
                pointer_size: 8,
            },
        )
        .unwrap();
        assert_eq!(result.dereferenced, vec![0x1020, 0x1050]);
        assert_eq!(result.final_address, 0x1054);
    }

    #[test]
    fn pointer_search_finds_direct_predecessor() {
        let target = 0x5000_u64;
        let mut bytes = vec![0_u8; 0x40];
        bytes[0x10..0x18].copy_from_slice(&(target - 0x20).to_le_bytes());
        let memory = Bytes::new(0x2000, bytes);
        let paths = search_pointer_chains(
            &memory,
            target,
            &ScannerConfig::default(),
            PointerSearchOptions {
                max_depth: 1,
                max_offset: 0x100,
                pointer_size: 8,
                alignment: 8,
                max_results: 8,
            },
        )
        .unwrap();
        assert!(paths.iter().any(|path| path.root == 0x2010 && path.offsets == vec![0x20]));
    }
}
