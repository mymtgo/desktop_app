//! Reading MTGO's deck files into decks and versions (decks spec 3; 0.x
//! `SyncDecks` and `AdoptOrphanDecks`).
//!
//! The engine reads the active account's deck files once it has caught up after
//! opening, every `DECK_SYNC_EVERY_MS`, after a new local account logs in, and
//! when asked (`request_deck_sync`, at most once a minute, or
//! `sync_deck_files` directly). Each changed file becomes one
//! `DeckWrite::SyncFile` in the normal batch.

use std::fs;
use std::path::Path;

use decks::DeckFile;
use mtgo_log::deck_files::{deck_files, deck_folder};
use store::Platform;
use store::decks::{DeckWrite, deck_by_mtgo_id};

use super::Engine;
use crate::TrackerError;

/// How often the deck files are read (0.x polls every five minutes).
pub const DECK_SYNC_EVERY_MS: i64 = 300_000;
/// `request_deck_sync` asks for a sync at most this often.
pub const DECK_REQUEST_EVERY_MS: i64 = 60_000;

impl Engine {
    /// Is a deck sync due at `now_ms`: never synced yet, asked for, or the
    /// last one is `DECK_SYNC_EVERY_MS` old.
    pub(super) fn deck_sync_due(&self, now_ms: i64) -> bool {
        self.deck_sync_wanted
            || self
                .last_deck_sync_ms
                .is_none_or(|at| now_ms - at >= DECK_SYNC_EVERY_MS)
    }

    /// Ask for a deck sync at the next caught-up tick (a match whose deck
    /// cannot be linked). Honoured at most once per `DECK_REQUEST_EVERY_MS`;
    /// returns whether this request was.
    pub fn request_deck_sync(&mut self, now_ms: i64) -> bool {
        if self
            .last_deck_request_ms
            .is_some_and(|at| now_ms - at < DECK_REQUEST_EVERY_MS)
        {
            return false;
        }
        self.last_deck_request_ms = Some(now_ms);
        self.deck_sync_wanted = true;
        true
    }

    /// Read the active account's deck files now and queue a write for each
    /// deck that changes. Returns how many decks a changed file updates (an
    /// unchanged file giving its deck the local account is not counted). The
    /// writes go in the pending batch; `flush` commits them.
    pub fn sync_deck_files(&mut self, now_ms: i64) -> Result<u32, TrackerError> {
        self.last_deck_sync_ms = Some(now_ms);
        self.deck_sync_wanted = false;
        // Unlinked recent matches are tried again at the next link phase.
        self.links.synced();
        let Some(folder) = deck_folder(&self.env, self.config.mtgo_data_folder.as_deref()) else {
            return Ok(0);
        };
        let paths = deck_files(&folder);
        if paths.is_empty() {
            return Ok(0);
        }
        // The stored decks must include every write sent so far.
        self.flush()?;
        let account = self.local_account();
        let mut written = 0;
        let mut adopted = false;
        for path in paths {
            let Some(file) = self.read_deck_file(&path) else {
                continue;
            };
            if let Some(write) = self.deck_write(file, account.as_deref(), now_ms)? {
                match &write {
                    DeckWrite::Adopt { .. } => adopted = true,
                    DeckWrite::SyncFile { file, .. } => {
                        written += 1;
                        self.synced_deck_ids.push(file.net_deck_id.clone());
                    }
                    _ => written += 1,
                }
                self.batch.decks.push(write);
            }
        }
        if written > 0 || adopted {
            self.decks_changed = true;
            self.links.candidates_changed();
        }
        Ok(written)
    }

    /// The write one parsed deck file needs, if any. The stored
    /// `file_synced_at` is checked before anything is computed.
    fn deck_write(
        &self,
        file: DeckFile,
        account: Option<&str>,
        now_ms: i64,
    ) -> Result<Option<DeckWrite>, TrackerError> {
        let stored = deck_by_mtgo_id(&self.reader, &file.net_deck_id)?;
        if let Some(deck) = &stored
            && deck
                .file_synced_at
                .is_some_and(|synced| synced >= file.timestamp_ms)
        {
            // Unchanged. 0.x still gives a deck without an account the local one.
            return Ok(match (deck.account_id, account) {
                (None, Some(name)) => Some(DeckWrite::Adopt {
                    mtgo_id: file.net_deck_id,
                    account: name.to_string(),
                    at_ms: now_ms,
                }),
                _ => None,
            });
        }
        let signature = decks::signature(&file.cards);
        let identity = super::deck_use::identity_update(&self.reader, &file.cards)?;
        Ok(Some(DeckWrite::SyncFile {
            file,
            signature,
            account: account.map(str::to_string),
            platform: Platform::Mtgo,
            synced_at_ms: now_ms,
            identity,
        }))
    }

