//! Bounded static PE import-table inspection for the experimental four-clock
//! IAT backend. Name coverage is NOT proof that Wine resolves these entries to
//! the same functions or that installing hooks in a game is safe.
//!
//! Read-only: no Wine process handles, injection, or registry modifications.

use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const MAX_SECTIONS: usize = 96;
const MAX_IMPORT_DESCRIPTORS: usize = 256;
const MAX_IMPORT_THUNKS: usize = 8192;
const MAX_NAME: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct TimerImportCoverage {
    pub query_performance_counter: bool,
    pub get_tick_count64: bool,
    pub get_tick_count: bool,
    pub time_get_time: bool,
    /// All four symbol *names* occur in the main executable's static IAT.
    /// Actual runtime hook eligibility remains a separate DLL-side check.
    pub all_four_static_names: bool,
}

impl TimerImportCoverage {
    fn record(&mut self, name: &[u8]) {
        match name {
            b"QueryPerformanceCounter" => self.query_performance_counter = true,
            b"GetTickCount64" => self.get_tick_count64 = true,
            b"GetTickCount" => self.get_tick_count = true,
            b"timeGetTime" => self.time_get_time = true,
            _ => {}
        }
    }

    fn finish(&mut self) {
        self.all_four_static_names = self.query_performance_counter
            && self.get_tick_count64
            && self.get_tick_count
            && self.time_get_time;
    }
}

#[derive(Clone, Copy)]
struct Section {
    rva: u32,
    raw_offset: u32,
    raw_size: u32,
    virtual_size: u32,
}

struct Reader {
    file: File,
    size: u64,
    headers_size: u32,
    sections: Vec<Section>,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

impl Reader {
    fn read(&mut self, offset: u64, count: usize) -> Result<Vec<u8>, String> {
        if count > 4096
            || offset
                .checked_add(count as u64)
                .is_none_or(|end| end > self.size)
        {
            return Err("PE read crosses its bounded file range".into());
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|error| format!("cannot seek PE data: {error}"))?;
        let mut bytes = vec![0_u8; count];
        self.file
            .read_exact(&mut bytes)
            .map_err(|error| format!("cannot read PE data: {error}"))?;
        Ok(bytes)
    }

    /// Translate an RVA to an on-disk offset and remaining raw bytes.
    fn locate(&self, rva: u32) -> Result<(u64, usize), String> {
        if rva < self.headers_size && (rva as u64) < self.size {
            let remaining = (self.headers_size - rva) as u64;
            return Ok((rva as u64, remaining.min(self.size - rva as u64) as usize));
        }
        for section in &self.sections {
            let start = section.rva;
            let extent = section.virtual_size.max(section.raw_size);
            if rva >= start && rva - start < extent {
                let inside = rva - start;
                if inside >= section.raw_size {
                    return Err("PE RVA addresses zero-filled, not on-disk import data".into());
                }
                let offset = u64::from(section.raw_offset) + u64::from(inside);
                if offset >= self.size {
                    return Err("PE section points beyond file".into());
                }
                let remaining = u64::from(section.raw_size - inside).min(self.size - offset);
                return Ok((offset, remaining as usize));
            }
        }
        Err("PE import RVA does not belong to a file-backed section".into())
    }

    fn bytes_at_rva(&mut self, rva: u32, count: usize) -> Result<Vec<u8>, String> {
        let (offset, remaining) = self.locate(rva)?;
        if count > remaining {
            return Err("PE import read crosses section boundary".into());
        }
        self.read(offset, count)
    }

