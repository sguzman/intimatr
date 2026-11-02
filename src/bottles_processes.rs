//! Strictly read-only Linux /proc discovery for a particular Wine prefix.
//!
//! A process inheriting WINEPREFIX is *not* proof that it is a Wine game.
//! The Linux PID is not converted to a Windows PID, and no process is opened
//! for injection or marked attachable here.

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs::{self, File},
        io::Read,
        os::unix::fs::MetadataExt,
        path::{Path, PathBuf},
    };

    use serde::Serialize;

    const MAX_ENVIRON_BYTES: u64 = 256 * 1024;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct PrefixProcess {
        pub linux_pid: u32,
        /// Basename of /proc/PID/exe, if readable. Not a proven game identity.
        pub linux_binary: Option<String>,
        pub windows_pid: Option<u32>,
        pub windows_executable_verified: bool,
        pub safe_to_attach: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct PrefixProcessReport {
        pub operation: &'static str,
        pub canonical_prefix: PathBuf,
        pub inspected_owned_pids: usize,
        pub inaccessible_environments: usize,
        pub missing_prefix_environments: usize,
        pub ambiguous_prefix_environments: usize,
        pub processes: Vec<PrefixProcess>,
        pub attachment_attempted: bool,
    }

    enum PrefixEnvironment {
        Matched,
        Other,
        Missing,
        Ambiguous,
    }

    fn classify_environ(bytes: &[u8], prefix: &Path) -> PrefixEnvironment {
        let mut values = bytes
            .split(|value| *value == 0)
            .filter_map(|part| part.strip_prefix(b"WINEPREFIX="));
        let Some(value) = values.next() else {
            return PrefixEnvironment::Missing;
        };
        if values.next().is_some() {
            return PrefixEnvironment::Ambiguous;
        }
        let Ok(path) = std::str::from_utf8(value) else {
            return PrefixEnvironment::Ambiguous;
        };
        let path = Path::new(path);
        if !path.is_absolute() {
            return PrefixEnvironment::Ambiguous;
        }
        let Ok(canonical) = fs::canonicalize(path) else {
            return PrefixEnvironment::Ambiguous;
        };
        if canonical == prefix {
            PrefixEnvironment::Matched
        } else {
            PrefixEnvironment::Other
        }
    }

    fn limited_environ(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
        let file = File::open(path)?;
        let mut bytes = Vec::new();
        file.take(MAX_ENVIRON_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_ENVIRON_BYTES {
            Ok(None)
        } else {
            Ok(Some(bytes))
        }
    }

    /// A snapshot, not a PID reservation; PIDs may exit or be recycled.
    pub fn inspect_prefix_processes(
        prefix: &Path,
        proc_root: &Path,
    ) -> Result<PrefixProcessReport, String> {
        let prefix = fs::canonicalize(prefix)
            .map_err(|error| format!("cannot resolve requested Wine prefix: {error}"))?;
        if !prefix.join("drive_c").is_dir() {
            return Err("Wine prefix is not initialized (missing drive_c)".to_owned());
        }
        let user_uid = fs::metadata(proc_root.join("self"))
            .or_else(|_| fs::metadata(proc_root))
            .map_err(|error| format!("cannot inspect proc root: {error}"))?
            .uid();

        let mut report = PrefixProcessReport {
            operation: "read_only_prefix_process_inspection",
            canonical_prefix: prefix.clone(),
            inspected_owned_pids: 0,
            inaccessible_environments: 0,
            missing_prefix_environments: 0,
            ambiguous_prefix_environments: 0,
            processes: Vec::new(),
            attachment_attempted: false,
        };
        let entries = fs::read_dir(proc_root)
            .map_err(|error| format!("cannot enumerate proc root: {error}"))?;
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == 0 {
                continue;
            }
            let directory = entry.path();
            let Ok(metadata) = fs::metadata(&directory) else {
                continue;
            };
            if !metadata.is_dir() || metadata.uid() != user_uid {
                continue;
            }
            report.inspected_owned_pids += 1;
            let environ = match limited_environ(&directory.join("environ")) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => {
                    report.ambiguous_prefix_environments += 1;
                    continue;
                }
                Err(_) => {
                    report.inaccessible_environments += 1;
                    continue;
                }
            };
            match classify_environ(&environ, &prefix) {
                PrefixEnvironment::Missing => report.missing_prefix_environments += 1,
                PrefixEnvironment::Ambiguous => report.ambiguous_prefix_environments += 1,
                PrefixEnvironment::Other => {}
                PrefixEnvironment::Matched => {
                    let linux_binary = fs::read_link(directory.join("exe"))
                        .ok()
                        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()));
                    report.processes.push(PrefixProcess {
                        linux_pid: pid,
                        linux_binary,
                        windows_pid: None,
                        windows_executable_verified: false,
                        safe_to_attach: false,
                    });
                }
            }
        }
        report.processes.sort_by_key(|process| process.linux_pid);
        Ok(report)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        struct Fixture(PathBuf);

        impl Fixture {
            fn new() -> Self {
                let id = COUNTER.fetch_add(1, Ordering::Relaxed);
                let root = std::env::temp_dir().join(format!(
                    "intimatr-proc-test-{}-{id}",
                    std::process::id()
                ));
                fs::create_dir_all(&root).unwrap();
                Self(root)
            }
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).ok();
            }
        }

        #[test]
        fn rejects_missing_and_ambiguous_prefix_environment() {
            let fixture = Fixture::new();
            let prefix = fixture.0.join("bottle");
            fs::create_dir_all(&prefix).unwrap();
            assert!(matches!(
                classify_environ(b"OTHER=1\0", &prefix),
                PrefixEnvironment::Missing
            ));
            assert!(matches!(
                classify_environ(b"WINEPREFIX=relative\0", &prefix),
                PrefixEnvironment::Ambiguous
            ));
            let repeated = format!(
                "WINEPREFIX={}\0WINEPREFIX={}\0",
                prefix.display(),
                prefix.display()
            );
            assert!(matches!(
                classify_environ(repeated.as_bytes(), &prefix),
                PrefixEnvironment::Ambiguous
            ));
        }

        #[test]
        fn only_reports_matching_owned_linux_pids_not_attachable_windows_targets() {
            let fixture = Fixture::new();
            let prefix = fixture.0.join("bottle");
            let other = fixture.0.join("different");
            let proc_root = fixture.0.join("proc");
            fs::create_dir_all(prefix.join("drive_c")).unwrap();
            fs::create_dir_all(other.join("drive_c")).unwrap();
            for pid in [1001, 1002, 1003] {
                fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
            }
            fs::write(
                proc_root.join("1001/environ"),
                format!("WINEPREFIX={}\0", prefix.display()),
            )
            .unwrap();
            fs::write(
                proc_root.join("1002/environ"),
                format!("WINEPREFIX={}\0", other.display()),
            )
            .unwrap();
            fs::write(proc_root.join("1003/environ"), b"WINEPREFIX=relative\0").unwrap();

            let report = inspect_prefix_processes(&prefix, &proc_root).unwrap();
            assert_eq!(report.operation, "read_only_prefix_process_inspection");
            assert_eq!(report.processes.len(), 1);
            assert_eq!(report.processes[0].linux_pid, 1001);
            assert_eq!(report.processes[0].windows_pid, None);
            assert!(!report.processes[0].safe_to_attach);
            assert!(!report.attachment_attempted);
            assert_eq!(report.ambiguous_prefix_environments, 1);
        }

        #[test]
        fn missing_initialized_prefix_fails_closed() {
            let fixture = Fixture::new();
            let prefix = fixture.0.join("bottle");
            fs::create_dir_all(&prefix).unwrap();
            assert!(inspect_prefix_processes(&prefix, &fixture.0).is_err());
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{PrefixProcess, PrefixProcessReport, inspect_prefix_processes};
