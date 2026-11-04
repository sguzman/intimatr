//! Read-only fail-closed Bottles environment policy preflight.
//! This validates a reported setting, not delivery into a running game.

use serde_json::Value;

const PROFILE_ENV: &str = "INTIMATR_SPEED_PROFILE";

pub(super) fn verify_profile_environment(
    inventory_json: &str,
    bottle: &str,
) -> Result<&'static str, String> {
    let document: Value = serde_json::from_str(inventory_json)
        .map_err(|error| format!("invalid native Bottles environment inventory: {error}"))?;
    let root = document
        .as_object()
        .ok_or("native Bottles environment inventory must be a name-keyed object")?;
    let bottles = match root.get("bottles").or_else(|| root.get("Bottles")) {
        Some(Value::Object(items)) => items,
        Some(_) => return Err("native Bottles environment inventory lacks bottle metadata".into()),
        None => root,
    };
    let selected = bottles
        .get(bottle)
        .ok_or("selected bottle is absent from native environment inventory")?
        .as_object()
        .ok_or("selected bottle has no environment configuration metadata")?;

    // Unknown settings cannot authorize unrestricted inheritance.
    let limited = selected
        .get("Limit_System_Environment")
        .and_then(Value::as_bool)
        .ok_or("selected bottle Limit_System_Environment is missing or not a boolean")?;
    let explicit = selected
        .get("Environment_Variables")
        .and_then(Value::as_object)
        .ok_or("selected bottle Environment_Variables is missing or not an object")?;
    if explicit
        .keys()
        .any(|name| name.eq_ignore_ascii_case(PROFILE_ENV))
    {
        return Err("selected bottle explicitly overrides INTIMATR_SPEED_PROFILE".into());
    }

    if limited {
        let inherited = selected
            .get("Inherited_Environment_Variables")
            .and_then(Value::as_array)
            .ok_or("restricted bottle has no valid inherited environment variable list")?;
        let mut occurrences = 0;
        for item in inherited {
            let name = item
                .as_str()
                .ok_or("inherited environment variable list contains a non-string entry")?;
            if name == PROFILE_ENV {
                occurrences += 1;
            }
        }
        if occurrences != 1 {
            return Err(
                "restricted bottle must allowlist INTIMATR_SPEED_PROFILE exactly once before launch"
                    .into(),
            );
        }
        Ok("restricted_explicitly_allowlisted")
    } else {
        Ok("unrestricted_inheritance")
    }
}

/// The registered program can add a second, higher-precedence environment
/// layer after the bottle-wide inherited-variable filter. Never accept a
/// per-program value that could shadow this explicitly selected launch file.
pub(super) fn verify_registered_program_environment(
    programs_json: &str,
    program_name: &str,
) -> Result<(), String> {
    let value: Value = serde_json::from_str(programs_json).map_err(|error| {
        format!("invalid native Bottles registered program environment: {error}")
    })?;
    let programs = value
        .as_array()
        .ok_or("native Bottles programs must be an array")?;
    let selected = programs
        .iter()
        .filter(|program| program.get("name").and_then(Value::as_str) == Some(program_name))
        .collect::<Vec<_>>();
    if selected.len() != 1 {
        return Err(
            "selected registered program environment identity is absent or ambiguous".into(),
        );
    }
    let program = selected[0]
        .as_object()
        .ok_or("selected registered program environment record is invalid")?;
    match program.get("environment") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(environment)) => {
            if environment
                .keys()
                .any(|name| name.eq_ignore_ascii_case(PROFILE_ENV))
            {
                Err("selected registered program overrides INTIMATR_SPEED_PROFILE".into())
            } else {
                Ok(())
            }
        }
        Some(_) => Err("selected registered program environment must be an object".into()),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ProfileEnvironmentOptions {
    bottle: String,
    program: Option<String>,
    bottles_cli: String,
}

