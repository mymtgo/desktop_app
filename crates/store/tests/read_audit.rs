//! The read audit (league self-management spec section 8): every read of
//! `matches` or `leagues` in `crates/store/src` and `app/src` goes through
//! `listed_matches` or `listed_leagues`, or is on `ALLOWED` with its reason.
//!
//! A read is SQL text `FROM <table>` or `JOIN <table>` (not `DELETE FROM`),
//! the words possibly on two lines or apart by a `\n` or `\t` escape, or a
//! string literal that starts with the table name and a space
//! (`"<table> m ..."`: a league read scope's `from:`, or a constant or `let`
//! interpolated after `FROM`). Comment lines, test modules (`#[cfg(test)]`
//! directly above a top-level `mod name {`, up to its `}` in column 0),
//! `tests.rs`, `*_tests.rs` and `test_support.rs` are skipped.
//!
//! `ALLOWED` counts the reads per file and table. A new read of the table
//! itself changes a count and fails the test: read the view instead, or
//! raise the count with a reason that covers the new read. A count that
//! drops fails too, so the list stays exact.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// (file from the workspace root, table, reads, why it reads the table
/// itself).
const ALLOWED: &[(&str, &str, usize, &str)] = &[
    (
        "app/src/legacy/import_scale.rs",
        "matches",
        1,
        "a test-only module (`#[cfg(test)] mod import_scale;` in legacy.rs)",
    ),
    (
        "crates/store/src/api_reads.rs",
        "leagues",
        1,
        "a match report names the MTGO league the match was played in",
    ),
    (
        "crates/store/src/api_reads/arena.rs",
        "leagues",
        2,
        "course deck by match id, and the run keys of one match by id",
    ),
    (
        "crates/store/src/api_reads/arena.rs",
        "matches",
        6,
        "Arena report queue's own rules, API reports by match id (rank and season at the match), and course deck by match id",
    ),
    (
        "crates/store/src/api_reads.rs",
        "matches",
        5,
        "API reports by match id, and the report queue's own rules",
    ),
    (
        "crates/store/src/archetype_edit.rs",
        "matches",
        5,
        "detection writes on one match by id, every stored format, and the commanders of one match by id",
    ),
    (
        "crates/store/src/archetype_edit/manual.rs",
        "matches",
        1,
        "one match by id while its archetype is set",
    ),
    (
        "crates/store/src/archetypes.rs",
        "matches",
        4,
        "detection input, deck and format of one match by id, and a pulled match's format by id for its commander archetypes",
    ),
    (
        "crates/store/src/arena_courses.rs",
        "matches",
        1,
        "the event names of an account's Arena matches by token, hidden ones too, for the Arena run fold (as the league sync reads)",
    ),
    (
        "crates/store/src/arena_games.rs",
        "matches",
        2,
        "Arena matches tracked before game messages were stored, hidden ones too, and whether one match by token is a pulled copy with a synced source",
    ),
    (
        "crates/store/src/arena_game_sync.rs",
        "matches",
        4,
        "the Arena game sync works on stored matches by id or token, hidden ones too, pulled \
         copies included",
    ),
    (
        "crates/store/src/match_commanders.rs",
        "matches",
        1,
        "the commander sync reads one match's stored format and deck version by id, hidden ones too, and asks its detection again",
    ),
    (
        "crates/store/src/arena_replay.rs",
        "matches",
        3,
        "Arena replays by game id or match token, as the MTGO replay reads, and every Arena match for the arena-mtgr dev command",
    ),
    (
        "crates/store/src/arena_run_sync.rs",
        "leagues",
        5,
        "the Arena run sync finds its own rows by run key, removed ones too, as the league sync does, and clears links into them; the arena-seed summary lists every `arena:` run; a pulled run's account is the one whose row holds its client id, removed ones too",
    ),
    (
        "crates/store/src/arena_run_sync.rs",
        "matches",
        5,
        "the Arena run sync clears the league and registered deck of hidden matches too, and finds a match by token; the arena-seed summary counts the completed ones of each run; a pulled run's account is the one of a pulled match naming it, hidden ones too",
    ),
    (
        "crates/store/src/card_stats.rs",
        "matches",
        1,
        "card stats regenerate hidden matches too (0.x did)",
    ),
    (
        "crates/store/src/cards.rs",
        "matches",
        1,
        "one log match by id",
    ),
    (
        "crates/store/src/dashboard.rs",
        "matches",
        2,
        "the outer read of rows chosen from listed_matches; decks of given tokens",
    ),
    (
        "crates/store/src/deck_index.rs",
        "matches",
        1,
        "decks of given match tokens",
    ),
    (
        "crates/store/src/decks.rs",
        "matches",
        5,
        "deck linking and version clean-up over every stored match; one link by token; \
         archiving an unused deck checks every stored match, hidden ones included",
    ),
    (
        "crates/store/src/decks.rs",
        "leagues",
        2,
        "archiving an unused deck checks every league on the deck or one of its versions",
    ),
    (
        "crates/store/src/deleted.rs",
        "leagues",
        3,
        "the Deleted list reads exactly the removed leagues, names the league a deleted match \
         was deleted from (deleted_from_league_id) and the league it was deleted together with \
         (issue #418)",
    ),
    (
        "crates/store/src/deleted.rs",
        "matches",
        3,
        "the Deleted lists read exactly the hidden matches and a deleted league's record",
    ),
    (
        "crates/store/src/endstep_replay.rs",
        "matches",
        3,
        "Endstep replays by game id or match token, as the Arena replay reads, and every Endstep match for the endstep-mtgr dev command",
    ),
    (
        "crates/store/src/endstep_sync.rs",
        "matches",
        8,
        "a ranked result's match token, by match id; the Endstep matches still in progress, hidden ones too, for the tracker's quiet refold; the match a game rows write names by token, hidden ones too; a pulled copy by token or id (its synced source, its game rows and the account and opponent its fold names), hidden ones too; every pulled copy, hidden ones too, derived again on a sync version raise",
    ),
    (
        "crates/store/src/sync_sources.rs",
        "matches",
        1,
        "the source of one match by id for its bundle, hidden ones too",
    ),
    (
        "crates/store/src/game_sync.rs",
        "matches",
        4,
        "game sync works on stored matches by id or token, hidden ones too",
    ),
    (
        "crates/store/src/history_check.rs",
        "matches",
        1,
        "MTGO's verdict checks every stored log match by its MTGO id, hidden ones too",
    ),
    (
        "crates/store/src/history_import.rs",
        "leagues",
        1,
        "Delete all imported keeps a history row a league holds (import bookkeeping)",
    ),
    (
        "crates/store/src/history_import.rs",
        "matches",
        7,
        "the history import's duplicate checks and Delete all imported: every stored row",
    ),
    (
        "crates/store/src/import.rs",
        "leagues",
        3,
        "a pull finds its league row by its stored sync id, else by token and start; the \
         account clean-up checks every row",
    ),
    (
        "crates/store/src/import.rs",
        "matches",
        12,
        "imports and pulls find and update rows whatever the player did with them (spec 4.6), a pulled copy's league link (spec 6.3), and the account a pulled Arena or Endstep deck is filed under (cross-device sync spec 7)",
    ),
    (
        "crates/store/src/import_api.rs",
        "matches",
        1,
        "the card stats import by token",
    ),
    (
        "crates/store/src/import_archetypes.rs",
        "matches",
        6,
        "the 0.x archetype import finds rows by token or id",
    ),
    (
        "crates/store/src/import_decks.rs",
        "matches",
        2,
        "the 0.x deck links find rows by token",
    ),
    (
        "crates/store/src/import_leagues.rs",
        "leagues",
        11,
        "the 0.x league import finds its rows, removed ones too (spec 4.6), and a pulled run \
         by its 0.x id alone",
    ),
    (
        "crates/store/src/import_leagues.rs",
        "matches",
        5,
        "the 0.x league import's member checks and the player's marks on a member (spec 4.6)",
    ),
    (
        "crates/store/src/import_limited.rs",
        "leagues",
        1,
        "the 0.x limited import finds its run rows",
    ),
    (
        "crates/store/src/import_limited.rs",
        "matches",
        1,
        "the 0.x limited import finds matches by token",
    ),
    (
        "crates/store/src/imported_detail.rs",
        "matches",
        4,
        "the imported postboard rule by match id or token",
    ),
    (
        "crates/store/src/league_sync.rs",
        "leagues",
        8,
        "the league sync folds and carries every row: a removed row is input (spec 4.4); a \
         merge reads both rows' sync ids and whether the winner is a 0.x row",
    ),
    (
        "crates/store/src/league_sync.rs",
        "matches",
        8,
        "the league sync places and snapshots stored matches by token or id, and finds the \
         rows a match is linked to by hand (Task 3, `linked_rows`)",
    ),
    (
        "crates/store/src/leagues.rs",
        "leagues",
        12,
        "edits find removed rows to refuse, undo (by row id) or restore them, a moved match's league \
         owner (in both move candidate reads) and the next manual id; notes and drops by key; \
         the removed league a hidden match was deleted with (Restore, Undo, issue #418); \
         the Arena run guard reads an undone row's key and a match's run key, listed or not",
    ),
    (
        "crates/store/src/leagues.rs",
        "matches",
        6,
        "notes by match token; tournament rows of any match (members come from listed_matches); \
         the hidden matches deleted with a removed league (Restore, Undo, issue #418); \
         DERIVED_CANDIDATE_SQL judges a hidden match as listed for a pulled league link; \
         the Arena run guard refuses moving any match out of an Arena run, hidden or not",
    ),
    (
        "crates/store/src/limited.rs",
        "matches",
        1,
        "one deck snapshot by match token",
    ),
    (
        "crates/store/src/manual.rs",
        "leagues",
        2,
        "the version of a manual league check_manual_league found in listed_leagues, and the \
         owner of a corrected match's league, synced even when the league is removed",
    ),
    (
        "crates/store/src/manual.rs",
        "matches",
        8,
        "the manual match sheet reads one match by token or id, and a pushed manual match's \
         platform and opponent name by token",
    ),
    (
        "crates/store/src/match_edits.rs",
        "leagues",
        1,
        "Restore checks whether the league a match was deleted from (deleted_from_league_id) \
         is deleted (removed rows by design)",
    ),
    (
        "crates/store/src/match_edits.rs",
        "matches",
        9,
        "match edits, the hidden checks and the restore rule (unhide) read hidden rows by design",
    ),
    (
        "crates/store/src/overlay.rs",
        "matches",
        1,
        "live game inputs by token",
    ),
    (
        "crates/store/src/platform.rs",
        "matches",
        1,
        "a match's platform by token, hidden ones too (no MTGO folder search for Arena or Endstep)",
    ),
    (
        "crates/store/src/read.rs",
        "leagues",
        2,
        "the owner of a run key for the tracker, removed runs too (Restore); the run key a \
         removed row has now, for Undo by row id",
    ),
    (
        "crates/store/src/read.rs",
        "matches",
        18,
        "tracker and projection bookkeeping by token, id or source: every stored row",
    ),
    (
        "crates/store/src/replay.rs",
        "matches",
        4,
        "replays by match or game id",
    ),
    (
        "crates/store/src/sync.rs",
        "leagues",
        13,
        "sync pushes and pulls every row; a deletion travels in its bundle (spec 6); a manual \
         league is found by its stored sync id, else by token and start; an Arena course run's \
         bundle, and whether a league is an Arena run, read its row by id, removed ones too; a \
         pulled Arena run's player fields are weighed against its row, removed ones too",
    ),
    (
        "crates/store/src/sync.rs",
        "matches",
        14,
        "sync pushes and pulls every row, hidden ones too; a pulled Arena match's run and account \
         by token, hidden ones too",
    ),
    (
        "crates/store/src/sync_player.rs",
        "leagues",
        3,
        "sync: player fields by client id, every row (removed and detached ones too)",
    ),
    (
        "crates/store/src/sync_player.rs",
        "matches",
        2,
        "sync: player fields and website tombstones by client id, every row (hidden ones too)",
    ),
    (
        "crates/store/src/tournament_sync.rs",
        "matches",
        1,
        "the event overlay's own matches of a tournament: a hidden or detached match is still over for its round",
    ),
    (
        "crates/store/src/write.rs",
        "matches",
        17,
        "the projection's writes and the hand-over of a copy's records, its hide with the league \
         it was deleted from, its league choice, whether the player corrected it, and its fix, \
         and the Arena game sync's match by token and whether it derived an Arena game",
    ),
];

