//! The platform a page reads: MTGO, Arena or Endstep (`accounts.platform`).

use mtgo_match::ABANDON_AFTER_MS;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};

use crate::{Connection, StoreError};

/// The settings key of the platform the pages read (`"mtgo"`, `"arena"` or
/// `"endstep"`).
pub const PLATFORM_CONTEXT: &str = "platform_context";

/// The settings key of the Arena switch (endstep spec 8.1): a JSON bool, or
/// absent (or `null`) when the user has not chosen.
pub const ARENA_ENABLED: &str = "arena_enabled";

/// The settings key of the Endstep switch: a JSON bool, off when absent.
pub const ENDSTEP_ENABLED: &str = "endstep_enabled";

/// Which game's matches and decks a page shows. Every page reads one
/// platform; the app passes MTGO until the platform switch (Arena plan 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Mtgo,
    Arena,
    Endstep,
}

impl Platform {
    /// The `accounts.platform` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Mtgo => "mtgo",
            Platform::Arena => "arena",
            Platform::Endstep => "endstep",
        }
    }

    /// The platform of an `as_str` value; anything else is `None`.
    pub fn from_key(text: &str) -> Option<Platform> {
        match text {
            "mtgo" => Some(Platform::Mtgo),
            "arena" => Some(Platform::Arena),
            "endstep" => Some(Platform::Endstep),
            _ => None,
        }
    }
}

/// An Arena match's kind (`matches.game_structure`): best of one or best of
/// three. MTGO rows hold MTGO's `GameStructureCd` text in the same column, so
/// an MTGO read never filters by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BestOf {
    One,
    Three,
}

impl BestOf {
    /// The `matches.game_structure` value.
    pub fn key(self) -> &'static str {
        match self {
            BestOf::One => "bo1",
            BestOf::Three => "bo3",
        }
    }

    /// The kind of a `key` value; anything else (an MTGO structure) is `None`.
    pub fn from_key(text: &str) -> Option<BestOf> {
        match text {
            "bo1" => Some(BestOf::One),
            "bo3" => Some(BestOf::Three),
            _ => None,
        }
    }
}

/// Binds as its `matches.game_structure` text.
impl ToSql for BestOf {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.key()))
    }
}

/// Which platforms have data: MTGO when an MTGO account or an accountless
/// deck exists (a pulled Arena or Endstep deck waiting for its account,
/// `pulled_platform`, is not one), Arena or Endstep when an account of that
/// platform does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformPresence {
    pub mtgo: bool,
    pub arena: bool,
    pub endstep: bool,
}

impl PlatformPresence {
    /// Whether `platform` is marked.
    pub fn has(self, platform: Platform) -> bool {
        match platform {
            Platform::Mtgo => self.mtgo,
            Platform::Arena => self.arena,
            Platform::Endstep => self.endstep,
        }
    }
}

/// Which platforms have data (see `PlatformPresence`).
pub fn platform_presence(conn: &Connection) -> Result<PlatformPresence, StoreError> {
    let mtgo: bool = conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE platform = 'mtgo')
                 OR EXISTS (SELECT 1 FROM decks
                            WHERE account_id IS NULL AND pulled_platform IS NULL)",
        )?
        .query_row([], |row| row.get(0))?;
    let mut has_accounts =
        conn.prepare_cached("SELECT EXISTS (SELECT 1 FROM accounts WHERE platform = ?1)")?;
    let arena: bool = has_accounts.query_row([Platform::Arena], |row| row.get(0))?;
    let endstep: bool = has_accounts.query_row([Platform::Endstep], |row| row.get(0))?;
    Ok(PlatformPresence {
        mtgo,
        arena,
        endstep,
    })
}

/// The Integrations switches as stored: Arena's choice (`None` when not
/// chosen) and whether Endstep is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformSwitches {
    pub arena: Option<bool>,
    pub endstep: bool,
}