impl ProfileEnvironmentOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut bottle = None;
        let mut program = None;
        let mut bottles_cli = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            let value = iter
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
                "--program" if program.is_none() => program = Some(value.clone()),
                "--bottles-cli" if bottles_cli.is_none() => bottles_cli = Some(value.clone()),
                _ => return Err(format!("duplicate or unexpected profile-env flag: {flag}")),
            }
        }
        let bottle: String = bottle.ok_or("missing --bottle")?;
        if bottle.trim().is_empty() {
            return Err("selected bottle name must not be empty".into());
        }
        if program.as_deref().is_some_and(|name| name.trim().is_empty()) {
            return Err("selected registered program name must not be empty".into());
        }
        let bottles_cli = bottles_cli.unwrap_or_else(|| "bottles-cli".to_owned());
        if std::path::Path::new(&bottles_cli)
            .file_name()
            .is_none_or(|name| name != "bottles-cli")
        {
            return Err("expected a native bottles-cli executable".into());
        }
        Ok(Self {
            bottle,
            program,
            bottles_cli,
        })
    }
}

/// Read-only report for native field inspection, with blockers shown as data.
pub(super) fn run_profile_env(args: Vec<String>) -> Result<(), String> {
    let options = ProfileEnvironmentOptions::parse(&args)?;
    let inventory = super::bottles_json_with_command(&options.bottles_cli, &["list", "bottles"])?;
    let (mode, blocker) = match verify_profile_environment(&inventory, &options.bottle) {
        Ok(mode) => (Some(mode), None),
        Err(error) => (None, Some(error)),
    };
    let (program_passed, program_blocker) = if let Some(program) = &options.program {
        let programs =
            super::bottles_json_with_command(&options.bottles_cli, &["programs", "-b", &options.bottle])?;
        match verify_registered_program_environment(&programs, program) {
            Ok(()) => (Some(true), None),
            Err(error) => (Some(false), Some(error)),
        }
    } else {
        (None, None)
    };
    let combined = program_passed.map(|passed| passed && mode.is_some());
    let report = serde_json::json!({
        "operation": "read_only_native_bottles_profile_environment_preflight",
        "selected_bottle": options.bottle,
        "selected_registered_program": options.program,
        "reported_bottle_policy_accepts_profile_environment": mode.is_some(),
        "reported_registered_program_environment_accepts_profile": program_passed,
        "registered_program_environment_blocker": program_blocker,
        "reported_combined_profile_environment_policy_accepts": combined,
        "reported_environment_mode": mode,
        "blocker": blocker,
        "bottles_run_invoked": false,
        "profile_delivery_into_game_verified": false,
        "game_process_started_verified": false,
        "bottle_configuration_modified": false,
        "attachment_attempted": false
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| format!("cannot encode Bottles environment preflight: {error}"))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ProfileEnvironmentOptions, verify_profile_environment,
        verify_registered_program_environment,
    };
    use serde_json::{Value, json};

    fn inventory(value: Value) -> String {
        json!({
            "Selected": value,
            "Other": {
                "Limit_System_Environment": false,
                "Environment_Variables": {}
            }
        })
        .to_string()
    }

    #[test]
    fn two_explicit_inheritance_modes() {
        let unrestricted = inventory(json!({
            "Limit_System_Environment": false, "Environment_Variables": {}
        }));
        assert_eq!(
            verify_profile_environment(&unrestricted, "Selected").unwrap(),
            "unrestricted_inheritance"
        );
        let restricted = inventory(json!({
            "Limit_System_Environment": true,
            "Environment_Variables": {},
            "Inherited_Environment_Variables": ["PATH", "INTIMATR_SPEED_PROFILE"]
        }));
        assert_eq!(
            verify_profile_environment(&restricted, "Selected").unwrap(),
            "restricted_explicitly_allowlisted"
        );
        let nested = json!({"bottles": serde_json::from_str::<Value>(&restricted).unwrap()});
        assert_eq!(
            verify_profile_environment(&nested.to_string(), "Selected").unwrap(),
            "restricted_explicitly_allowlisted"
        );
    }

    #[test]
    fn restricted_bottle_requires_single_exact_allowlist_entry() {
        for inherited in [
            json!([]),
            json!(["PATH"]),
            json!(["intimatr_speed_profile"]),
            json!(["INTIMATR_SPEED_PROFILE", "INTIMATR_SPEED_PROFILE"]),
        ] {
            let raw = inventory(json!({
                "Limit_System_Environment": true,
                "Environment_Variables": {},
                "Inherited_Environment_Variables": inherited
            }));
            assert!(
                verify_profile_environment(&raw, "Selected")
                    .unwrap_err()
                    .contains("must allowlist")
            );
        }
    }

    #[test]
    fn explicit_override_blocks_both_modes() {
        for limited in [true, false] {
            let raw = inventory(json!({
                "Limit_System_Environment": limited,
                "Environment_Variables": {"intimatr_speed_profile": "C:\\wrong.toml"},
                "Inherited_Environment_Variables": ["INTIMATR_SPEED_PROFILE"]
            }));
            assert!(
                verify_profile_environment(&raw, "Selected")
                    .unwrap_err()
                    .contains("explicitly overrides")
            );
        }
    }

    #[test]
    fn registered_game_environment_cannot_shadow_selected_profile() {
        for good in [
            r#"[{"name":"Game","path":"C:\\\\Games\\\\Game.exe"}]"#,
            r#"[{"name":"Game","environment":null}]"#,
            r#"[{"name":"Game","environment":{"OTHER":"ok"}}]"#,
        ] {
            assert!(
                verify_registered_program_environment(good, "Game").is_ok(),
                "{good}"
            );
        }
        for bad in [
            r#"[{"name":"Game","environment":{"INTIMATR_SPEED_PROFILE":"C:\\\\other.toml"}}]"#,
            r#"[{"name":"Game","environment":{"intimatr_speed_profile":"C:\\\\other.toml"}}]"#,
            r#"[{"name":"Game","environment":"INTIMATR_SPEED_PROFILE=x"}]"#,
            r#"[{"name":"Game"},{"name":"Game"}]"#,
            r#"[{"name":"Other"}]"#,
            r#"{"Game":{}}"#,
        ] {
            assert!(
                verify_registered_program_environment(bad, "Game").is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn read_only_cli_requires_explicit_bottle_and_native_command() {
        let valid = [
            "--bottle",
            "My Games",
            "--bottles-cli",
            "/usr/bin/bottles-cli",
        ]
        .map(str::to_owned);
        assert_eq!(
            ProfileEnvironmentOptions::parse(&valid).unwrap().bottle,
            "My Games"
        );
        let selected = ["--bottle", "My Games", "--program", "Single Player"].map(str::to_owned);
        assert_eq!(
            ProfileEnvironmentOptions::parse(&selected)
                .unwrap()
                .program
                .as_deref(),
            Some("Single Player")
        );
        for bad in [
            vec![],
            vec!["--bottle", ""],
            vec!["--bottle", "Games", "--bottle", "Another"],
            vec!["--bottle", "Games", "--program", ""],
            vec!["--bottle", "Games", "--program", "Game", "--program", "Other"],
            vec!["--bottle", "Games", "--bottles-cli", "/usr/bin/wine"],
            vec!["--bottle", "Games", "--unknown", "value"],
        ] {
            let args: Vec<String> = bad.into_iter().map(str::to_owned).collect();
            assert!(ProfileEnvironmentOptions::parse(&args).is_err());
        }
    }

    #[test]
    fn incomplete_invalid_or_wrong_bottle_metadata_is_refused() {
        for raw in [
            "[]".to_owned(),
            "{}".to_owned(),
            json!({"bottles": ["Selected"]}).to_string(),
            inventory(json!({})),
            inventory(json!({
                "Limit_System_Environment": "false",
                "Environment_Variables": {}
            })),
            inventory(json!({"Limit_System_Environment": false})),
            inventory(json!({
                "Limit_System_Environment": true,
                "Environment_Variables": {},
                "Inherited_Environment_Variables": ["INTIMATR_SPEED_PROFILE", 42]
            })),
        ] {
            assert!(
                verify_profile_environment(&raw, "Selected").is_err(),
                "{raw}"
            );
        }
        let raw = inventory(json!({
            "Limit_System_Environment": false,
            "Environment_Variables": {}
        }));
        assert!(verify_profile_environment(&raw, "Absent").is_err());
    }
}