const TABLES: [&str; 2] = ["matches", "leagues"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs` file under `dir`, in path order.
fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("a source folder")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

fn skipped_file(name: &str) -> bool {
    name == "test_support.rs" || name == "tests.rs" || name.ends_with("_tests.rs")
}

fn is_module_start(line: &str) -> bool {
    let module = line.starts_with("mod ")
        || line.starts_with("pub mod ")
        || line.starts_with("pub(crate) mod ");
    module && line.ends_with(" {")
}

/// The file's lines with comment lines and test modules blanked (kept as
/// empty lines, so line numbers hold).
fn code_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_test_module = false;
    let mut previous = "";
    for line in text.lines() {
        if in_test_module {
            if line == "}" {
                in_test_module = false;
            }
            lines.push(String::new());
        } else if previous == "#[cfg(test)]" && is_module_start(line) {
            // The attribute must be on the line directly above `mod`: one
            // with a doc comment or another attribute between them is read.
            in_test_module = true;
            lines.push(String::new());
        } else if line.trim_start().starts_with("//") {
            lines.push(String::new());
        } else {
            lines.push(line.to_string());
        }
        previous = line;
    }
    lines
}

/// The leading table name of a word (`leagues)",` is `leagues`).
fn table_of(word: &str) -> String {
    word.trim_start_matches('"')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// Each read's line and table.
fn reads(lines: &[String]) -> Vec<(usize, String)> {
    let mut words: Vec<(usize, String)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        // A `\n` or `\t` escape in a string literal separates words too
        // (`"\nJOIN matches m"`).
        let spaced = line.replace("\\n", " ").replace("\\t", " ");
        for word in spaced.split_whitespace() {
            words.push((index + 1, word.to_string()));
        }
    }
    let mut found = Vec::new();
    for at in 1..words.len() {
        let table = table_of(&words[at].1);
        if !TABLES.contains(&table.as_str()) {
            continue;
        }
        let keyword = words[at - 1].1.trim_start_matches(['"', '(']);
        let deleted = at >= 2 && words[at - 2].1.ends_with("DELETE");
        let sql_read = (keyword == "FROM" || keyword == "JOIN") && !deleted;
        // `"matches m ..."`: a scope's `from:`, or a table held in a
        // constant or `let` and interpolated after `FROM` or `JOIN`.
        let literal_read = words[at].1 == format!("\"{table}");
        if sql_read || literal_read {
            found.push((words[at].0, table));
        }
    }
    found
}

