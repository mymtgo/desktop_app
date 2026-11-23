//! Which release folders in one channel folder of the update bucket
//! (`stable/<version>/`, `edge/<version>/`, the legacy `beta/<version>/`) a
//! release deletes: every version but the newest `keep`, by semver, and never
//! a version a feed points at (one may have been rolled back to an older
//! version, and edge and the beta pointer may point into `stable/`). A name
//! that is not a version is never deleted. `newer` is the semver order the
//! promote workflow uses.

use std::cmp::Ordering;

/// The versions to delete, oldest first. `versions` are the folder names;
/// `promoted` are the feeds' versions that could be read.
pub fn to_delete(versions: &[String], promoted: &[String], keep: usize) -> Vec<String> {
    let mut parsed: Vec<(Version, String)> = Vec::new();
    for name in versions {
        if let Some(version) = Version::parse(name)
            && !parsed.iter().any(|(_, seen)| seen == name)
        {
            parsed.push((version, name.clone()));
        }
    }
    // Newest first.
    parsed.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<String> = parsed
        .into_iter()
        .skip(keep)
        .map(|(_, name)| name)
        .filter(|name| !promoted.contains(name))
        .collect();
    out.reverse();
    out
}

/// Whether `candidate` is newer than `current` by semver; `None` when either
/// is not a version.
pub fn newer(candidate: &str, current: &str) -> Option<bool> {
    let candidate = Version::parse(candidate)?;
    let current = Version::parse(current)?;
    Some(candidate > current)
}

/// A version as semver orders it: `1.0.0-beta.2` < `1.0.0-beta.10` <
/// `1.0.0`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    /// The dot-separated pre-release parts; empty for a release.
    pre_release: Vec<String>,
}

impl Version {
    fn parse(text: &str) -> Option<Version> {
        let (core, pre_release) = match text.split_once('-') {
            Some((core, pre_release)) => (core, Some(pre_release)),
            None => (text, None),
        };
        let mut numbers = Vec::new();
        for part in core.split('.') {
            if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            numbers.push(part.parse::<u64>().ok()?);
        }
        let core: [u64; 3] = numbers.try_into().ok()?;
        let mut parts = Vec::new();
        if let Some(pre_release) = pre_release {
            for part in pre_release.split('.') {
                if part.is_empty() || !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    return None;
                }
                parts.push(part.to_string());
            }
        }
        Some(Version {
            core,
            pre_release: parts,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre_release.is_empty(), other.pre_release.is_empty()) {
                (true, true) => Ordering::Equal,
                // A release is newer than any of its pre-releases.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre_release(&self.pre_release, &other.pre_release),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Semver's pre-release order: part by part, numbers as numbers and below
/// words, and a shorter list first when one is the start of the other.
fn compare_pre_release(a: &[String], b: &[String]) -> Ordering {
    for (left, right) in a.iter().zip(b.iter()) {
        let order = match (left.parse::<u64>(), right.parse::<u64>()) {
            (Ok(left), Ok(right)) => left.cmp(&right),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => left.cmp(right),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn keeps_the_newest_three_by_semver() {
        let versions = names(&[
            "1.0.0-beta.10",
            "1.0.0-beta.2",
            "1.0.0",
            "1.0.0-beta.9",
            "1.0.0-beta.1",
        ]);
        assert_eq!(
            to_delete(&versions, &names(&["1.0.0"]), 3),
            names(&["1.0.0-beta.1", "1.0.0-beta.2"])
        );
    }

    // A rollback points the feed at an older version: it stays.
    #[test]
    fn never_deletes_the_promoted_version() {
        let versions = names(&[
            "1.0.0-beta.1",
            "1.0.0-beta.2",
            "1.0.0-beta.3",
            "1.0.0-beta.4",
        ]);
        assert_eq!(
            to_delete(&versions, &names(&["1.0.0-beta.1"]), 3),
            Vec::<String>::new()
        );
        assert_eq!(to_delete(&versions, &[], 3), names(&["1.0.0-beta.1"]));
    }

    // Each feed (stable, edge, the legacy beta pointer) may point into any
    // channel's folder, so every one of their versions stays.
    #[test]
    fn never_deletes_a_version_any_feed_points_at() {
        let versions = names(&["1.0.0", "1.0.1", "1.0.2", "1.0.3", "1.0.4"]);
        assert_eq!(
            to_delete(&versions, &names(&["1.0.0", "1.0.1"]), 3),
            Vec::<String>::new()
        );
    }

    #[test]
    fn newer_follows_semver() {
        assert_eq!(newer("1.0.0", "1.0.0-beta.7"), Some(true));
        assert_eq!(newer("1.1.0-rc.1", "1.0.1"), Some(true));
        assert_eq!(newer("1.0.1", "1.1.0-rc.2"), Some(false));
        assert_eq!(newer("1.0.0", "1.0.0"), Some(false));
        assert_eq!(newer("1.0", "1.0.0"), None);
        assert_eq!(newer("1.0.0", "latest"), None);
    }

    #[test]
    fn names_that_are_not_versions_are_never_deleted() {
        let versions = names(&[
            "latest.json",
            "",
            "1.0",
            "1.0.0-",
            "x.y.z",
            "0.9.0",
            "1.0.0",
            "1.1.0",
            "1.2.0",
        ]);
        assert_eq!(to_delete(&versions, &[], 3), names(&["0.9.0"]));
    }

    #[test]
    fn fewer_than_keep_deletes_nothing() {
        assert!(to_delete(&names(&["1.0.0-beta.1", "1.0.0-beta.2"]), &[], 3).is_empty());
        assert!(to_delete(&[], &[], 3).is_empty());
    }

    #[test]
    fn pre_release_parts_follow_semver() {
        let order = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.10.0",
        ];
        for pair in order.windows(2) {
            let older = Version::parse(pair[0]).expect(pair[0]);
            let newer = Version::parse(pair[1]).expect(pair[1]);
            assert!(older < newer, "{} < {}", pair[0], pair[1]);
        }
    }
}
