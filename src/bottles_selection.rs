//! Read-only selection rules for Bottles-registered offline games.
//!
//! This produces an auditable list. It never claims a DLL is installed,
//! loaded, compatible, or attached, and it never mutates any bottle.

use serde::Serialize;

use crate::bottles::BottleInventory;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionRules {
    pub all: bool,
    pub include_bottle: Vec<String>,
    pub include_program: Vec<String>,
    pub exclude_bottle: Vec<String>,
    pub exclude_program: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionDecision {
    Selected,
    NotIncluded,
    ExcludedBottle,
    ExcludedProgram,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedProgram {
    pub bottle: String,
    pub name: String,
    pub executable: String,
    pub path: String,
    pub decision: SelectionDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionPlan {
    /// Planning only; no DLL installation, load, or compatibility checking.
    pub operation: &'static str,
    pub selected_count: usize,
    pub programs: Vec<PlannedProgram>,
}

impl SelectionRules {
    pub fn validate(&self) -> Result<(), &'static str> {
        let includes = !self.include_bottle.is_empty() || !self.include_program.is_empty();
        if !self.all && !includes {
            return Err("explicit --all or at least one include filter is required");
        }
        if self.all && includes {
            return Err("--all cannot be combined with include filters");
        }
        let patterns = self
            .include_bottle
            .iter()
            .chain(&self.include_program)
            .chain(&self.exclude_bottle)
            .chain(&self.exclude_program);
        let mut count = 0;
        for pattern in patterns {
            count += 1;
            if pattern.trim().is_empty() || pattern.len() > 256 {
                return Err("patterns must be nonempty and at most 256 bytes");
            }
        }
        if count > 64 {
            return Err("too many filter patterns (maximum 64)");
        }
        Ok(())
    }

    pub fn plan(&self, inventory: &[BottleInventory]) -> Result<SelectionPlan, &'static str> {
        self.validate()?;
        let mut programs = Vec::new();
        let mut selected_count = 0;
        for bottle in inventory {
            for program in &bottle.programs {
                let bottle_allowed = self.include_bottle.is_empty()
                    || any_match(&self.include_bottle, &bottle.bottle);
                let program_allowed = self.include_program.is_empty()
                    || matches_program(
                        &self.include_program,
                        &program.name,
                        &program.executable,
                        &program.path,
                    );
                let decision = if any_match(&self.exclude_bottle, &bottle.bottle) {
                    SelectionDecision::ExcludedBottle
                } else if matches_program(
                    &self.exclude_program,
                    &program.name,
                    &program.executable,
                    &program.path,
                ) {
                    SelectionDecision::ExcludedProgram
                } else if self.all || (bottle_allowed && program_allowed) {
                    SelectionDecision::Selected
                } else {
                    SelectionDecision::NotIncluded
                };
                if decision == SelectionDecision::Selected {
                    selected_count += 1;
                }
                programs.push(PlannedProgram {
                    bottle: bottle.bottle.clone(),
                    name: program.name.clone(),
                    executable: program.executable.clone(),
                    path: program.path.clone(),
                    decision,
                });
            }
        }
        Ok(SelectionPlan {
            operation: "read_only_selection",
            selected_count,
            programs,
        })
    }
}

fn any_match(patterns: &[String], target: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| wildcard_match(pattern, target))
}

fn matches_program(patterns: &[String], name: &str, executable: &str, path: &str) -> bool {
    any_match(patterns, name) || any_match(patterns, executable) || any_match(patterns, path)
}

/// Anchored, case-insensitive ASCII glob with ? and *; both Windows and
/// Unix separators compare as '/'. No regex or shell expansion is performed.
pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    let value: Vec<char> = value
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    let mut prior = vec![false; value.len() + 1];
    prior[0] = true;
    for token in pattern {
        let mut next = vec![false; value.len() + 1];
        if token == '*' {
            next[0] = prior[0];
            for j in 1..=value.len() {
                next[j] = prior[j] || next[j - 1];
            }
        } else {
            for j in 1..=value.len() {
                next[j] =
                    prior[j - 1] && (token == '?' || token.eq_ignore_ascii_case(&value[j - 1]));
            }
        }
        prior = next;
    }
    prior[value.len()]
}