#[test]
fn the_scanner_finds_reads_and_skips_writes_comments_and_tests() {
    let sample = "fn a() {
    conn.execute(\"SELECT id FROM leagues WHERE x\", []);
    // FROM matches in a comment
    tx.execute(\"DELETE FROM leagues WHERE id = 1\", []);
    let s = \"SELECT * FROM listed_leagues l JOIN
        matches m ON m.id = l.id\";
}
const S: Scope = Scope { from: \"leagues l\" };
#[cfg(test)]
mod tests {
    const T: &str = \"FROM matches\";
}
const U: &str = \"(SELECT id FROM matches)\";
const M: &str = \"matches m JOIN accounts a ON a.id = m.account_id\";
let names = [\"leagues\", \"matches\"];
sql.push_str(\"\\nJOIN matches m ON m.id = g.match_id\");
sql.push_str(\"\\tFROM leagues l\");
";
    assert_eq!(
        reads(&code_lines(sample)),
        vec![
            (2, "leagues".to_string()),
            (6, "matches".to_string()),
            (8, "leagues".to_string()),
            (13, "matches".to_string()),
            (14, "matches".to_string()),
            (16, "matches".to_string()),
            (17, "leagues".to_string()),
        ]
    );
}

#[test]
fn every_read_of_matches_or_leagues_uses_its_view_or_is_allowed() {
    let root = workspace_root();
    let mut counted: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for dir in ["crates/store/src", "app/src"] {
        let mut files = Vec::new();
        rust_files(&root.join(dir), &mut files);
        for path in files {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if skipped_file(name) {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file");
            let relative = path
                .strip_prefix(&root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            for (line, table) in reads(&code_lines(&text)) {
                counted
                    .entry((relative.clone(), table))
                    .or_default()
                    .push(line);
            }
        }
    }
    let mut problems = Vec::new();
    for (file, table, allowed, reason) in ALLOWED {
        assert!(!reason.is_empty(), "{file}: a reason");
        let lines = counted
            .remove(&(file.to_string(), table.to_string()))
            .unwrap_or_default();
        if lines.len() != *allowed {
            problems.push(format!(
                "{file}: {} reads of `{table}` at lines {lines:?}, allowed {allowed}",
                lines.len()
            ));
        }
    }
    for ((file, table), lines) in counted {
        problems.push(format!(
            "{file}: reads `{table}` itself at lines {lines:?}: read `listed_{table}`, \
             or allow it with a reason"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