/// The stored switches. A value that is not a JSON bool is not chosen
/// (Arena) or off (Endstep).
pub fn stored_switches(conn: &Connection) -> Result<PlatformSwitches, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT value FROM settings WHERE key = ?1")?;
    let arena: Option<String> = stmt
        .query_row([ARENA_ENABLED], |row| row.get(0))
        .optional()?;
    let endstep: Option<String> = stmt
        .query_row([ENDSTEP_ENABLED], |row| row.get(0))
        .optional()?;
    Ok(PlatformSwitches {
        arena: arena.and_then(|value| serde_json::from_str::<bool>(&value).ok()),
        endstep: endstep
            .and_then(|value| serde_json::from_str::<bool>(&value).ok())
            .unwrap_or(false),
    })
}

/// Whether Arena is on: the stored choice, else on for anyone who already
/// has Arena data and off for a fresh install (endstep spec 8.1).
pub fn arena_on(switches: PlatformSwitches, presence: PlatformPresence) -> bool {
    switches.arena.unwrap_or(presence.arena)
}

/// The platforms the header shows and the pages may read: MTGO when it has
/// data, Arena and Endstep when they have data and are switched on.
pub fn shown(presence: PlatformPresence, switches: PlatformSwitches) -> PlatformPresence {
    PlatformPresence {
        mtgo: presence.mtgo,
        arena: presence.arena && arena_on(switches, presence),
        endstep: presence.endstep && switches.endstep,
    }
}

/// The platforms shown now (see `shown`).
pub fn shown_platforms(conn: &Connection) -> Result<PlatformPresence, StoreError> {
    Ok(shown(platform_presence(conn)?, stored_switches(conn)?))
}

/// The stored platform choice, if any. A value that is not a platform is none.
pub fn stored_platform(conn: &Connection) -> Result<Option<Platform>, StoreError> {
    let value: Option<String> = conn
        .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
        .query_row([PLATFORM_CONTEXT], |row| row.get(0))
        .optional()?;
    Ok(value.and_then(|value| Platform::from_key(&value)))
}

/// The platform the pages read (Arena spec 15) among the `shown` ones: the
/// stored choice when it is shown; else the first shown one of MTGO, Arena
/// and Endstep; with none, MTGO. A switched-off platform is never shown, so
/// it is never the page platform.
pub fn page_platform(stored: Option<Platform>, shown: PlatformPresence) -> Platform {
    if let Some(platform) = stored
        && shown.has(platform)
    {
        return platform;
    }
    for platform in [Platform::Mtgo, Platform::Arena, Platform::Endstep] {
        if shown.has(platform) {
            return platform;
        }
    }
    Platform::Mtgo
}

/// The platform the pages read now.
pub fn current_platform(conn: &Connection) -> Result<Platform, StoreError> {
    Ok(page_platform(
        stored_platform(conn)?,
        shown_platforms(conn)?,
    ))
}

/// The platform of the match `token`'s account, hidden or not (a hidden
/// Arena or Endstep match must never fall through to MTGO's reads); `None`
/// for an unknown match or one without an account.
pub fn match_platform(conn: &Connection, token: &str) -> Result<Option<Platform>, StoreError> {
    let platform: Option<String> = conn
        .prepare_cached(
            "SELECT a.platform FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE m.token = ?1",
        )?
        .query_row([token], |row| row.get(0))
        .optional()?;
    Ok(platform.and_then(|key| Platform::from_key(&key)))
}

/// Every listed match in progress, with its account's platform, oldest
/// first: the header's live dots before any push arrives. A match with no
/// activity for longer than the abandon window (`ABANDON_AFTER_MS`) before
/// `now_ms` is left out: the tracker settles it once it runs, and a long
/// closed app must not show it live until then.
pub fn live_matches(conn: &Connection, now_ms: i64) -> Result<Vec<(String, Platform)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.token, a.platform FROM listed_matches m
         JOIN accounts a ON a.id = m.account_id
         WHERE m.state = 'in_progress' AND m.last_activity_at >= ?1
         ORDER BY m.started_at, m.id",
    )?;
    let rows = stmt.query_map([now_ms - ABANDON_AFTER_MS], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (token, key) = row?;
        if let Some(platform) = Platform::from_key(&key) {
            out.push((token, platform));
        }
    }
    Ok(out)
}