    /// The local account deck files belong to: the local username of the log
    /// that grew most recently (the first such log when none grew live yet),
    /// else the only known account, else none.
    fn local_account(&self) -> Option<String> {
        let mut best: Option<(Option<i64>, String)> = None;
        for file in &self.files {
            let Some(name) = file.ctx.local_username() else {
                continue;
            };
            let grown = file.tailer.last_live_growth_ms();
            if best
                .as_ref()
                .is_none_or(|(best_grown, _)| grown > *best_grown)
            {
                best = Some((grown, name.to_string()));
            }
        }
        if let Some((_, name)) = best {
            return Some(name);
        }
        match self.projector.known_accounts() {
            [only] => Some(only.clone()),
            _ => None,
        }
    }

    /// Read and parse one deck file. `None` (logged once per file) when it
    /// cannot be read, does not parse, or is not a deck grouping.
    fn read_deck_file(&mut self, path: &Path) -> Option<DeckFile> {
        // A file over the deck size limit is never loaded (it would not
        // parse anyway): a crafted huge file in the deck folder cannot
        // exhaust memory.
        if fs::metadata(path).is_ok_and(|meta| meta.len() > decks::MAX_DEK_BYTES as u64) {
            if self.warn_unreadable(path) {
                tracing::warn!(path = %path.display(), "a deck file is too large to read");
            }
            return None;
        }
        let parsed = match fs::read(path) {
            // 0.x reads the Timestamp in the app timezone, which is UTC.
            Ok(bytes) => decks::parse_deck_file(&deck_file_text(&bytes), chrono_tz::UTC),
            Err(error) => {
                if self.warn_unreadable(path) {
                    tracing::warn!(path = %path.display(), %error, "could not read a deck file");
                } else {
                    tracing::debug!(path = %path.display(), %error, "could not read a deck file");
                }
                return None;
            }
        };
        match parsed {
            Some(file) => {
                self.skipped_deck_files.remove(path);
                Some(file)
            }
            None => {
                if self.skipped_deck_files.insert(path.to_path_buf()) {
                    tracing::info!(
                        path = %path.display(),
                        "skipping a grouping file that is not a readable deck"
                    );
                }
                None
            }
        }
    }

    /// Whether a read failure of this file is the first this session: MTGO
    /// can hold a file locked while it writes it, which would otherwise warn
    /// at every sync.
    fn warn_unreadable(&mut self, path: &Path) -> bool {
        self.unreadable_deck_files.insert(path.to_path_buf())
    }
}

