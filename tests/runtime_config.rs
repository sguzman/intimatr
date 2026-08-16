use intimatr::config::AppConfig;

#[test]
fn runtime_queue_defaults_are_bounded() {
    let config = AppConfig::from_toml_str(
        r#"
[target]
executable = "ExampleGame.exe"
"#,
    )
    .expect("minimal config should use defaults");

    assert_eq!(config.runtime.command_workers, 4);
    assert_eq!(config.runtime.command_queue_capacity, 64);
}

#[test]
fn runtime_queue_configuration_is_validated() {
    for invalid in [
        r#"
[target]
executable = "ExampleGame.exe"
[runtime]
command_workers = 0
command_queue_capacity = 64
"#,
        r#"
[target]
executable = "ExampleGame.exe"
[runtime]
command_workers = 4
command_queue_capacity = 0
"#,
        r#"
[target]
executable = "ExampleGame.exe"
[runtime]
command_workers = 33
command_queue_capacity = 64
"#,
        r#"
[target]
executable = "ExampleGame.exe"
[runtime]
command_workers = 4
command_queue_capacity = 65537
"#,
    ] {
        assert!(AppConfig::from_toml_str(invalid).is_err());
    }
}
