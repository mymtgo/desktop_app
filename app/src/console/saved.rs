//! The saved queries file, `<app data>/console/queries.json`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::types::ConsoleParam;

/// One saved query; the name is its key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
    pub params: Vec<ConsoleParam>,
    /// When it was last saved, UTC unix milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
}

/// The saved queries file's name inside the console folder.
const FILE: &str = "queries.json";
/// Where an unreadable file is moved, so the user's queries are not lost.
const BAD_FILE: &str = "queries.json.bad";
/// The file being written, renamed over `FILE` once complete.
const TEMP_FILE: &str = "queries.json.tmp";

/// The saved queries file in the console folder `dir`.
pub fn file(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// The saved queries in `dir`, for showing: `load_strict`, with an
/// unreadable file shown as no queries (and left where it is).
pub fn load(dir: &Path) -> Vec<SavedQuery> {
    load_strict(dir).unwrap_or_else(|error| {
        tracing::warn!(%error, "the saved console queries could not be read");
        Vec::new()
    })
}

/// The saved queries in `dir`, for changing them. A missing file is no
/// queries. A file that is not valid JSON is moved to `queries.json.bad`
/// (replacing an older one) and the list starts empty, so the next save
/// does not overwrite it. Any other read error, or a bad file that cannot
/// be moved aside, is an error: the caller must not write over the file.
pub fn load_strict(dir: &Path) -> io::Result<Vec<SavedQuery>> {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    match serde_json::from_str(&text) {
        Ok(queries) => Ok(queries),
        Err(error) => {
            tracing::warn!(%error, "the saved console queries are not valid, so they were moved aside");
            // `rename` replaces an older bad file, on Windows too.
            std::fs::rename(&path, dir.join(BAD_FILE))?;
            Ok(Vec::new())
        }
    }
}

/// Writes `queries` to `dir` (made when missing): to a temp file first,
/// synced to disk, then renamed over the old file, so a crash never leaves
/// half a file.
pub fn store(dir: &Path, queries: &[SavedQuery]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string_pretty(queries).map_err(io::Error::other)?;
    let temp = dir.join(TEMP_FILE);
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, dir.join(FILE))
}

/// `list` with `query` added, or in place of the saved query of the same
/// name.
pub fn put(mut list: Vec<SavedQuery>, query: SavedQuery) -> Vec<SavedQuery> {
    match list.iter_mut().find(|saved| saved.name == query.name) {
        Some(saved) => *saved = query,
        None => list.push(query),
    }
    list
}

/// `list` without the saved query called `name`.
pub fn delete(mut list: Vec<SavedQuery>, name: &str) -> Vec<SavedQuery> {
    list.retain(|saved| saved.name != name);
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::testing::Scratch;

    fn query(name: &str, sql: &str) -> SavedQuery {
        SavedQuery {
            name: name.to_string(),
            sql: sql.to_string(),
            params: vec![ConsoleParam {
                name: "format".to_string(),
                value: Some("CMODERN".to_string()),
            }],
            updated_at: 1_700_000_000_000,
        }
    }

    #[test]
    fn round_trip() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("console");
        let queries = vec![
            query("Win rate", "SELECT 1"),
            query("Decks", "SELECT * FROM v_decks"),
        ];
        store(&dir, &queries).expect("store");
        assert!(dir.join("queries.json").exists());
        assert_eq!(load(&dir), queries);
    }

    #[test]
    fn missing_file_is_empty() {
        let scratch = Scratch::new();
        assert_eq!(load(&scratch.0.join("console")), Vec::new());
    }

    #[test]
    fn corrupt_file_is_kept_aside() {
        let scratch = Scratch::new();
        let dir = &scratch.0;
        std::fs::write(dir.join("queries.json.bad"), "older").unwrap();
        std::fs::write(dir.join("queries.json"), "{").unwrap();
        assert_eq!(load(dir), Vec::new());
        assert!(!dir.join("queries.json").exists(), "moved aside");
        assert_eq!(
            std::fs::read_to_string(dir.join("queries.json.bad")).unwrap(),
            "{",
            "the newer bad file replaces the older one"
        );
    }

    #[test]
    fn strict_load_refuses_an_unreadable_file() {
        let scratch = Scratch::new();
        let dir = &scratch.0;
        // A folder where the file should be cannot be read on any OS.
        std::fs::create_dir_all(dir.join("queries.json")).unwrap();
        assert!(load_strict(dir).is_err());
        assert_eq!(load(dir), Vec::new(), "the list still shows, empty");
        assert!(dir.join("queries.json").is_dir(), "left as it was");
    }

    #[test]
    fn strict_load_moves_a_bad_file_aside() {
        let scratch = Scratch::new();
        let dir = &scratch.0;
        std::fs::write(dir.join("queries.json"), "{").unwrap();
        assert_eq!(load_strict(dir).unwrap(), Vec::new());
        assert!(dir.join("queries.json.bad").exists());
    }

    #[test]
    fn put_replaces_by_name_and_delete_removes() {
        let list = put(Vec::new(), query("A", "SELECT 1"));
        let list = put(list, query("B", "SELECT 2"));
        let list = put(list, query("A", "SELECT 3"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "A");
        assert_eq!(list[0].sql, "SELECT 3", "replaced in place");
        let list = delete(list, "A");
        assert_eq!(list, vec![query("B", "SELECT 2")]);
        assert_eq!(delete(list.clone(), "missing"), list);
    }
}
