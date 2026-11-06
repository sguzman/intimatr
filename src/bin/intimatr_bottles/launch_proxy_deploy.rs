//! Explicit, reversible startup proxy for selected offline Bottles games.
//! Legacy version-1 Eastward deployments remain readable and removable.
//! Never auto-selects games, invokes Wine, or touches a running Windows process.
//! The original Steam API DLL is preserved byte-for-byte under a unique name.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use intimatr::{
    config::AppConfig,
    speed_profile_fingerprint::{sha256_hex, sha256_words},
};
use serde::{Deserialize, Serialize};

const ORIGINAL: &str = "steam_api64.dll";
const SAVED: &str = "steam_api64_intimatr_original.dll";
const RUNTIME: &str = "intimatr.dll";
const CONFIG: &str = "Eastward.exe.toml";
const STATE: &str = "intimatr-launch-deployment.json";
const QUARANTINE: &str = "steam_api64.intimatr-removing.dll";
const MAX_FILE: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Deployment {
    version: u32,
    original_sha256: String,
    proxy_sha256: String,
    runtime_sha256: String,
    config_sha256: String,
    #[serde(default)]
    executable: Option<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    operation: &'static str,
    game_directory: PathBuf,
    status: &'static str,
    original_preserved: bool,
    launcher_changed: bool,
    game_process_touched: bool,
    timer_hooks_activated: bool,
}

fn digest(bytes: &[u8]) -> String {
    sha256_hex(&sha256_words(bytes))
}

fn present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("refusing symbolic link: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(format!("not a regular file: {}", path.display()))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_absolute() || !present(path)? {
        return Err(format!(
            "expected existing absolute regular file: {}",
            path.display()
        ));
    }
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.len() == 0 || metadata.len() > MAX_FILE {
        return Err(format!(
            "file exceeds size policy or is empty: {}",
            path.display()
        ));
    }
    fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn same_file(path: &Path, expected: &str) -> Result<bool, String> {
    Ok(present(path)? && digest(&read_file(path)?) == expected)
}

fn require_absent(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(format!("refusing existing path: {}", path.display()));
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn valid_executable(executable: &str) -> bool {
    executable.len() > 4
        && executable.len() <= 128
        && executable.to_ascii_lowercase().ends_with(".exe")
        && !executable.starts_with('.')
        && !executable.contains("..")
        && executable
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
}

fn config_name(executable: &str) -> String {
    format!("{executable}.toml")
}

fn state_executable(state: &Deployment) -> Result<&str, String> {
    if state.version == 1 && state.executable.is_none() {
        return Ok("Eastward.exe");
    }
    if state.version != 2 {
        return Err("unsupported deployment marker version".into());
    }
    let executable = state
        .executable
        .as_deref()
        .ok_or("deployment marker lacks executable identity")?;
    if !valid_executable(executable) {
        return Err("invalid executable identity in deployment marker".into());
    }
    Ok(executable)
}

fn verify_game_dir(raw: &Path) -> Result<PathBuf, String> {
    if !raw.is_absolute() {
        return Err("--game-dir requires an absolute Linux path".into());
    }
    if fs::symlink_metadata(raw)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("game directory must not be a symlink".into());
    }
    let game = fs::canonicalize(raw).map_err(|e| e.to_string())?;
    if !game.is_dir() {
        return Err("game directory must be a directory".into());
    }
    let drive = game
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "drive_c"))
        .ok_or("game directory must be inside a bottle's drive_c")?;
    if drive == game {
        return Err("game directory must be below drive_c".into());
    }
    let prefix = drive
        .parent()
        .ok_or("cannot resolve selected bottle prefix")?;
    if !present(&prefix.join("bottle.yml"))? || !drive.is_dir() {
        return Err("selected directory is not inside an initialized Bottles prefix".into());
    }
    let canonical_drive = fs::canonicalize(drive).map_err(|e| e.to_string())?;
    if canonical_drive != drive {
        return Err("selected game drive_c cannot use a symlink".into());
    }
    Ok(game)
}

