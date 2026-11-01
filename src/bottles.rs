//! Read-only Bottles CLI JSON decoding.
//!
//! The Bottles integration manager is native Linux software. Discovering
//! registered games is deliberately separate from deployment, runtime attach,
//! and any modification to a bottle.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredProgram {
    pub name: String,
    pub executable: String,
    pub path: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BottleInventory {
    pub bottle: String,
    pub programs: Vec<RegisteredProgram>,
}

#[derive(Debug, Error)]
pub enum BottlesInventoryError {
    #[error("invalid Bottles JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported Bottles list JSON shape")]
    UnsupportedList,
    #[error("Bottles returned a missing or empty bottle name")]
    MissingBottleName,
    #[error("Bottles returned a registered program with missing identity or path")]
    InvalidProgram,
}

/// Accept explicit JSON arrays or a top-level `bottles` collection.
///
/// The Bottles CLI documentation documents JSON output but does not guarantee
/// a stable full list-bottles schema. Reject unknown shapes rather than
/// inventing bottle names or inspecting arbitrary Wine directories.
pub fn parse_bottle_names(input: &str) -> Result<Vec<String>, BottlesInventoryError> {
    let root: Value = serde_json::from_str(input)?;
    let collection = match &root {
        Value::Array(_) => &root,
        Value::Object(map) => map
            .get("bottles")
            .or_else(|| map.get("Bottles"))
            .ok_or(BottlesInventoryError::UnsupportedList)?,
        _ => return Err(BottlesInventoryError::UnsupportedList),
    };

    let mut names = Vec::new();
    match collection {
        Value::Array(items) => {
            for item in items {
                let name = match item {
                    Value::String(name) => name.as_str(),
                    Value::Object(map) => map
                        .get("name")
                        .or_else(|| map.get("Name"))
                        .and_then(Value::as_str)
                        .ok_or(BottlesInventoryError::MissingBottleName)?,
                    _ => return Err(BottlesInventoryError::UnsupportedList),
                };
                if name.trim().is_empty() {
                    return Err(BottlesInventoryError::MissingBottleName);
                }
                names.push(name.to_owned());
            }
        }
        Value::Object(map) => {
            for name in map.keys() {
                if name.trim().is_empty() {
                    return Err(BottlesInventoryError::MissingBottleName);
                }
                names.push(name.clone());
            }
        }
        _ => return Err(BottlesInventoryError::UnsupportedList),
    }

    names.sort_unstable();
    names.dedup();
    Ok(names)
}

/// Parse only programs that Bottles itself reports as registered.
///
/// Bottles explicitly says this list does not include every loose executable
/// someone might launch by hand. No false claim of full disk discovery.
pub fn parse_registered_programs(
    input: &str,
) -> Result<Vec<RegisteredProgram>, BottlesInventoryError> {
    let programs: Vec<RegisteredProgram> = serde_json::from_str(input)?;
    if programs.iter().any(|program| {
        program.name.trim().is_empty()
            || program.executable.trim().is_empty()
            || program.path.trim().is_empty()
    }) {
        return Err(BottlesInventoryError::InvalidProgram);
    }
    Ok(programs)
}

#[cfg(test)]
mod tests {
    use super::{BottlesInventoryError, parse_bottle_names, parse_registered_programs};

    #[test]
    fn reads_empty_and_named_bottles() {
        assert!(parse_bottle_names("[]").unwrap().is_empty());
        assert!(parse_bottle_names(r#"{"bottles":[]}"#).unwrap().is_empty());
        assert_eq!(
            parse_bottle_names(r#"{"bottles":["Gaming", "Media", "Gaming"]}"#).unwrap(),
            vec!["Gaming", "Media"]
        );
        assert_eq!(
            parse_bottle_names(r#"{"bottles":[{"name":"My Bottle"}]}"#).unwrap(),
            vec!["My Bottle"]
        );
    }

    #[test]
    fn rejects_unknown_bottle_shapes_and_missing_names() {
        for invalid in [
            "not json",
            "{}",
            "42",
            r#"{"bottles":null}"#,
            r#"{"bottles":[""]}"#,
            r#"{"bottles":[{}]}"#,
        ] {
            assert!(parse_bottle_names(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn accepts_documented_bottles_program_json() {
        // Structure taken from Bottles' publicly documented --json programs
        // format; the example executable and path are synthetic.
        let sample = r#"[{"executable":"Game.exe","arguments":"-windowed","name":"My Game","path":"C:\\Games\\Game.exe","folder":"","icon":"","script":null,"removed":null}]"#;
        let programs = parse_registered_programs(sample).unwrap();
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0].name, "My Game");
        assert_eq!(programs[0].executable, "Game.exe");
        assert_eq!(programs[0].path, r"C:\Games\Game.exe");
    }

    #[test]
    fn does_not_silently_discard_malformed_program_records() {
        assert!(parse_registered_programs("[]").unwrap().is_empty());
        assert!(matches!(
            parse_registered_programs(
                r#"[{"name":"","executable":"game.exe","path":"C:\\Game.exe"}]"#
            ),
            Err(BottlesInventoryError::InvalidProgram)
        ));
        assert!(parse_registered_programs(r#"[{"name":"Game"}]"#).is_err());
    }
}