/// Whether the match `token` belongs to an Arena or Endstep account, hidden
/// or not: such a match is never searched for in MTGO's data folder and its
/// replay is never shared. An unknown match, or one without an account, is
/// not.
pub fn match_is_non_mtgo(conn: &Connection, token: &str) -> Result<bool, StoreError> {
    Ok(matches!(
        match_platform(conn, token)?,
        Some(Platform::Arena | Platform::Endstep)
    ))
}

/// The platform of the deck's account; for a deck with no account, the
/// platform it was pulled for (`pulled_platform`, an Arena or Endstep deck
/// waiting for its account), else MTGO (a deck file or a 0.x import). MTGO
/// for no such deck.
pub fn deck_platform(conn: &Connection, deck_id: i64) -> Result<Platform, StoreError> {
    let platform: Option<String> = conn
        .prepare_cached(
            "SELECT COALESCE(a.platform, d.pulled_platform) FROM decks d
             LEFT JOIN accounts a ON a.id = d.account_id
             WHERE d.id = ?1",
        )?
        .query_row([deck_id], |row| row.get(0))
        .optional()?
        .flatten();
    Ok(platform
        .and_then(|key| Platform::from_key(&key))
        .unwrap_or(Platform::Mtgo))
}

/// The condition that the deck `deck` (a table name or alias, `d`) is of
/// the platform bound to `?{index}`: its account's. A deck without an
/// account is of the platform it was pulled for (`pulled_platform`, an Arena
/// or Endstep deck waiting for its account), else it came from MTGO (a deck
/// file or a 0.x import) and counts as MTGO.
pub fn deck_of_platform(deck: &str, index: u32) -> String {
    format!(
        "({of_account}
       OR ({deck}.account_id IS NULL AND COALESCE({deck}.pulled_platform, 'mtgo') = ?{index}))",
        of_account = account_of_platform(&format!("{deck}.account_id"), &format!("?{index}")),
    )
}

/// The condition that the account column `column` (`m.account_id`) belongs to
/// an account of the platform in `platform`: a bound parameter (`?4`) or a
/// quoted key (`'mtgo'`). The one spelling of "the matches of a platform".
pub fn account_of_platform(column: &str, platform: &str) -> String {
    format!("{column} IN (SELECT id FROM accounts WHERE platform = {platform})")
}

/// `account_of_platform` for MTGO, with the key written in.
pub fn account_of_mtgo(column: &str) -> String {
    account_of_platform(column, "'mtgo'")
}