    fn cstr_at_rva(&mut self, rva: u32) -> Result<Vec<u8>, String> {
        let (offset, remaining) = self.locate(rva)?;
        let bytes = self.read(offset, remaining.min(MAX_NAME))?;
        let end = bytes
            .iter()
            .position(|value| *value == 0)
            .ok_or("unterminated PE import name")?;
        Ok(bytes[..end].to_vec())
    }
}

/// Inspect static x64 PE32+ imports. An incomplete or malformed table yields
/// an error (unknown coverage), never a fabricated set of missing imports.
pub fn inspect_pe_timer_imports(path: &Path) -> Result<TimerImportCoverage, String> {
    let file = File::open(path).map_err(|error| format!("cannot open PE imports: {error}"))?;
    let size = file
        .metadata()
        .map_err(|error| format!("cannot stat PE imports: {error}"))?
        .len();
    let mut reader = Reader {
        file,
        size,
        headers_size: 0,
        sections: Vec::new(),
    };
    let dos = reader.read(0, 64)?;
    if &dos[..2] != b"MZ" {
        return Err("missing PE MZ signature".into());
    }
    let pe_offset = u64::from(u32_at(&dos, 60));
    if !(64..=1024 * 1024).contains(&pe_offset) {
        return Err("PE header offset outside bounded range".into());
    }
    let coff = reader.read(pe_offset, 24)?;
    if &coff[..4] != b"PE\0\0" || u16_at(&coff, 4) != 0x8664 {
        return Err("timer import preflight requires an x86-64 PE".into());
    }
    let section_count = usize::from(u16_at(&coff, 6));
    let optional_size = usize::from(u16_at(&coff, 20));
    if section_count == 0 || section_count > MAX_SECTIONS || !(128..=4096).contains(&optional_size)
    {
        return Err("PE has unsupported section or optional header count".into());
    }
    let optional = reader.read(pe_offset + 24, optional_size)?;
    if u16_at(&optional, 0) != 0x020b {
        return Err("timer import preflight requires a PE32+ image".into());
    }
    reader.headers_size = u32_at(&optional, 60);
    let import_rva = u32_at(&optional, 120);
    let import_size = u32_at(&optional, 124);
    let section_offset = pe_offset + 24 + optional_size as u64;
    let section_bytes = reader.read(section_offset, section_count * 40)?;
    for section in section_bytes.chunks_exact(40) {
        reader.sections.push(Section {
            virtual_size: u32_at(section, 8),
            rva: u32_at(section, 12),
            raw_size: u32_at(section, 16),
            raw_offset: u32_at(section, 20),
        });
    }
    if import_rva == 0 && import_size == 0 {
        return Ok(TimerImportCoverage::default());
    }
    if import_rva == 0 || import_size < 20 {
        return Err("PE import data directory is incomplete".into());
    }
    let mut coverage = TimerImportCoverage::default();
    let count = usize::try_from(import_size / 20)
        .unwrap_or(0)
        .min(MAX_IMPORT_DESCRIPTORS);
    let mut terminated = false;
    for index in 0..count {
        let offset = (index as u32)
            .checked_mul(20)
            .ok_or("PE import descriptor overflow")?;
        let rva = import_rva
            .checked_add(offset)
            .ok_or("PE import RVA overflow")?;
        let desc = reader.bytes_at_rva(rva, 20)?;
        if desc.iter().all(|byte| *byte == 0) {
            terminated = true;
            break;
        }
        let name_rva = u32_at(&desc, 12);
        let original_thunk = u32_at(&desc, 0);
        let first_thunk = u32_at(&desc, 16);
        if name_rva == 0 || first_thunk == 0 {
            return Err("PE import descriptor lacks a DLL or IAT".into());
        }
        let dll = reader.cstr_at_rva(name_rva)?;
        // Exact names from supported system DLLs; not a runtime address test.
        let dll = String::from_utf8_lossy(&dll).to_ascii_lowercase();
        let timer_dll = matches!(
            dll.as_str(),
            "kernel32.dll" | "kernelbase.dll" | "winmm.dll"
        ) || dll.starts_with("api-ms-win-core-");
        let thunk_rva = if original_thunk == 0 {
            first_thunk
        } else {
            original_thunk
        };
        let mut thunk_terminated = false;
        for thunk_index in 0..MAX_IMPORT_THUNKS {
            let rva = thunk_rva
                .checked_add((thunk_index as u32) * 8)
                .ok_or("PE thunk RVA overflow")?;
            let thunk = u64_at(&reader.bytes_at_rva(rva, 8)?, 0);
            if thunk == 0 {
                thunk_terminated = true;
                break;
            }
            if thunk & (1_u64 << 63) != 0 {
                continue; // Imported by ordinal: cannot infer a symbol name.
            }
            if thunk > u64::from(u32::MAX) {
                return Err("PE import thunk has an invalid name RVA".into());
            }
            if timer_dll {
                let name = reader.cstr_at_rva(
                    (thunk as u32)
                        .checked_add(2)
                        .ok_or("PE import name RVA overflow")?,
                )?;
                coverage.record(&name);
            }
        }
        if !thunk_terminated {
            return Err("PE import thunk table exceeded bounded range".into());
        }
    }
    if !terminated {
        return Err("PE import descriptor table has no terminator".into());
    }
    coverage.finish();
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn fixture() -> Vec<u8> {
        let mut pe = vec![0_u8; 0x800];
        pe[..2].copy_from_slice(b"MZ");
        pe[60..64].copy_from_slice(&0x80_u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        pe[0x86..0x88].copy_from_slice(&1_u16.to_le_bytes());
        pe[0x94..0x96].copy_from_slice(&0xf0_u16.to_le_bytes());
        pe[0x98..0x9a].copy_from_slice(&0x020b_u16.to_le_bytes());
        pe[0x98 + 60..0x98 + 64].copy_from_slice(&0x200_u32.to_le_bytes());
        pe[0x98 + 120..0x98 + 124].copy_from_slice(&0x1000_u32.to_le_bytes());
        pe[0x98 + 124..0x98 + 128].copy_from_slice(&0x40_u32.to_le_bytes());
        let section = 0x98 + 0xf0;
        pe[section + 8..section + 12].copy_from_slice(&0x400_u32.to_le_bytes());
        pe[section + 12..section + 16].copy_from_slice(&0x1000_u32.to_le_bytes());
        pe[section + 16..section + 20].copy_from_slice(&0x400_u32.to_le_bytes());
        pe[section + 20..section + 24].copy_from_slice(&0x400_u32.to_le_bytes());
        // Two descriptors and one zero terminator.
        pe[0x400..0x404].copy_from_slice(&0x1080_u32.to_le_bytes());
        pe[0x40c..0x410].copy_from_slice(&0x1060_u32.to_le_bytes());
        pe[0x410..0x414].copy_from_slice(&0x1080_u32.to_le_bytes());
        pe[0x414..0x418].copy_from_slice(&0x10c0_u32.to_le_bytes());
        pe[0x420..0x424].copy_from_slice(&0x1070_u32.to_le_bytes());
        pe[0x424..0x428].copy_from_slice(&0x10c0_u32.to_le_bytes());
        pe[0x460..0x46d].copy_from_slice(b"KERNEL32.dll\0");
        pe[0x470..0x47a].copy_from_slice(b"winmm.dll\0");
        for (i, rva) in [0x1100_u64, 0x1130, 0x1160].iter().enumerate() {
            pe[0x480 + 8 * i..0x488 + 8 * i].copy_from_slice(&rva.to_le_bytes());
        }
        pe[0x4c0..0x4c8].copy_from_slice(&0x1190_u64.to_le_bytes());
        for (offset, name) in [
            (0x502, b"QueryPerformanceCounter\0".as_slice()),
            (0x532, b"GetTickCount64\0".as_slice()),
            (0x562, b"GetTickCount\0".as_slice()),
            (0x592, b"timeGetTime\0".as_slice()),
        ] {
            pe[offset..offset + name.len()].copy_from_slice(name);
        }
        pe
    }

    fn with_file(bytes: &[u8], test: impl FnOnce(&Path)) {
        let path = std::env::temp_dir().join(format!(
            "intimatr-pe-timers-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).unwrap();
        test(&path);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn all_four_static_symbols_are_reported_without_claiming_runtime_hooks() {
        with_file(&fixture(), |path| {
            let coverage = inspect_pe_timer_imports(path).unwrap();
            assert!(coverage.all_four_static_names);
            assert!(coverage.query_performance_counter);
            assert!(coverage.get_tick_count64);
            assert!(coverage.get_tick_count);
            assert!(coverage.time_get_time);
        });
    }

    #[test]
    fn missing_symbol_and_invalid_directory_are_distinct() {
        let mut missing = fixture();
        missing[0x592..0x592 + 11].fill(0);
        with_file(&missing, |path| {
            let coverage = inspect_pe_timer_imports(path).unwrap();
            assert!(!coverage.all_four_static_names);
            assert!(!coverage.time_get_time);
        });
        let mut invalid = fixture();
        invalid[0x498..0x49c].copy_from_slice(&0xffff_ffff_u32.to_le_bytes());
        with_file(&invalid, |path| {
            assert!(inspect_pe_timer_imports(path).is_err())
        });
    }

    #[test]
    fn no_import_directory_is_known_absent_not_an_error() {
        let mut empty = fixture();
        empty[0x98 + 120..0x98 + 128].fill(0);
        with_file(&empty, |path| {
            let coverage = inspect_pe_timer_imports(path).unwrap();
            assert!(!coverage.all_four_static_names);
        });
    }
}