#[cfg(test)]
mod tests {
    use super::{SelectionDecision, SelectionRules, wildcard_match};
    use crate::bottles::{BottleInventory, RegisteredProgram};

    fn bottle(name: &str, programs: &[(&str, &str)]) -> BottleInventory {
        BottleInventory {
            bottle: name.to_owned(),
            programs: programs
                .iter()
                .map(|(name, exe)| RegisteredProgram {
                    name: (*name).to_owned(),
                    executable: (*exe).to_owned(),
                    path: format!(r"C:\\Games\\{exe}"),
                    arguments: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn wildcard_matches_anchored_windows_paths_and_names() {
        assert!(wildcard_match("disco*.exe", "Disco Elysium.exe"));
        assert!(!wildcard_match("disco", "Disco Elysium.exe"));
        assert!(wildcard_match("C:/Games/*.EXE", r"C:\Games\Game.exe"));
        assert!(wildcard_match("Game?.exe", "Game1.exe"));
        assert!(!wildcard_match("Game?.exe", "Game10.exe"));
    }

    #[test]
    fn requires_opt_in_even_when_every_program_is_known() {
        assert!(SelectionRules::default().plan(&[]).is_err());
        let invalid = SelectionRules {
            all: true,
            include_program: vec!["*.exe".into()],
            ..SelectionRules::default()
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn filters_intersect_and_deny_overrides_allow() {
        let inventory = vec![
            bottle(
                "Games",
                &[("Disco", "Disco.exe"), ("Launcher", "Launcher.exe")],
            ),
            bottle("Tests", &[("Disco", "Disco.exe"), ("Dredge", "Dredge.exe")]),
        ];
        let rules = SelectionRules {
            include_bottle: vec!["Games".into()],
            include_program: vec!["Disco*.exe".into(), "Launcher.exe".into()],
            exclude_program: vec!["Launcher.exe".into()],
            ..SelectionRules::default()
        };
        let plan = rules.plan(&inventory).unwrap();
        assert_eq!(plan.selected_count, 1);
        assert_eq!(plan.programs[0].decision, SelectionDecision::Selected);
        assert_eq!(
            plan.programs[1].decision,
            SelectionDecision::ExcludedProgram
        );
        assert_eq!(plan.programs[2].decision, SelectionDecision::NotIncluded);
        assert_eq!(plan.programs[3].decision, SelectionDecision::NotIncluded);
    }

    #[test]
    fn all_with_excludes_still_never_modifies_bottles() {
        let inventory = vec![
            bottle("Games", &[("Disco", "Disco.exe")]),
            bottle("Online", &[("Game", "Online.exe")]),
        ];
        let rules = SelectionRules {
            all: true,
            exclude_bottle: vec!["Online".into()],
            ..SelectionRules::default()
        };
        let plan = rules.plan(&inventory).unwrap();
        assert_eq!(plan.operation, "read_only_selection");
        assert_eq!(plan.selected_count, 1);
        assert_eq!(plan.programs[1].decision, SelectionDecision::ExcludedBottle);
    }

    #[test]
    fn invalid_and_overlong_filters_fail_closed() {
        for pattern in ["", " ", "x".repeat(257).as_str()] {
            let rules = SelectionRules {
                include_program: vec![pattern.to_owned()],
                ..SelectionRules::default()
            };
            assert!(rules.validate().is_err());
        }
    }

    #[test]
    fn duplicate_executable_names_remain_disambiguated_by_bottle() {
        let inventory = vec![
            bottle("A", &[("Game", "Game.exe")]),
            bottle("B", &[("Game", "Game.exe")]),
        ];
        let rules = SelectionRules {
            include_bottle: vec!["A".into()],
            ..SelectionRules::default()
        };
        let plan = rules.plan(&inventory).unwrap();
        assert_eq!(plan.selected_count, 1);
        assert_eq!(plan.programs[0].bottle, "A");
        assert_eq!(plan.programs[1].decision, SelectionDecision::NotIncluded);
    }
}