fn check_pe64_dll(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return Err("DLL source is not a DOS/PE image".into());
    }
    let offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    let header = bytes
        .get(offset..offset.saturating_add(24))
        .ok_or("DLL PE header extends beyond file")?;
    if &header[..4] != b"PE\0\0"
        || header[4..6] != [0x64, 0x86]
        || (u16::from_le_bytes(header[22..24].try_into().unwrap()) & 0x2000) == 0
    {
        return Err("expected 64-bit x86 Windows PE DLL".into());
    }
    Ok(())
}

fn state_path(game: &Path) -> PathBuf {
    game.join(STATE)
}

fn load_state(game: &Path) -> Result<Deployment, String> {
    let bytes = read_file(&state_path(game))?;
    if bytes.len() > 4096 {
        return Err("deployment marker exceeds limit".into());
    }
    let state: Deployment =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid deployment marker: {e}"))?;
    state_executable(&state)?;
    Ok(state)
}

fn check_installed(game: &Path) -> Result<Deployment, String> {
    let state = load_state(game)?;
    let cfg = config_name(state_executable(&state)?);
    for (path, hash) in [
        (game.join(ORIGINAL), &state.proxy_sha256),
        (game.join(SAVED), &state.original_sha256),
        (game.join(RUNTIME), &state.runtime_sha256),
        (game.join("config").join(&cfg), &state.config_sha256),
    ] {
        if !same_file(&path, hash)? {
            return Err(format!(
                "installed file changed or missing; refusing mutation: {}",
                path.display()
            ));
        }
    }
    if fs::symlink_metadata(game.join(QUARANTINE)).is_ok() {
        return Err("interrupted removal state found; refusing mutation".into());
    }
    Ok(state)
}

fn print_report(
    operation: &'static str,
    game: &Path,
    status: &'static str,
    changed: bool,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&Report {
        operation,
        game_directory: game.to_path_buf(),
        status,
        original_preserved: status == "installed",
        launcher_changed: changed,
        game_process_touched: false,
        timer_hooks_activated: false,
    })
    .map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

fn status(game: &Path) -> Result<(), String> {
    if present(&state_path(game))? {
        check_installed(game)?;
        return print_report("read_only_launch_proxy_status", game, "installed", false);
    }
    if !present(&game.join(ORIGINAL))?
        || present(&game.join(SAVED))?
        || present(&game.join(RUNTIME))?
        || present(&game.join("config").join(CONFIG))?
        || fs::symlink_metadata(game.join(QUARANTINE)).is_ok()
    {
        return Err("unmanaged or incomplete deployment files found; no automatic cleanup".into());
    }
    print_report(
        "read_only_launch_proxy_status",
        game,
        "not_installed",
        false,
    )
}

