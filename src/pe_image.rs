//! Bounded read-only PE header inspection.
//! Identifies the executable *file*, not the live process or Wine runner.

use serde::Serialize;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PeIdentity {
    pub architecture: &'static str,
    pub machine: u16,
}

pub fn inspect_pe(path: &Path) -> Result<PeIdentity, String> {
    let mut file = File::open(path).map_err(|error| format!("cannot open PE: {error}"))?;
    let mut dos = [0_u8; 64];
    file.read_exact(&mut dos)
        .map_err(|error| format!("incomplete DOS header: {error}"))?;
    if &dos[..2] != b"MZ" {
        return Err("missing DOS MZ signature".into());
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if !(64..=1024 * 1024).contains(&offset) {
        return Err("PE header offset outside bounded inspection range".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("cannot seek to PE header: {error}"))?;
    let mut coff = [0_u8; 26];
    file.read_exact(&mut coff)
        .map_err(|error| format!("incomplete PE signature and COFF header: {error}"))?;
    if &coff[..4] != b"PE\0\0" {
        return Err("missing PE signature".into());
    }
    let machine = u16::from_le_bytes([coff[4], coff[5]]);
    let magic = u16::from_le_bytes([coff[24], coff[25]]);
    let architecture = match (machine, magic) {
        (0x014c, 0x010b) => "x86",
        (0x8664, 0x020b) => "x86_64",
        (0xaa64, 0x020b) => "aarch64",
        _ => return Err("unknown or inconsistent PE machine and optional header".into()),
    };
    Ok(PeIdentity {
        architecture,
        machine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn make(machine: u16, magic: u16) -> Vec<u8> {
        let mut image = vec![0_u8; 256];
        image[..2].copy_from_slice(b"MZ");
        image[60..64].copy_from_slice(&128_u32.to_le_bytes());
        image[128..132].copy_from_slice(b"PE\0\0");
        image[132..134].copy_from_slice(&machine.to_le_bytes());
        image[152..154].copy_from_slice(&magic.to_le_bytes());
        image
    }
    fn with_file(image: &[u8], check: impl FnOnce(&Path)) {
        let path = std::env::temp_dir().join(format!(
            "intimatr-pe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, image).unwrap();
        check(&path);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn reads_x86_x64_arm64_headers() {
        for (machine, magic, arch) in [
            (0x014c, 0x010b, "x86"),
            (0x8664, 0x020b, "x86_64"),
            (0xaa64, 0x020b, "aarch64"),
        ] {
            with_file(&make(machine, magic), |path| {
                let identity = inspect_pe(path).unwrap();
                assert_eq!(identity.machine, machine);
                assert_eq!(identity.architecture, arch);
            });
        }
    }
    #[test]
    fn refuses_bad_or_mismatched_images() {
        let mut wrong_signature = make(0x8664, 0x020b);
        wrong_signature[128..132].copy_from_slice(b"NOPE");
        let mut large_offset = make(0x8664, 0x020b);
        large_offset[60..64].copy_from_slice(&0x1fffff_u32.to_le_bytes());
        for invalid in [
            vec![],
            b"MZ".to_vec(),
            make(0x014c, 0x020b),
            make(0x8664, 0x010b),
            wrong_signature,
            large_offset,
        ] {
            with_file(&invalid, |path| assert!(inspect_pe(path).is_err()));
        }
    }
}
