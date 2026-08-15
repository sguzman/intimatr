use std::path::Path;

use intimatr::config::{AppConfig, ConfigError};

const SAMPLE: &str = include_str!("../config/ExampleGame.exe.toml");

#[test]
fn sample_configuration_parses() {
    let config = AppConfig::from_toml_str(SAMPLE).expect("sample config should parse");

    assert_eq!(config.target.executable, "ExampleGame.exe");
    assert_eq!(config.rpc.bind, "127.0.0.1:31337");
    assert!(config.policy.allow_memory_read);
    assert!(config.policy.allow_memory_write);
    assert!(config.ui.initially_visible);
    assert_eq!(config.ui.scan_page_size, 256);
}

#[test]
fn executable_name_selects_one_config_file() {
    let path = AppConfig::config_path_for_executable(
        Path::new("config"),
        Path::new("games").join("ExampleGame.exe"),
    )
    .expect("executable should resolve");

    assert_eq!(path, Path::new("config").join("ExampleGame.exe.toml"));
}

#[test]
fn executable_validation_is_case_insensitive() {
    let config = AppConfig::from_toml_str(SAMPLE).expect("sample config should parse");

    config
        .validate_for_executable(Path::new("examplegame.EXE"))
        .expect("Windows executable matching should be case-insensitive");
}

#[test]
fn executable_mismatch_is_rejected() {
    let config = AppConfig::from_toml_str(SAMPLE).expect("sample config should parse");

    let error = config
        .validate_for_executable(Path::new("OtherGame.exe"))
        .expect_err("wrong executable should be rejected");

    assert!(matches!(error, ConfigError::ExecutableMismatch { .. }));
}

#[test]
fn invalid_scanner_alignment_is_rejected() {
    let input = SAMPLE.replace("alignment = 1", "alignment = 0");
    let error = AppConfig::from_toml_str(&input).expect_err("zero alignment is invalid");

    assert!(matches!(error, ConfigError::InvalidValue(_)));
}

#[test]
fn invalid_ui_page_size_is_rejected() {
    let input = SAMPLE.replace("scan_page_size = 256", "scan_page_size = 8192");
    let error = AppConfig::from_toml_str(&input).expect_err("oversized UI page is invalid");

    assert!(matches!(error, ConfigError::InvalidValue(_)));
}
