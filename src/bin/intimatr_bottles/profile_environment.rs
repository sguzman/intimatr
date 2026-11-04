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

#[cfg(test)]
mod tests {
    use super::verify_profile_environment;
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
            assert!(verify_profile_environment(&raw, "Selected").is_err(), "{raw}");
        }
        let raw = inventory(json!({
            "Limit_System_Environment": false,
            "Environment_Variables": {}
        }));
        assert!(verify_profile_environment(&raw, "Absent").is_err());
    }
}
