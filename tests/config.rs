use std::path::Path;

use intimatr::config::{AppConfig, ConfigError};

const SAMPLE: &str = include_str!("../config/ExampleGame.exe.toml");

#[test]
fn sample_configuration_parses() {
    let config = AppConfig::from_toml_str(SAMPLE).expect("sample config should parse");

    assert_eq!(config.target.executable, "ExampleGame.exe");
    assert_eq!(config.rpc.bind, "127.0.0.1:31337");
    assert_eq!(config.rpc.pipe_name, "intimatr");
    assert_eq!(config.rpc.max_response_bytes, 4_194_304);
    assert!(config.policy.allow_memory_read);
    assert!(config.policy.allow_memory_write);
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
fn tcp_rpc_bind_must_be_loopback() {
    let input = SAMPLE.replace("127.0.0.1:31337", "0.0.0.0:31337");
    let error = AppConfig::from_toml_str(&input).expect_err("non-loopback RPC must be rejected");

    assert!(matches!(error, ConfigError::InvalidValue(_)));
}

#[test]
fn named_pipe_client_limit_is_bounded_by_windows() {
    let input = SAMPLE
        .replace("transport = \"tcp\"", "transport = \"named_pipe\"")
        .replace("max_clients = 4", "max_clients = 255");
    let error = AppConfig::from_toml_str(&input).expect_err("too many pipe instances is invalid");

    assert!(matches!(error, ConfigError::InvalidValue(_)));
}
