//! Limited matches by set (dashboard redesign spec section 4.10, the Sets
//! card): a match's run's set, else the set its format code names, else
//! Other.

use std::collections::HashMap;

use mtgo_match::Outcome;

use crate::set_code::set_code_from_play_format;

/// One limited match to place in a set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetMatch {
    pub token: String,
    /// The match's format code (`DHOBHOBHOB`).
    pub format: Option<String>,
    pub outcome: Outcome,
}

/// One set's matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetCount {
    /// `None` for the Other row.
    pub set_code: Option<String>,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
}

/// Matches by set: a match's run's set (`run_sets`, by match token), else
/// the set its format code names, else Other. Most matches first, then by
/// code; Other last.
pub fn matches_by_set(matches: &[SetMatch], run_sets: &HashMap<String, String>) -> Vec<SetCount> {
    let mut counts: Vec<SetCount> = Vec::new();
    for played in matches {
        let set_code = match run_sets.get(&played.token) {
            Some(code) => Some(code.clone()),
            None => played.format.as_deref().and_then(set_code_from_play_format),
        };
        let index = match counts.iter().position(|count| count.set_code == set_code) {
            Some(index) => index,
            None => {
                counts.push(SetCount {
                    set_code,
                    matches: 0,
                    wins: 0,
                    losses: 0,
                });
                counts.len() - 1
            }
        };
        let count = &mut counts[index];
        count.matches += 1;
        match played.outcome {
            Outcome::Win => count.wins += 1,
            Outcome::Loss => count.losses += 1,
            Outcome::Draw | Outcome::Unknown => {}
        }
    }
    counts.sort_by(|a, b| {
        a.set_code
            .is_none()
            .cmp(&b.set_code.is_none())
            .then_with(|| b.matches.cmp(&a.matches))
            .then_with(|| a.set_code.cmp(&b.set_code))
    });
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played(token: &str, format: Option<&str>, outcome: Outcome) -> SetMatch {
        SetMatch {
            token: token.to_string(),
            format: format.map(str::to_string),
            outcome,
        }
    }

    #[test]
    fn a_match_takes_its_runs_set_then_its_formats_then_other() {
        let run_sets = HashMap::from([("q-1".to_string(), "QNW".to_string())]);
        let matches = [
            played("q-1", Some("DQODQODQOD"), Outcome::Win),
            played("q-2", Some("DQNWQNWQNW"), Outcome::Loss),
            played("q-3", Some("S6QOD"), Outcome::Win),
            played("q-4", Some("Limited"), Outcome::Draw),
            played("q-5", None, Outcome::Win),
        ];
        assert_eq!(
            matches_by_set(&matches, &run_sets),
            vec![
                SetCount {
                    set_code: Some("QNW".to_string()),
                    matches: 2,
                    wins: 1,
                    losses: 1
                },
                SetCount {
                    set_code: Some("QOD".to_string()),
                    matches: 1,
                    wins: 1,
                    losses: 0
                },
                SetCount {
                    set_code: None,
                    matches: 2,
                    wins: 1,
                    losses: 0
                },
            ]
        );
        assert!(matches_by_set(&[], &run_sets).is_empty());
    }
}