/// Binds as its `accounts.platform` text, so a query takes it as a parameter.
impl ToSql for Platform {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

/// Reads an `accounts.platform` value. The columns are CHECK constrained, so
/// unknown text is a bad row and fails the read.
impl FromSql for Platform {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Platform::from_key(text)
            .ok_or_else(|| FromSqlError::Other(format!("platform {text}").into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::*;

    fn presence(mtgo: bool, arena: bool) -> PlatformPresence {
        PlatformPresence {
            mtgo,
            arena,
            endstep: false,
        }
    }

    #[test]
    fn the_account_condition_names_the_column_and_the_platform() {
        assert_eq!(
            account_of_platform("m.account_id", "?4"),
            "m.account_id IN (SELECT id FROM accounts WHERE platform = ?4)"
        );
        assert_eq!(
            account_of_mtgo("account_id"),
            "account_id IN (SELECT id FROM accounts WHERE platform = 'mtgo')"
        );
    }

    #[test]
    fn match_platform_follows_the_matchs_account() {
        let (_dir, conn) = db("platform-match-is-arena");
        let me = account(&conn, "Qlocal");
        for token in ["q-mtgo", "q-arena"] {
            conn.execute(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES (?1, ?2, 'log', 'complete', 'win', 1, 1, 1, 1)",
                rusqlite::params![token, me],
            )
            .expect("match");
        }
        move_match_to_arena(&conn, "q-arena");
        assert_eq!(
            match_platform(&conn, "q-mtgo").expect("mtgo"),
            Some(Platform::Mtgo)
        );
        assert_eq!(
            match_platform(&conn, "q-arena").expect("arena"),
            Some(Platform::Arena)
        );
        assert_eq!(match_platform(&conn, "q-none").expect("unknown"), None);
        // A hidden Arena match is still Arena: never an MTGO folder search.
        conn.execute(
            "UPDATE matches SET hidden_at = 1 WHERE token = 'q-arena'",
            [],
        )
        .expect("hide");
        assert_eq!(
            match_platform(&conn, "q-arena").expect("hidden arena"),
            Some(Platform::Arena)
        );
    }

    #[test]
    fn live_matches_are_the_listed_ones_in_progress_with_their_platform() {
        let (_dir, conn) = db("platform-live-matches");
        let me = account(&conn, "Qlocal");
        for (token, state, started) in [
            ("q-mtgo", "in_progress", 2),
            ("q-arena", "in_progress", 1),
            ("q-done", "complete", 3),
            ("q-hidden", "in_progress", 4),
        ] {
            conn.execute(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES (?1, ?2, 'log', ?3, 'unknown', ?4, 1, 1, 1)",
                rusqlite::params![token, me, state, started],
            )
            .expect("match");
        }
        move_match_to_arena(&conn, "q-arena");
        conn.execute(
            "UPDATE matches SET hidden_at = 1 WHERE token = 'q-hidden'",
            [],
        )
        .expect("hide");
        assert_eq!(
            live_matches(&conn, 1).expect("live"),
            vec![
                ("q-arena".to_string(), Platform::Arena),
                ("q-mtgo".to_string(), Platform::Mtgo),
            ]
        );
    }

    #[test]
    fn a_match_quiet_past_the_abandon_window_is_not_live() {
        let (_dir, conn) = db("platform-live-stale");
        let me = account(&conn, "Qlocal");
        let now = 10 * ABANDON_AFTER_MS;
        for (token, last_activity) in [
            ("q-fresh", now - ABANDON_AFTER_MS),
            ("q-stale", now - ABANDON_AFTER_MS - 1),
        ] {
            conn.execute(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES (?1, ?2, 'log', 'in_progress', 'unknown', 1, ?3, 1, 1)",
                rusqlite::params![token, me, last_activity],
            )
            .expect("match");
        }
        assert_eq!(
            live_matches(&conn, now).expect("live"),
            vec![("q-fresh".to_string(), Platform::Mtgo)]
        );
    }

    #[test]
    fn match_is_non_mtgo_is_true_for_arena_and_endstep_matches() {
        let (_dir, mut conn) = db("platform-match-non-mtgo");
        let me = account(&conn, "Qlocal");
        conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-mtgo', ?1, 'log', 'complete', 'win', 1, 1, 1, 1),
                    ('q-arena', ?1, 'log', 'complete', 'win', 1, 1, 1, 1)",
            [me],
        )
        .expect("matches");
        move_match_to_arena(&conn, "q-arena");
        seed_endstep(&mut conn, "q-endstep");
        assert!(!match_is_non_mtgo(&conn, "q-mtgo").expect("mtgo"));
        assert!(match_is_non_mtgo(&conn, "q-arena").expect("arena"));
        assert!(match_is_non_mtgo(&conn, "q-endstep").expect("endstep"));
        assert!(!match_is_non_mtgo(&conn, "q-none").expect("unknown"));
        assert_eq!(
            match_platform(&conn, "q-endstep").expect("platform"),
            Some(Platform::Endstep)
        );
        // A hidden Endstep match is still Endstep.
        hide(&conn, "q-endstep");
        assert!(match_is_non_mtgo(&conn, "q-endstep").expect("hidden endstep"));
    }

    #[test]
    fn page_platform_follows_the_only_platform_with_data() {
        assert_eq!(
            page_platform(Some(Platform::Mtgo), presence(false, true)),
            Platform::Arena
        );
        assert_eq!(
            page_platform(Some(Platform::Arena), presence(true, false)),
            Platform::Mtgo
        );
        assert_eq!(page_platform(None, presence(false, false)), Platform::Mtgo);
        assert_eq!(
            page_platform(Some(Platform::Arena), presence(false, false)),
            Platform::Mtgo
        );
    }

    #[test]
    fn page_platform_keeps_the_stored_choice_when_both_have_data() {
        assert_eq!(
            page_platform(Some(Platform::Arena), presence(true, true)),
            Platform::Arena
        );
        assert_eq!(page_platform(None, presence(true, true)), Platform::Mtgo);
    }

    #[test]
    fn presence_counts_accounts_and_accountless_decks() {
        let (_dir, conn) = db("platform-presence");
        assert_eq!(
            platform_presence(&conn).expect("read"),
            presence(false, false)
        );
        conn.execute(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
             VALUES ('d1', 'Deck', 'CMODERN', 1, 1, 'file')",
            [],
        )
        .expect("deck");
        assert_eq!(
            platform_presence(&conn).expect("read"),
            presence(true, false)
        );
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Player#00001', 1, 'arena', 'Player#00001')",
            [],
        )
        .expect("arena account");
        assert_eq!(
            platform_presence(&conn).expect("read"),
            presence(true, true)
        );
    }