fn install(
    game: &Path,
    proxy: &Path,
    runtime: &Path,
    config: &Path,
    executable: &str,
    expected_original_sha256: Option<&str>,
) -> Result<(), String> {
    if !valid_executable(executable) || !present(&game.join(executable))? {
        return Err("selected game executable name is unsafe or missing".into());
    }
    status_preflight(game, executable)?;
    let original = read_file(&game.join(ORIGINAL))?;
    let proxy_bytes = read_file(proxy)?;
    let runtime_bytes = read_file(runtime)?;
    let config_bytes = read_file(config)?;
    check_pe64_dll(&original)?;
    if let Some(expected) = expected_original_sha256 {
        if expected.len() != 64
            || !expected.bytes().all(|b| b.is_ascii_hexdigit())
            || digest(&original) != expected.to_ascii_lowercase()
        {
            return Err("original game DLL SHA-256 changed or is invalid; refusing install".into());
        }
    }
    check_pe64_dll(&proxy_bytes)?;
    check_pe64_dll(&runtime_bytes)?;
    let config_string =
        std::str::from_utf8(&config_bytes).map_err(|_| "launch config is not UTF-8")?;
    let parsed = AppConfig::from_toml_str(config_string)
        .map_err(|e| format!("invalid launch configuration: {e}"))?;
    if parsed.target.executable != executable {
        return Err("launch config does not target the selected executable exactly".into());
    }

    let state = Deployment {
        version: 2,
        original_sha256: digest(&original),
        proxy_sha256: digest(&proxy_bytes),
        runtime_sha256: digest(&runtime_bytes),
        config_sha256: digest(&config_bytes),
        executable: Some(executable.to_owned()),
    };
    let cfg_dir = game.join("config");
    let created_config_dir = if cfg_dir.exists() {
        if !cfg_dir.is_dir()
            || fs::symlink_metadata(&cfg_dir)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err("refusing non-directory or symlinked config directory".into());
        }
        false
    } else {
        fs::create_dir(&cfg_dir)
            .map_err(|e| format!("cannot create game config directory: {e}"))?;
        true
    };
    let staged_proxy = game.join("steam_api64.intimatr-stage.dll");
    let staged_runtime = game.join("intimatr.stage.dll");
    let filename = config_name(executable);
    let staged_config = cfg_dir.join(format!("{filename}.intimatr-stage"));
    let staged_state = game.join("intimatr-launch-deployment.stage.json");
    let marker = state_path(game);
    let result = (|| -> Result<(), String> {
        for path in [
            &staged_proxy,
            &staged_runtime,
            &staged_config,
            &staged_state,
        ] {
            require_absent(path)?;
        }
        write_new(&staged_proxy, &proxy_bytes)?;
        write_new(&staged_runtime, &runtime_bytes)?;
        write_new(&staged_config, &config_bytes)?;
        let state_bytes = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
        write_new(&staged_state, &state_bytes)?;
        fs::rename(&staged_runtime, game.join(RUNTIME)).map_err(|e| e.to_string())?;
        fs::rename(&staged_config, cfg_dir.join(&filename)).map_err(|e| e.to_string())?;
        fs::rename(game.join(ORIGINAL), game.join(SAVED)).map_err(|e| e.to_string())?;
        fs::rename(&staged_proxy, game.join(ORIGINAL)).map_err(|e| e.to_string())?;
        fs::rename(&staged_state, &marker).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        // Best-effort rollback only where contents prove our ownership.
        let original_path = game.join(ORIGINAL);
        let saved_path = game.join(SAVED);
        if same_file(&saved_path, &state.original_sha256).unwrap_or(false) {
            if same_file(&original_path, &state.proxy_sha256).unwrap_or(false) {
                let _ = fs::remove_file(&original_path);
            }
            if !original_path.exists() {
                let _ = fs::rename(&saved_path, &original_path);
            }
        }
        for (path, bytes_hash) in [
            (game.join(RUNTIME), &state.runtime_sha256),
            (cfg_dir.join(&filename), &state.config_sha256),
            (staged_proxy.clone(), &state.proxy_sha256),
            (staged_runtime.clone(), &state.runtime_sha256),
            (staged_config.clone(), &state.config_sha256),
        ] {
            if same_file(&path, bytes_hash).unwrap_or(false) {
                let _ = fs::remove_file(path);
            }
        }
        let _ = fs::remove_file(staged_state);
        if created_config_dir {
            let _ = fs::remove_dir(&cfg_dir);
        }
        return Err(format!(
            "installation failed; attempted safe rollback: {error}"
        ));
    }
    check_installed(game)?;
    print_report(
        "explicit_offline_launch_proxy_install",
        game,
        "installed",
        true,
    )
}

fn status_preflight(game: &Path, executable: &str) -> Result<(), String> {
    if present(&state_path(game))? || !present(&game.join(ORIGINAL))? {
        return Err(
            "installation requires a pristine original DLL and no deployment marker".into(),
        );
    }
    for path in [
        game.join(SAVED),
        game.join(RUNTIME),
        game.join("config").join(config_name(executable)),
        game.join(QUARANTINE),
        game.join("steam_api64.intimatr-stage.dll"),
        game.join("intimatr.stage.dll"),
        game.join("config")
            .join(format!("{}.intimatr-stage", config_name(executable))),
        game.join("intimatr-launch-deployment.stage.json"),
    ] {
        require_absent(&path)?;
    }
    Ok(())
}