/// The text of a deck file's bytes, by its byte order mark: UTF-8 with a BOM,
/// UTF-16 little or big endian with a BOM, else UTF-8. The BOM is dropped.
/// Invalid sequences become U+FFFD, and an odd last byte of UTF-16 is
/// ignored. No real sample shows which encoding MTGO writes; 0.x's simplexml
/// reads all of these.
fn deck_file_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16_text(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16_text(rest, true);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// UTF-16 bytes (after the BOM) as text.
fn utf16_text(bytes: &[u8], big_endian: bool) -> String {
    // `as_chunks` gives whole pairs and drops an odd last byte.
    let (pairs, _) = bytes.as_chunks::<2>();
    let mut units = Vec::with_capacity(pairs.len());
    for pair in pairs {
        units.push(if big_endian {
            u16::from_be_bytes(*pair)
        } else {
            u16::from_le_bytes(*pair)
        });
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackerConfig;
    use crate::engine::test_support::open;
    use fixture_tools::TestDir;
    use std::fs::File;
    use std::time::{Duration, SystemTime};
    use store::decks::{DeckEdit, DeckRow, EditOutcome, deck_by_mtgo_id, versions_of};

    const DECK_1: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    const DECK_2: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    const OLD_ACCOUNT_DECK: &str = "2a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    const NOT_A_DECK: &str = "3a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    const BROKEN: &str = "4a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    /// 2026-07-01T12:00:00Z
    const NOW: i64 = 1_782_907_200_000;
    const LOGIN: &[u8] =
        b"12:00:00 [INF] (Login|MtGO Login Success) Username: Qlocal (8)\n12:00:01 [INF] (UI|after)\n";

    fn config(dir: &TestDir) -> TrackerConfig {
        TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: None,
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        }
    }

    /// Three ticks a second apart (reading the whole log) and commit: not
    /// `run_until_settled`, these tests time the deck syncs from `now_ms`.
    fn run_three_ticks(engine: &mut Engine, now_ms: i64) {
        for step in 0..3 {
            engine.tick(now_ms + step * 1_000).expect("tick");
        }
        engine.flush().expect("flush");
    }

    /// A deck file. `cards` are `(catalog id, quantity, sideboard)`.
    fn deck_xml(id: &str, name: &str, timestamp: &str, cards: &[(u32, u32, bool)]) -> String {
        grouping_xml(id, name, "Deck", timestamp, cards)
    }

    fn grouping_xml(
        id: &str,
        name: &str,
        kind: &str,
        timestamp: &str,
        cards: &[(u32, u32, bool)],
    ) -> String {
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<Grouping Name=\"{name}\" NetDeckId=\"{id}\" \
             GroupingType=\"{kind}\" Timestamp=\"{timestamp}\" FormatCode=\"CMODERN\">\r\n"
        );
        for (catalog_id, quantity, sideboard) in cards {
            let side = if *sideboard { "True" } else { "False" };
            xml.push_str(&format!(
                "  <Item CatId=\"{catalog_id}\" Quantity=\"{quantity}\" IsSideboard=\"{side}\" />\r\n"
            ));
        }
        xml.push_str("</Grouping>");
        xml
    }

    fn deck_path(account: &str, id: &str) -> String {
        format!("mtgo-data/{account}/grouping {id}.xml")
    }

    /// An account folder whose `user_settings` was last written `minutes_ago`.
    fn account_folder(dir: &TestDir, folder: &str, minutes_ago: u64) {
        let path = dir.write(&format!("mtgo-data/{folder}/user_settings"), b"x");
        let at = SystemTime::now() - Duration::from_secs(minutes_ago * 60);
        File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(at)
            .expect("set modified");
    }

    const ACCOUNT: &str = "Q1/A1/acct-new";
    const MAIN: [(u32, u32, bool); 2] = [(1001, 4, false), (1002, 2, true)];

    fn deck(config: &TrackerConfig, id: &str) -> Option<DeckRow> {
        let reader = store::open_reader(&config.db_path).expect("reader");
        deck_by_mtgo_id(&reader, id).expect("deck")
    }

    fn signatures(config: &TrackerConfig, id: &str) -> Vec<String> {
        let reader = store::open_reader(&config.db_path).expect("reader");
        let deck = deck_by_mtgo_id(&reader, id).expect("deck").expect("exists");
        versions_of(&reader, deck.id)
            .expect("versions")
            .into_iter()
            .map(|version| version.signature)
            .collect()
    }

    fn account_name(config: &TrackerConfig, account_id: Option<i64>) -> Option<String> {
        let reader = store::open_reader(&config.db_path).expect("reader");
        let id = account_id?;
        Some(
            reader
                .query_row("SELECT username FROM accounts WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .expect("account"),
        )
    }

    fn card_rows(config: &TrackerConfig) {
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, type, color_identity)
                 VALUES (1001, 'Qcard', 'Creature', 'U'), (1002, 'Qland', 'Land', '');",
        )
        .expect("cards");
    }

    fn utf16(text: &str, big_endian: bool) -> Vec<u8> {
        let mut bytes = if big_endian {
            vec![0xFE, 0xFF]
        } else {
            vec![0xFF, 0xFE]
        };
        for unit in text.encode_utf16() {
            let pair = if big_endian {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            };
            bytes.extend_from_slice(&pair);
        }
        bytes
    }

    #[test]
    fn deck_file_bytes_are_read_by_their_byte_order_mark() {
        let xml = deck_xml(DECK_1, "Qdeck \u{e9}", "2026-06-30T10:00:00", &MAIN);
        let mut utf8_bom = vec![0xEF, 0xBB, 0xBF];
        utf8_bom.extend_from_slice(xml.as_bytes());
        let mut odd = utf16(&xml, false);
        odd.push(0x41);
        for (label, bytes) in [
            ("UTF-8", xml.as_bytes().to_vec()),
            ("UTF-8 with BOM", utf8_bom),
            ("UTF-16 LE", utf16(&xml, false)),
            ("UTF-16 BE", utf16(&xml, true)),
            ("UTF-16 LE, odd last byte", odd),
        ] {
            let text = deck_file_text(&bytes);
            assert!(text.starts_with("<?xml"), "{label}: {text:?}");
            let file = decks::parse_deck_file(&text, chrono_tz::UTC)
                .unwrap_or_else(|| panic!("{label} parses"));
            assert_eq!(file.name, "Qdeck \u{e9}", "{label}");
            assert_eq!(file.cards.len(), 2, "{label}");
        }
    }

    #[test]
    fn a_utf16_deck_file_becomes_a_deck() {
        let dir = TestDir::new("decks-sync-utf16");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let xml =
            deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN).replace("utf-8", "utf-16");
        dir.write(&deck_path(ACCOUNT, DECK_1), &utf16(&xml, false));
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        assert_eq!(deck(&config, DECK_1).expect("synced").name, "Qdeck");
    }

    #[test]
    fn the_newest_account_folders_deck_files_become_decks_on_open() {
        let dir = TestDir::new("decks-sync-open");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, "Q0/A0/acct-old", 120);
        account_folder(&dir, ACCOUNT, 5);
        let old = deck_xml(OLD_ACCOUNT_DECK, "Qold", "2026-06-01T10:00:00", &MAIN);
        dir.write(
            &deck_path("Q0/A0/acct-old", OLD_ACCOUNT_DECK),
            old.as_bytes(),
        );
        let first = deck_xml(DECK_1, "Qdeck &amp; co", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let empty = deck_xml(DECK_2, "Qempty", "2026-06-30T11:00:00", &[]);
        dir.write(&deck_path(ACCOUNT, DECK_2), empty.as_bytes());
        let binder = grouping_xml(
            NOT_A_DECK,
            "Qbinder",
            "Binder",
            "2026-06-30T10:00:00",
            &MAIN,
        );
        dir.write(&deck_path(ACCOUNT, NOT_A_DECK), binder.as_bytes());
        dir.write(&deck_path(ACCOUNT, BROKEN), b"<Grouping Name=\"Qbroken\"");
        dir.write(&format!("mtgo-data/{ACCOUNT}/notes.xml"), first.as_bytes());

        let config = config(&dir);
        // Card data from a 0.x import, before the tracker opens.
        drop(store::open_writer(&config.db_path).expect("migrate"));
        card_rows(&config);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);

        let synced = deck(&config, DECK_1).expect("the deck file became a deck");
        assert_eq!(synced.name, "Qdeck & co");
        assert_eq!(synced.format, "CMODERN");
        assert_eq!(synced.source, "file");
        assert_eq!(synced.file_synced_at, Some(1_782_813_600_000));
        assert_eq!(
            account_name(&config, synced.account_id).as_deref(),
            Some("Qlocal"),
            "the log's local account"
        );
        assert_eq!(
            synced.color_identity.as_deref(),
            Some("U"),
            "computed from the card data"
        );
        let cards: Vec<decks::DeckCard> = MAIN
            .iter()
            .map(|(catalog_id, quantity, sideboard)| decks::DeckCard {
                catalog_id: *catalog_id,
                quantity: *quantity,
                sideboard: *sideboard,
            })
            .collect();
        assert_eq!(signatures(&config, DECK_1), vec![decks::signature(&cards)]);

        let empty = deck(&config, DECK_2).expect("a deck without cards is still a deck");
        assert_eq!(empty.color_identity, None);
        assert_eq!(signatures(&config, DECK_2).len(), 1);

        assert!(
            deck(&config, OLD_ACCOUNT_DECK).is_none(),
            "an older account folder"
        );
        assert!(deck(&config, NOT_A_DECK).is_none(), "not a deck grouping");
        assert!(deck(&config, BROKEN).is_none(), "does not parse");
    }

    #[test]
    fn an_unchanged_file_is_skipped_a_newer_one_updates_and_a_removed_one_keeps_its_deck() {
        let dir = TestDir::new("decks-sync-changes");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let second = deck_xml(DECK_2, "Qsecond", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_2), second.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);

        let id = deck(&config, DECK_1).expect("synced").id;
        engine
            .writer
            .edit_deck(DeckEdit::Rename {
                deck_id: id,
                name: "Qrenamed".to_string(),
            })
            .expect("rename");
        assert_eq!(
            engine.sync_deck_files(NOW + 10_000).expect("sync"),
            0,
            "unchanged files write nothing"
        );

        let edited = deck_xml(
            DECK_1,
            "Qnew name",
            "2026-06-30T12:00:00",
            &[(1003, 4, false)],
        );
        dir.write(&deck_path(ACCOUNT, DECK_1), edited.as_bytes());
        std::fs::remove_file(dir.join(&deck_path(ACCOUNT, DECK_2))).expect("remove");
        assert_eq!(engine.sync_deck_files(NOW + 20_000).expect("sync"), 1);
        engine.flush().expect("flush");

        let changed = deck(&config, DECK_1).expect("deck");
        assert_eq!(changed.name, "Qrenamed", "a renamed deck keeps its name");
        assert_eq!(changed.original_name.as_deref(), Some("Qdeck"));
        assert_eq!(changed.file_synced_at, Some(1_782_820_800_000));
        let new_cards = [decks::DeckCard {
            catalog_id: 1003,
            quantity: 4,
            sideboard: false,
        }];
        assert_eq!(
            signatures(&config, DECK_1),
            vec![decks::signature(&new_cards)],
            "the unused old version is pruned"
        );
        assert!(
            deck(&config, DECK_2).is_some(),
            "a deck is never deleted because its file is gone"
        );
    }

    /// 07 spec section 6: `cloud_sync_enabled` mirrors the server's slot
    /// ledger and only the slot rules and the toggle write it.
    #[test]
    fn the_deck_file_sync_never_writes_cloud_sync_enabled() {
        let dir = TestDir::new("decks-sync-cloud-flag");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        let flag = |enabled: Option<bool>| {
            let conn = store::open_writer(&config.db_path).expect("writer");
            if let Some(enabled) = enabled {
                conn.execute(
                    "UPDATE decks SET cloud_sync_enabled = ?2 WHERE mtgo_id = ?1",
                    store::params![DECK_1, enabled],
                )
                .expect("flag");
            }
            conn.query_row(
                "SELECT cloud_sync_enabled FROM decks WHERE mtgo_id = ?1",
                [DECK_1],
                |row| row.get::<_, bool>(0),
            )
            .expect("read flag")
        };
        assert!(!flag(None), "a new deck starts off");
        assert!(flag(Some(true)));

        let edited = deck_xml(DECK_1, "Qnew", "2026-06-30T12:00:00", &[(1003, 4, false)]);
        dir.write(&deck_path(ACCOUNT, DECK_1), edited.as_bytes());
        assert_eq!(engine.sync_deck_files(NOW + 20_000).expect("sync"), 1);
        engine.flush().expect("flush");
        assert!(flag(None), "a changed file keeps the flag on");

        flag(Some(false));
        let again = deck_xml(DECK_1, "Qnewer", "2026-06-30T14:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), again.as_bytes());
        assert_eq!(engine.sync_deck_files(NOW + 40_000).expect("sync"), 1);
        engine.flush().expect("flush");
        assert!(!flag(None), "and off");
    }

    #[test]
    fn a_file_without_cards_keeps_a_manual_identity_and_one_with_cards_recomputes_it() {
        let dir = TestDir::new("decks-sync-identity");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        let id = deck(&config, DECK_1).expect("synced").id;
        engine
            .writer
            .edit_deck(DeckEdit::SetIdentity {
                deck_id: id,
                identity: Some("W,B".to_string()),
            })
            .expect("manual identity");

        let emptied = deck_xml(DECK_1, "Qdeck", "2026-06-30T11:00:00", &[]);
        dir.write(&deck_path(ACCOUNT, DECK_1), emptied.as_bytes());
        engine.sync_deck_files(NOW + 10_000).expect("sync");
        engine.flush().expect("flush");
        let kept = deck(&config, DECK_1).expect("deck");
        assert_eq!(kept.color_identity.as_deref(), Some("W,B"));
        assert!(kept.color_identity_manual, "still a manual identity");

        // No card data: no colour reaches 4, so 0.x stores NULL.
        let refilled = deck_xml(DECK_1, "Qdeck", "2026-06-30T12:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), refilled.as_bytes());
        engine.sync_deck_files(NOW + 20_000).expect("sync");
        engine.flush().expect("flush");
        let recomputed = deck(&config, DECK_1).expect("deck");
        assert_eq!(recomputed.color_identity, None);
        assert!(!recomputed.color_identity_manual, "computed again");
    }

    #[test]
    fn decks_sync_on_open_every_five_minutes_and_on_a_rate_limited_request() {
        let dir = TestDir::new("decks-sync-when");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        assert!(deck(&config, DECK_1).is_some(), "synced on open");

        let second = deck_xml(DECK_2, "Qsecond", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_2), second.as_bytes());
        run_three_ticks(&mut engine, NOW + 10_000);
        assert!(deck(&config, DECK_2).is_none(), "not due yet");

        let requested = NOW + 20_000;
        assert!(engine.request_deck_sync(requested));
        run_three_ticks(&mut engine, requested);
        assert!(
            deck(&config, DECK_2).is_some(),
            "a request syncs at the next tick"
        );
        assert!(
            !engine.request_deck_sync(requested + DECK_REQUEST_EVERY_MS - 1),
            "at most one request a minute"
        );

        let edited = deck_xml(DECK_1, "Qdeck", "2026-06-30T11:00:00", &[(1003, 4, false)]);
        dir.write(&deck_path(ACCOUNT, DECK_1), edited.as_bytes());
        let before = deck(&config, DECK_1).expect("deck").file_synced_at;
        engine
            .tick(requested + DECK_SYNC_EVERY_MS - 1)
            .expect("tick");
        engine.flush().expect("flush");
        assert_eq!(deck(&config, DECK_1).expect("deck").file_synced_at, before);
        engine.tick(requested + DECK_SYNC_EVERY_MS).expect("tick");
        engine.flush().expect("flush");
        assert_eq!(
            deck(&config, DECK_1).expect("deck").file_synced_at,
            Some(1_782_817_200_000),
            "the five-minute sync read the edit"
        );
        assert!(engine.request_deck_sync(requested + DECK_REQUEST_EVERY_MS));
    }

    #[test]
    fn a_deck_synced_without_an_account_gets_the_first_login_without_a_file_change() {
        let dir = TestDir::new("decks-sync-adopt");
        dir.write("logs/mtgo.log", b"12:00:00 [INF] (UI|before login)\n");
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        assert_eq!(deck(&config, DECK_1).expect("synced").account_id, None);

        dir.append("logs/mtgo.log", LOGIN);
        run_three_ticks(&mut engine, NOW + 10_000);
        let adopted = deck(&config, DECK_1).expect("deck");
        assert_eq!(
            account_name(&config, adopted.account_id).as_deref(),
            Some("Qlocal")
        );
        assert_eq!(
            adopted.file_synced_at,
            Some(1_782_813_600_000),
            "the file did not change"
        );
    }

    #[test]
    fn an_unchanged_file_gives_its_accountless_deck_the_local_account() {
        let dir = TestDir::new("decks-sync-backfill");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        // Two accounts, so the first-account rule adopts nothing, and a deck
        // already stored without one at this file's timestamp.
        {
            let mut conn = store::open_writer(&config.db_path).expect("writer");
            conn.execute_batch(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qother', 1), ('Qlocal', 2);",
            )
            .expect("accounts");
            let file = decks::parse_deck_file(&first, chrono_tz::UTC).expect("parse");
            store::write::commit(
                &mut conn,
                &store::write::WriteBatch {
                    decks: vec![store::decks::DeckWrite::SyncFile {
                        signature: decks::signature(&file.cards),
                        file,
                        account: None,
                        platform: store::Platform::Mtgo,
                        synced_at_ms: 1,
                        identity: store::decks::IdentityUpdate::Keep,
                    }],
                    now_ms: 1,
                    ..store::write::WriteBatch::default()
                },
            )
            .expect("commit");
        }
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        let backfilled = deck(&config, DECK_1).expect("deck");
        assert_eq!(
            account_name(&config, backfilled.account_id).as_deref(),
            Some("Qlocal")
        );
        assert_eq!(backfilled.file_synced_at, Some(1_782_813_600_000));

        // An adoption is not counted as a deck written from its file.
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute("UPDATE decks SET account_id = NULL", [])
            .expect("no account again");
        drop(conn);
        assert_eq!(engine.sync_deck_files(NOW + 10_000).expect("sync"), 0);
        engine.flush().expect("flush");
        assert!(deck(&config, DECK_1).expect("deck").account_id.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_deck_file_is_warned_about_once() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("decks-sync-unreadable");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        let path = dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
            .expect("unreadable");
        if std::fs::read(&path).is_ok() {
            // Running as root: every file is readable.
            return;
        }
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        assert!(engine.unreadable_deck_files.contains(&path));
        assert!(
            !engine.warn_unreadable(&path),
            "the next sync does not warn again"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("readable");
    }

    fn decks_changed(tick: &crate::Tick) -> usize {
        tick.events
            .iter()
            .filter(|event| matches!(event, crate::TrackerEvent::DecksChanged))
            .count()
    }

    #[test]
    fn a_sync_that_writes_reports_decks_changed_once_after_the_commit() {
        let dir = TestDir::new("decks-sync-event");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        let second = deck_xml(DECK_2, "Qsecond", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        dir.write(&deck_path(ACCOUNT, DECK_2), second.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut reported = 0;
        for step in 0..3 {
            let tick = engine.tick(NOW + step * 1_000).expect("tick");
            if decks_changed(&tick) > 0 {
                assert!(
                    deck(&config, DECK_1).is_some() && deck(&config, DECK_2).is_some(),
                    "committed before it is reported"
                );
            }
            reported += decks_changed(&tick);
        }
        assert_eq!(reported, 1, "two decks written, one event");

        // A sync that writes nothing reports nothing.
        assert!(engine.request_deck_sync(NOW + 10_000));
        let tick = engine.tick(NOW + 10_000).expect("tick");
        assert_eq!(decks_changed(&tick), 0);
    }

    fn needs_archetype(tick: &crate::Tick) -> Vec<i64> {
        tick.events
            .iter()
            .filter_map(|event| match event {
                crate::TrackerEvent::DeckNeedsArchetype { deck_ids } => Some(deck_ids.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Decision C5: a deck the sync wrote without an archetype is told to
    /// the archetypes worker (0.x `SyncDecks::prefillArchetype`), after the
    /// commit; a deck with an archetype is not, and the sync never writes it.
    #[test]
    fn a_synced_deck_without_an_archetype_asks_for_one_after_the_commit() {
        let dir = TestDir::new("decks-sync-needs-archetype");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut asked = Vec::new();
        for step in 0..3 {
            let tick = engine.tick(NOW + step * 1_000).expect("tick");
            asked.extend(needs_archetype(&tick));
        }
        let stored = deck(&config, DECK_1).expect("deck");
        assert_eq!(asked, [stored.id]);
    }

    /// Review Focus 1: a set deck archetype is input; a later sync of its
    /// file neither asks for a prefill nor touches it.
    #[test]
    fn a_deck_archetype_survives_the_deck_sync() {
        let dir = TestDir::new("decks-sync-keeps-archetype");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        for step in 0..3 {
            engine.tick(NOW + step * 1_000).expect("tick");
        }
        let first_synced = deck(&config, DECK_1).expect("deck").file_synced_at;
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute_batch(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-arch', 'Qarch', 'modern', 'manual', 1, 1);
             UPDATE decks SET archetype_id = (SELECT id FROM archetypes WHERE uuid = 'q-arch');",
        )
        .expect("archetype");
        drop(conn);
        let second = deck_xml(DECK_1, "Qdeck", "2026-06-30T11:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), second.as_bytes());
        assert!(engine.request_deck_sync(NOW + 100_000));
        let mut asked: Vec<i64> = Vec::new();
        for step in 0..3 {
            let tick = engine.tick(NOW + 100_000 + step * 1_000).expect("tick");
            asked.extend(needs_archetype(&tick));
        }
        assert!(asked.is_empty());
        let conn = store::open_reader(&config.db_path).expect("reader");
        let kept: (Option<i64>,) = conn
            .query_row("SELECT archetype_id FROM decks", [], |row| {
                Ok((row.get(0)?,))
            })
            .expect("kept");
        assert!(kept.0.is_some(), "the archetype stays");
        assert_ne!(
            deck(&config, DECK_1).expect("deck").file_synced_at,
            first_synced,
            "the file was synced again"
        );
    }

    #[test]
    fn a_deck_sync_that_adds_card_stubs_reports_them_once() {
        let dir = TestDir::new("decks-sync-stubs");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut reported = Vec::new();
        for step in 0..4 {
            let tick = engine.tick(NOW + step * 1_000).expect("tick");
            engine.flush().expect("flush");
            for event in &tick.events {
                if let crate::TrackerEvent::CardStubsAdded { count } = event {
                    reported.push(*count);
                }
            }
        }
        let cards: i64 = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT COUNT(*) FROM cards", [], |row| row.get(0))
            .expect("cards");
        assert!(cards > 0);
        assert_eq!(reported, vec![cards as u32], "one report of every stub");
    }

    #[test]
    fn deck_sync_and_linking_are_timed_apart_from_the_rebuild() {
        let dir = TestDir::new("decks-sync-timing");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut synced = None;
        for step in 0..3 {
            let tick = engine.tick(NOW + step * 1_000).expect("tick");
            if tick.events.contains(&crate::TrackerEvent::DecksChanged) {
                synced = Some(tick.timing);
            }
        }
        let timing = synced.expect("a tick synced the deck files");
        assert!(timing.decks_us > 0, "{timing:?}");
        assert!(timing.decks_us <= timing.total_us, "{timing:?}");
    }

    #[test]
    fn a_done_edit_is_reported_at_the_next_tick_and_a_refused_one_is_not() {
        let dir = TestDir::new("decks-edit-event");
        dir.write("logs/mtgo.log", LOGIN);
        account_folder(&dir, ACCOUNT, 5);
        let first = deck_xml(DECK_1, "Qdeck", "2026-06-30T10:00:00", &MAIN);
        dir.write(&deck_path(ACCOUNT, DECK_1), first.as_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        let deck_id = deck(&config, DECK_1).expect("synced").id;

        let renamed = engine.edit_deck(DeckEdit::Rename {
            deck_id,
            name: "Qrenamed".to_string(),
        });
        assert_eq!(renamed.expect("answered"), EditOutcome::Done);
        assert_eq!(deck(&config, DECK_1).expect("deck").name, "Qrenamed");
        let tick = engine.tick(NOW + 10_000).expect("tick");
        assert_eq!(decks_changed(&tick), 1);
        let tick = engine.tick(NOW + 11_000).expect("tick");
        assert_eq!(decks_changed(&tick), 0, "reported once");

        let archived = engine.edit_deck(DeckEdit::Archive {
            deck_id,
            at_ms: NOW,
        });
        assert_eq!(archived.expect("answered"), EditOutcome::Done);
        engine.tick(NOW + 12_000).expect("tick");
        let refused = engine.edit_deck(DeckEdit::Rename {
            deck_id,
            name: "Qagain".to_string(),
        });
        assert_eq!(refused.expect("answered"), EditOutcome::Archived);
        let tick = engine.tick(NOW + 13_000).expect("tick");
        assert_eq!(decks_changed(&tick), 0, "a refused edit changed nothing");
    }

    #[test]
    fn a_deck_import_is_reported_at_the_next_tick() {
        let dir = TestDir::new("decks-import-event");
        dir.write("logs/mtgo.log", LOGIN);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_three_ticks(&mut engine, NOW);
        engine
            .import_decks(store::import_decks::ImportedDecks::default(), NOW)
            .expect("imported");
        let tick = engine.tick(NOW + 10_000).expect("tick");
        assert_eq!(decks_changed(&tick), 1);
    }
}