    /// A pulled Arena or Endstep deck waiting for its account
    /// (`pulled_platform`, account NULL) is its platform's, never MTGO's: not
    /// on MTGO deck pages, not MTGO presence, not an MTGO deck.
    #[test]
    fn a_waiting_pulled_deck_is_its_platforms_not_mtgos() {
        let (_dir, conn) = db("platform-pulled-deck");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source,
                 pulled_platform)
             VALUES (1, 'q-arena', 'Qarena', 'CMODERN', 1, 1, 'imported', 'arena'),
                    (2, 'q-endstep', 'Qendstep', 'CMODERN', 1, 1, 'imported', 'endstep');",
        )
        .expect("decks");
        assert_eq!(deck_platform(&conn, 1).expect("read"), Platform::Arena);
        assert_eq!(deck_platform(&conn, 2).expect("read"), Platform::Endstep);
        assert!(
            !platform_presence(&conn).expect("read").mtgo,
            "a waiting Arena or Endstep deck is no MTGO data"
        );
        let scope = |platform: Platform| crate::deck_index::DeckScope {
            account_id: None,
            platform,
            format: None,
            hide_archived: false,
        };
        let count = |platform: Platform| {
            crate::deck_index::unclassified_count(&conn, &scope(platform)).expect("count")
        };
        assert_eq!(count(Platform::Mtgo), 0, "not on MTGO deck pages");
        assert_eq!(count(Platform::Arena), 1);
        assert_eq!(count(Platform::Endstep), 1);
        conn.execute(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
             VALUES ('q-file', 'Qfile', 'CMODERN', 1, 1, 'file')",
            [],
        )
        .expect("an MTGO deck file");
        assert_eq!(
            count(Platform::Mtgo),
            1,
            "an accountless deck file is MTGO's"
        );
        assert!(platform_presence(&conn).expect("read").mtgo);
    }

    #[test]
    fn endstep_round_trips_its_key() {
        assert_eq!(Platform::Endstep.as_str(), "endstep");
        assert_eq!(Platform::from_key("endstep"), Some(Platform::Endstep));
        for platform in [Platform::Mtgo, Platform::Arena, Platform::Endstep] {
            assert_eq!(Platform::from_key(platform.as_str()), Some(platform));
        }
    }

    #[test]
    fn presence_counts_endstep_accounts() {
        let (_dir, conn) = db("platform-presence-endstep");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, display_name)
             VALUES ('endstep:1', 1, 'endstep', 'Player')",
            [],
        )
        .expect("endstep account");
        assert_eq!(
            platform_presence(&conn).expect("read"),
            PlatformPresence {
                mtgo: false,
                arena: false,
                endstep: true,
            }
        );
    }

    #[test]
    fn page_platform_follows_endstep_when_it_is_the_only_one_with_data() {
        let only_endstep = PlatformPresence {
            mtgo: false,
            arena: false,
            endstep: true,
        };
        assert_eq!(page_platform(None, only_endstep), Platform::Endstep);
        let all = PlatformPresence {
            mtgo: true,
            arena: true,
            endstep: true,
        };
        assert_eq!(
            page_platform(Some(Platform::Endstep), all),
            Platform::Endstep
        );
        assert_eq!(page_platform(None, all), Platform::Mtgo);
        let arena_and_endstep = PlatformPresence {
            mtgo: false,
            arena: true,
            endstep: true,
        };
        assert_eq!(
            page_platform(Some(Platform::Arena), arena_and_endstep),
            Platform::Arena,
            "with data on two platforms the stored choice wins"
        );
        assert_eq!(
            page_platform(Some(Platform::Mtgo), arena_and_endstep),
            Platform::Arena,
            "a stored platform with no data falls back to the first one with data"
        );
    }

    fn set(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )
        .expect("set");
    }

    #[test]
    fn a_switched_off_platform_is_never_current() {
        let (_dir, conn) = db("platform-switched-off");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Player#00001', 1, 'arena', 'Player#00001')",
            [],
        )
        .expect("arena account");
        assert_eq!(
            current_platform(&conn).expect("read"),
            Platform::Arena,
            "Arena not chosen yet: on, since it has data"
        );
        set(&conn, PLATFORM_CONTEXT, "arena");
        set(&conn, ARENA_ENABLED, "false");
        assert_eq!(
            current_platform(&conn).expect("read"),
            Platform::Mtgo,
            "the only platform with data is off, and so is the stored choice"
        );
        assert!(!shown_platforms(&conn).expect("read").arena);

        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, display_name)
             VALUES ('endstep:1', 1, 'endstep', 'Player')",
            [],
        )
        .expect("endstep account");
        assert_eq!(
            current_platform(&conn).expect("read"),
            Platform::Mtgo,
            "Endstep is off until switched on"
        );
        set(&conn, ENDSTEP_ENABLED, "true");
        assert_eq!(current_platform(&conn).expect("read"), Platform::Endstep);
        set(&conn, ARENA_ENABLED, "true");
        assert_eq!(
            current_platform(&conn).expect("read"),
            Platform::Arena,
            "both on: the stored choice"
        );
        set(&conn, ARENA_ENABLED, "null");
        assert_eq!(
            current_platform(&conn).expect("read"),
            Platform::Arena,
            "a not-chosen Arena switch is on for Arena data"
        );
        set(&conn, ENDSTEP_ENABLED, "false");
        set(&conn, PLATFORM_CONTEXT, "endstep");
        assert_eq!(
            current_platform(&conn).expect("read"),
            Platform::Arena,
            "a stored choice that is off is never current"
        );
    }

    #[test]
    fn stored_platform_ignores_junk() {
        let (_dir, conn) = db("platform-stored");
        assert_eq!(stored_platform(&conn).expect("read"), None);
        for (value, expected) in [
            ("xbox", None),
            ("arena", Some(Platform::Arena)),
            ("mtgo", Some(Platform::Mtgo)),
        ] {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                rusqlite::params![PLATFORM_CONTEXT, value],
            )
            .expect("set");
            assert_eq!(stored_platform(&conn).expect("read"), expected);
        }
    }

    #[test]
    fn kinds_are_the_game_structure_values() {
        assert_eq!(BestOf::One.key(), "bo1");
        assert_eq!(BestOf::Three.key(), "bo3");
        assert_eq!(BestOf::from_key("bo1"), Some(BestOf::One));
        assert_eq!(BestOf::from_key("bo3"), Some(BestOf::Three));
        assert_eq!(BestOf::from_key("Modern"), None);
    }

    #[test]
    fn platforms_are_the_account_values() {
        assert_eq!(Platform::Mtgo.as_str(), "mtgo");
        assert_eq!(Platform::Arena.as_str(), "arena");
    }
}