fn remove(game: &Path) -> Result<(), String> {
    let state = check_installed(game)?;
    let filename = config_name(state_executable(&state)?);
    let original = game.join(ORIGINAL);
    let saved = game.join(SAVED);
    let quarantined = game.join(QUARANTINE);
    require_absent(&quarantined)?;
    fs::rename(&original, &quarantined)
        .map_err(|e| format!("cannot quarantine installed proxy: {e}"))?;
    if let Err(error) = fs::rename(&saved, &original) {
        let _ = fs::rename(&quarantined, &original);
        return Err(format!(
            "cannot restore original Steam DLL; attempted rollback: {error}"
        ));
    }
    if !same_file(&original, &state.original_sha256)? {
        return Err("original DLL restoration verification failed; preserving evidence".into());
    }
    // The original is already restored before any auxiliary files are removed.
    for path in [
        quarantined,
        game.join(RUNTIME),
        game.join("config").join(&filename),
        state_path(game),
    ] {
        fs::remove_file(&path).map_err(|e| {
            format!(
                "original restored, but cleanup incomplete at {}: {e}",
                path.display()
            )
        })?;
    }
    print_report(
        "explicit_offline_launch_proxy_remove",
        game,
        "not_installed",
        true,
    )
}

pub(super) fn run_launch_proxy(args: Vec<String>) -> Result<(), String> {
    let mut iter = args.into_iter();
    let action = iter.next().ok_or("expected status, install or remove")?;
    let mut game_dir = None;
    let mut proxy = None;
    let mut dll = None;
    let mut config = None;
    let mut exe = None;
    let mut expected_sha = None;
    let mut offline = false;
    let mut closed = false;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--offline-single-player" if !offline => offline = true,
            "--game-closed" if !closed => closed = true,
            _ => {
                let value = iter
                    .next()
                    .filter(|v| !v.is_empty() && !v.starts_with("--"))
                    .ok_or_else(|| format!("missing value after {arg}"))?;
                match arg.as_str() {
                    "--game-dir" if game_dir.is_none() => game_dir = Some(value),
                    "--proxy" if proxy.is_none() => proxy = Some(value),
                    "--dll" if dll.is_none() => dll = Some(value),
                    "--config" if config.is_none() => config = Some(value),
                    "--exe" if exe.is_none() => exe = Some(value),
                    "--original-sha256" if expected_sha.is_none() => expected_sha = Some(value),
                    _ => {
                        return Err(format!(
                            "unexpected or repeated launch-proxy argument: {arg}"
                        ));
                    }
                }
            }
        }
    }
    let game = verify_game_dir(Path::new(game_dir.as_deref().ok_or("missing --game-dir")?))?;
    match action.as_str() {
        "status" if !offline && !closed && proxy.is_none() && dll.is_none() && config.is_none() && exe.is_none() && expected_sha.is_none() => status(&game),
        "install" if offline && closed => {
            let selected = match exe {
                Some(value) => {
                    if expected_sha.is_none() {
                        return Err("generic install requires --original-sha256".into());
                    }
                    value
                },
                None if game.file_name().is_some_and(|name| name == "Eastward") => {
                    "Eastward.exe".to_owned()
                },
                None => return Err("generic install requires --exe and --original-sha256".into()),
            };
            install(
                &game,
                Path::new(proxy.as_deref().ok_or("install needs --proxy")?),
                Path::new(dll.as_deref().ok_or("install needs --dll")?),
                Path::new(config.as_deref().ok_or("install needs --config")?),
                &selected,
                expected_sha.as_deref(),
            )
        },
        "remove" if offline && closed && proxy.is_none() && dll.is_none() && config.is_none() && exe.is_none() && expected_sha.is_none() => remove(&game),
        _ => Err("usage: launch-proxy (status --game-dir DIR | install --game-dir DIR --proxy DLL --dll INTIMATR_DLL --config EXE_TOML [--exe GAME.exe --original-sha256 SHA256] --offline-single-player --game-closed | remove --game-dir DIR --offline-single-player --game-closed)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ID: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        game: PathBuf,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let num = ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "intimatr-launch-manager-{}-{num}",
                std::process::id()
            ));
            let game = root.join("Eastward/drive_c/Program Files/Eastward");
            let source = root.join("sources");
            fs::create_dir_all(&game).unwrap();
            fs::create_dir(&source).unwrap();
            fs::write(root.join("Eastward/bottle.yml"), b"Name: Eastward\n").unwrap();
            fs::write(game.join("Eastward.exe"), b"test executable").unwrap();
            fs::write(game.join(ORIGINAL), Self::dll(1)).unwrap();
            fs::write(source.join("proxy.dll"), Self::dll(2)).unwrap();
            fs::write(source.join("intimatr.dll"), Self::dll(3)).unwrap();
            fs::write(source.join(CONFIG), b"[target]\nexecutable = \"Eastward.exe\"\n[ui]\nenabled = false\n[rpc]\nenabled = false\n").unwrap();
            Self { root, game, source }
        }
        fn dll(fill: u8) -> Vec<u8> {
            let mut bytes = vec![fill; 512];
            bytes[0..2].copy_from_slice(b"MZ");
            bytes[0x3c..0x40].copy_from_slice(&(128u32).to_le_bytes());
            bytes[128..132].copy_from_slice(b"PE\0\0");
            bytes[132..134].copy_from_slice(&0x8664u16.to_le_bytes());
            bytes[150..152].copy_from_slice(&0x2000u16.to_le_bytes());
            bytes
        }
        fn install(&self) -> Result<(), String> {
            install(
                &self.game,
                &self.source.join("proxy.dll"),
                &self.source.join("intimatr.dll"),
                &self.source.join(CONFIG),
                "Eastward.exe",
                None,
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn installs_and_restores_exact_original_without_touching_bottle() {
        let fx = Fixture::new();
        let saved = fs::read(fx.game.join(ORIGINAL)).unwrap();
        let dir = verify_game_dir(&fx.game).unwrap();
        assert_eq!(dir, fx.game);
        status(&fx.game).unwrap();
        fx.install().unwrap();
        check_installed(&fx.game).unwrap();
        assert_eq!(fs::read(fx.game.join(SAVED)).unwrap(), saved);
        assert!(fx.install().is_err());
        remove(&fx.game).unwrap();
        assert_eq!(fs::read(fx.game.join(ORIGINAL)).unwrap(), saved);
        assert!(!fx.game.join(SAVED).exists());
        assert!(!fx.game.join(RUNTIME).exists());
        assert!(!fx.game.join("config").join(CONFIG).exists());
        assert!(!state_path(&fx.game).exists());
        status(&fx.game).unwrap();
    }

    #[test]
    fn modified_installed_proxy_blocks_removal_and_preserves_original_backup() {
        let fx = Fixture::new();
        fx.install().unwrap();
        let backup = fs::read(fx.game.join(SAVED)).unwrap();
        fs::write(fx.game.join(ORIGINAL), b"unrecognized replacement").unwrap();
        assert!(remove(&fx.game).is_err());
        assert_eq!(fs::read(fx.game.join(SAVED)).unwrap(), backup);
        assert_eq!(
            fs::read(fx.game.join(ORIGINAL)).unwrap(),
            b"unrecognized replacement"
        );
    }

    #[test]
    fn refuses_unmanaged_backup_or_config_and_unexpected_game_location() {
        let fx = Fixture::new();
        fs::write(fx.game.join(SAVED), b"someone else").unwrap();
        assert!(fx.install().is_err());
        assert_eq!(fs::read(fx.game.join(SAVED)).unwrap(), b"someone else");
        fs::remove_file(fx.game.join(SAVED)).unwrap();
        fs::create_dir(fx.game.join("config")).unwrap();
        fs::write(fx.game.join("config").join(CONFIG), b"existing settings").unwrap();
        assert!(fx.install().is_err());
        assert!(verify_game_dir(&fx.source).is_err());
    }

    #[test]
    fn multiple_independent_games_preserve_distinct_original_dlls_and_configs() {
        let fx = Fixture::new();
        let second = fx.root.join("SecondBottle/drive_c/Games/Another Game");
        fs::create_dir_all(&second).unwrap();
        fs::write(
            fx.root.join("SecondBottle/bottle.yml"),
            b"Name: SecondBottle\n",
        )
        .unwrap();
        fs::write(second.join("Another Game.exe"), b"fixture executable").unwrap();
        fs::write(second.join(ORIGINAL), Fixture::dll(7)).unwrap();
        let source_config = fx.source.join("Another Game.exe.toml");
        fs::write(&source_config, b"[target]\nexecutable = \"Another Game.exe\"\n[ui]\nenabled = false\n[rpc]\nenabled = false\n").unwrap();
        let expected_sha = digest(&Fixture::dll(7));
        assert!(verify_game_dir(&second).is_ok());
        assert!(
            install(
                &second,
                &fx.source.join("proxy.dll"),
                &fx.source.join("intimatr.dll"),
                &source_config,
                "Another Game.exe",
                Some(&expected_sha),
            )
            .is_ok()
        );
        fx.install().unwrap();
        assert_eq!(
            state_executable(&check_installed(&second).unwrap()).unwrap(),
            "Another Game.exe"
        );
        assert_eq!(
            state_executable(&check_installed(&fx.game).unwrap()).unwrap(),
            "Eastward.exe"
        );
        assert_eq!(fs::read(second.join(SAVED)).unwrap(), Fixture::dll(7));
        assert!(second.join("config/Another Game.exe.toml").exists());
        assert!(
            install(
                &second,
                &fx.source.join("proxy.dll"),
                &fx.source.join("intimatr.dll"),
                &source_config,
                "Another Game.exe",
                Some(&digest(&Fixture::dll(8))),
            )
            .is_err()
        );
        remove(&second).unwrap();
        assert_eq!(fs::read(second.join(ORIGINAL)).unwrap(), Fixture::dll(7));
        assert!(fx.game.join(SAVED).exists());
        remove(&fx.game).unwrap();
    }

    #[test]
    fn generic_install_refuses_wrong_original_digest_and_wrong_config_target() {
        let fx = Fixture::new();
        let wrong_sha = digest(&Fixture::dll(40));
        assert!(
            install(
                &fx.game,
                &fx.source.join("proxy.dll"),
                &fx.source.join("intimatr.dll"),
                &fx.source.join(CONFIG),
                "Eastward.exe",
                Some(&wrong_sha),
            )
            .is_err()
        );
        assert!(!state_path(&fx.game).exists());
        let correct_sha = digest(&Fixture::dll(1));
        assert!(
            install(
                &fx.game,
                &fx.source.join("proxy.dll"),
                &fx.source.join("intimatr.dll"),
                &fx.source.join(CONFIG),
                "Different.exe",
                Some(&correct_sha),
            )
            .is_err()
        );
        assert!(!fx.game.join(SAVED).exists());
    }

    #[test]
    fn legacy_v1_deployment_marker_remains_readable() {
        let fx = Fixture::new();
        fx.install().unwrap();
        let mut v: serde_json::Value =
            serde_json::from_slice(&fs::read(state_path(&fx.game)).unwrap()).unwrap();
        v["version"] = serde_json::json!(1);
        v.as_object_mut().unwrap().remove("executable");
        fs::write(state_path(&fx.game), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
        assert!(check_installed(&fx.game).is_ok());
        remove(&fx.game).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_original_and_game_directory() {
        use std::os::unix::fs::symlink;
        let fx = Fixture::new();
        fs::rename(fx.game.join(ORIGINAL), fx.source.join("actual-original")).unwrap();
        symlink(fx.source.join("actual-original"), fx.game.join(ORIGINAL)).unwrap();
        assert!(fx.install().is_err());
        symlink(&fx.game, fx.root.join("alias")).unwrap();
        assert!(verify_game_dir(&fx.root.join("alias")).is_err());
    }
}
