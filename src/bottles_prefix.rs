//! Read-only, fail-closed ownership check for a native Bottles prefix.
//!
//! Bottles exposes its configured bottles directory via
//! \`bottles-cli info bottles-path\`. The bottle directory must be a direct,
//! non-symlinked child with the exact selected name and an initialized prefix.
//! This verifies storage location only, not that a Wine process is safe to hook.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BottlePrefixIdentity {
    pub bottle: String,
    pub canonical_bottles_root: PathBuf,
    pub canonical_bottle_prefix: PathBuf,
    pub bottle_prefix_association_verified: bool,
    pub game_process_identity_verified: bool,
    pub runner_verified: bool,
    pub safe_to_attach: bool,
}

/// The Bottles CLI reports this value as a plain path, even in --json mode.
/// Do not trim spaces: spaces can be part of a valid directory name.
pub fn parse_bottles_root(output: &str) -> Result<&Path, String> {
    let trimmed = output.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty()
        || trimmed.contains('\0')
        || trimmed.contains('\r')
        || trimmed.contains('\n')
        || !Path::new(trimmed).is_absolute()
    {
        return Err("Bottles did not provide a single absolute bottles-path".into());
    }
    Ok(Path::new(trimmed))
}

/// Require an explicitly supplied prefix to equal Bottles' root/name path.
/// No Wine executable is launched and no game files are changed.
pub fn verify_bottle_prefix(
    bottles_root_output: &str,
    selected_bottle: &str,
    supplied_prefix: &Path,
) -> Result<BottlePrefixIdentity, String> {
    let candidate_name = Path::new(selected_bottle);
    if selected_bottle.is_empty()
        || selected_bottle == "."
        || selected_bottle == ".."
        || selected_bottle.contains('\\')
        || selected_bottle.chars().any(char::is_control)
        || !matches!(candidate_name.components().next(), Some(Component::Normal(_)))
        || candidate_name.components().count() != 1
    {
        return Err("invalid bottle name: expected one literal directory component".into());
    }
    let root = fs::canonicalize(parse_bottles_root(bottles_root_output)?)
        .map_err(|error| format!("Bottles root not accessible: {error}"))?;
    if !root.is_dir() {
        return Err("configured Bottles root is not a directory".into());
    }
    let candidate = root.join(selected_bottle);
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| format!("selected bottle directory does not exist: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("selected bottle is not a plain directory under Bottles root".into());
    }
    let canonical = fs::canonicalize(&candidate)
        .map_err(|error| format!("cannot resolve selected bottle directory: {error}"))?;
    if canonical.parent() != Some(root.as_path())
        || canonical.file_name() != Some(std::ffi::OsStr::new(selected_bottle))
    {
        return Err("Bottles directory does not resolve to an exact child of configured root".into());
    }
    if !canonical.join("drive_c").is_dir()
        || !canonical.join("bottle.yml").is_file()
    {
        return Err("selected bottle is missing its drive_c or bottle.yml marker".into());
    }
    let selected = fs::canonicalize(supplied_prefix)
        .map_err(|error| format!("cannot resolve supplied Wine prefix: {error}"))?;
    if selected != canonical {
        return Err("supplied WINEPREFIX is not the selected bottle's configured prefix".into());
    }
    Ok(BottlePrefixIdentity {
        bottle: selected_bottle.to_owned(),
        canonical_bottles_root: root,
        canonical_bottle_prefix: canonical,
        bottle_prefix_association_verified: true,
        game_process_identity_verified: false,
        runner_verified: false,
        safe_to_attach: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let n = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("intimatr-bottles-prefix-{}-{n}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn create_bottle(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.join("drive_c")).unwrap();
            fs::write(path.join("bottle.yml"), "Name: Fixture\n").unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn recognizes_exact_bottle_even_when_root_contains_spaces() {
        let fixture = Fixture::new();
        let prefix = fixture.create_bottle("My Games");
        let result = verify_bottle_prefix(
            &format!("{}\n", fixture.0.display()),
            "My Games",
            &prefix,
        )
        .unwrap();
        assert!(result.bottle_prefix_association_verified);
        assert!(!result.runner_verified);
        assert!(!result.game_process_identity_verified);
        assert!(!result.safe_to_attach);
    }

    #[test]
    fn refuses_wrong_prefix_bottle_names_and_missing_marker() {
        let fixture = Fixture::new();
        let primary = fixture.create_bottle("Primary");
        let other = fixture.create_bottle("Other");
        let root = fixture.0.to_str().unwrap();
        assert!(verify_bottle_prefix(root, "Primary", &other).is_err());
        for name in ["", ".", "..", "../Primary", "Primary/..", r"Other\bad"] {
            assert!(verify_bottle_prefix(root, name, &primary).is_err());
        }
        fs::remove_file(primary.join("bottle.yml")).unwrap();
        assert!(verify_bottle_prefix(root, "Primary", &primary).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_bottle_directory_even_if_prefix_resolves() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let target = fixture.create_bottle("Real");
        symlink(&target, fixture.0.join("Alias")).unwrap();
        assert!(
            verify_bottle_prefix(fixture.0.to_str().unwrap(), "Alias", &target).is_err()
        );
    }

    #[test]
    fn rejects_non_absolute_ambiguous_and_multiline_cli_roots() {
        for bad in ["", "relative/path", "  ", "/tmp/one\n/tmp/two", "/tmp/a\0b"] {
            assert!(parse_bottles_root(bad).is_err());
        }
        assert_eq!(parse_bottles_root("/tmp/bottles with spaces\n").unwrap(),
            Path::new("/tmp/bottles with spaces"));
    }
}
