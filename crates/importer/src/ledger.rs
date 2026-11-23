//! The 0.x coverage ledger: every table and column of the final 0.x schema
//! (`fixtures/0x/schema.sql`) with one disposition, so nothing is forgotten
//! before 0.x is retired. A later sub-project flips its `Deferred` rows to
//! `Imported` when its import step lands. `settings_entries` does the same
//! for the `settings.json` keys a sub-project imports (overlay spec section
//! 12).

use Disposition::{Imported, Skipped};

/// What happens to one 0.x column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Read by an importer reader into this v1 home.
    Imported(&'static str),
    /// Not imported yet: the named sub-project (or plan) must import it.
    Deferred(&'static str, &'static str),
    /// Never imported, for this reason.
    Skipped(&'static str),
}

/// One 0.x column and what happens to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerEntry {
    pub table: &'static str,
    pub column: &'static str,
    pub disposition: Disposition,
}

/// Every sub-project or plan a `Deferred` row may name.
pub const DEFERRED_TARGETS: [&str; 7] = ["06", "07", "08", "09", "10", "11", "12"];

/// Rows of the ledger: a table, its columns (space separated) and one
/// disposition for all of them.
const GROUPS: &[(&str, &str, Disposition)] = &[
    // accounts
    (
        "accounts",
        "id username",
        Imported(
            "accounts.username (every account not deleted, and the account of a deck or seat)",
        ),
    ),
    (
        "accounts",
        "login_id created_at deleted_at",
        Imported(
            "accounts.login_id (when v1 has none) and first_seen_at; deleted accounts are not registered",
        ),
    ),
    (
        "accounts",
        "tracked",
        Imported(
            "accounts.tracked (an untracked account arrives untracked unless the user set tracking in v1)",
        ),
    ),
    (
        "accounts",
        "active updated_at",
        Imported(
            "the active_account setting (0.x's active account, at its updated_at, when v1 has no switch)",
        ),
    ),
    // Laravel and 0.x framework state
    (
        "app_settings",
        "id timezone created_at updated_at",
        Skipped("legacy table no 0.x code reads; v1 dates logs with the OS timezone"),
    ),
    (
        "app_updates",
        "id update ran_at",
        Skipped("records which 0.x data repairs ran; v1 data is already repaired"),
    ),
    ("cache", "key value expiration", Skipped("Laravel cache")),
    (
        "cache_locks",
        "key owner expiration",
        Skipped("Laravel cache"),
    ),
    (
        "failed_jobs",
        "id uuid connection queue payload exception failed_at",
        Skipped("Laravel queue"),
    ),
    (
        "job_batches",
        "id name total_jobs pending_jobs failed_jobs failed_job_ids options cancelled_at created_at finished_at",
        Skipped("Laravel queue"),
    ),
    (
        "jobs",
        "id queue payload attempts reserved_at available_at created_at",
        Skipped("Laravel queue"),
    ),
    (
        "migrations",
        "id migration batch",
        Skipped("Laravel migrations"),
    ),
    // archetypes (#9, Plan T task 14: archetypes spec sections 15 and 21.5)
    (
        "archetype_cards",
        "archetype_id card_id quantity sideboard",
        Imported(
            "archetype_cards list 1 of a manual archetype without variants (0.x backfill_manual_archetype_variants)",
        ),
    ),
    (
        "archetype_cards",
        "id created_at updated_at",
        Skipped("row key (only orders rows) and bookkeeping"),
    ),
    (
        "archetype_deck_cards",
        "archetype_deck_id card_id quantity sideboard",
        Imported(
            "archetype_cards of a manual archetype's variant; a downloaded archetype's are not imported: decklists are fetched live",
        ),
    ),
    (
        "archetype_deck_cards",
        "id created_at updated_at",
        Skipped("row key (only orders rows) and bookkeeping"),
    ),
    (
        "archetype_decks",
        "id archetype_id",
        Imported(
            "archetype_cards lists 1, 2, ... of a manual archetype in id order; a downloaded archetype's are not imported: decklists are fetched live",
        ),
    ),
    (
        "archetype_decks",
        "uuid seen_count last_synced_at created_at updated_at",
        Skipped(
            "sample deck bookkeeping: decklists are fetched live, and a manual variant needs only its cards",
        ),
    ),
    (
        "archetype_match_attempts",
        "id match_id player_id format payload status_code response archetype_id confidence created_at updated_at",
        Skipped("debug log of archetype API calls"),
    ),
    (
        "archetypes",
        "id uuid name format color_identity manual is_fallback source_match_id incomplete merged_into_id created_at updated_at",
        Imported(
            "archetypes: the manual ones and those referenced by a match row, deck, guide, note or merge (the rest of 0.x's catalog is skipped: met archetypes are fetched as definitions); fallbacks map to the seeds by uuid",
        ),
    ),
    (
        "archetypes",
        "decklist_downloaded_at",
        Skipped(
            "0.x catalog download bookkeeping: met archetypes are fetched as definitions by the change check",
        ),
    ),
    (
        "deck_archetype_notes",
        "deck_id archetype_id body created_at",
        Imported(
            "deck_archetype_notes, once per (deck, archetype, created_at, body); a noted pair without a guide gets an empty guide",
        ),
    ),
    (
        "deck_archetype_notes",
        "id updated_at",
        Skipped("row key (only orders rows) and bookkeeping (a note is never edited)"),
    ),
    (
        "match_archetypes",
        "archetype_id mtgo_match_id player_id confidence manual",
        Imported(
            "match_archetypes, seat from game_player.is_local (hand-set rows always, detected ones where v1 has none)",
        ),
    ),
    (
        "match_archetypes",
        "archetype_deck_id",
        Skipped("v1 keeps no sample decks (archetypes ruling 9)"),
    ),
    (
        "match_archetypes",
        "id created_at updated_at",
        Skipped("row key (the first row per seat wins) and bookkeeping"),
    ),
    (
        "sideboard_guide_cards",
        "sideboard_guide_id oracle_id direction quantity",
        Imported("sideboard_guide_cards of an imported guide"),
    ),
    (
        "sideboard_guide_cards",
        "id created_at updated_at",
        Skipped("row key (only orders rows) and bookkeeping"),
    ),
    (
        "sideboard_guides",
        "deck_id archetype_id created_at updated_at",
        Imported("sideboard_guides (a pair v1 has a guide for keeps v1's)"),
    ),
    (
        "sideboard_guides",
        "id",
        Skipped("row key (joins the guide's cards)"),
    ),
    // card stats and API sync (#6, #7)
    (
        "card_game_stats",
        "oracle_id game_id quantity kept seen sided_out cast sided_in played kicked flashback madness evoked activated pregame_revealed pregame_played opponent warp free_cast bargained dashed bestowed replicated spectacle rebound escaped ninjutsu suspended buyback disturb foretold retraced mayhem miracle gifted casualty hand_seen graveyard_seen exile_seen battlefield_seen discarded cast_turn has_zone_data",
        Imported("game_cards and game_card_casts"),
    ),
    (
        "card_game_stats",
        "won",
        Skipped("a duplicate of the game's result (games.result)"),
    ),
    (
        "card_game_stats",
        "is_postboard",
        Skipped("a duplicate of games.game_number > 1"),
    ),
    (
        "card_game_stats",
        "deck_version_id",
        Skipped(
            "a duplicate of the game's match's matches.deck_version_id, which follows every relink",
        ),
    ),
    (
        "card_game_stats",
        "id created_at updated_at",
        Skipped("row key and bookkeeping"),
    ),
    (
        "card_stat_ship_queue",
        "game_id match_id payload status attempts next_attempt_at last_error created_at updated_at",
        Imported(
            "outbox card_stats rows, subject {match token}:{mtgo game id}, status and payload kept (sending becomes pending)",
        ),
    ),
    ("card_stat_ship_queue", "id", Skipped("row key")),
    (
        "sync_rejections",
        "type client_id reason hash",
        Imported("sync_rejections"),
    ),
    (
        "sync_rejections",
        "id created_at updated_at",
        Skipped("row key and bookkeeping"),
    ),
    (
        "sync_state",
        "type last_synced_at canonical_version full_cursor",
        Imported(
            "sync_state (a 0.x full_cursor, a 0.x row id, is dropped and the type runs in full: canonical_version 0)",
        ),
    ),
    (
        "sync_state",
        "id created_at updated_at",
        Skipped("row key and bookkeeping"),
    ),
    (
        "tournament_observation_queue",
        "id log_event_id tournament_token match_token event_type payload client_observed_at status attempts next_attempt_at last_error created_at updated_at",
        Skipped(
            "0.x queue keyed by 0.x log event ids; v1 ships its own stored events and the API ignores repeats by content hash",
        ),
    ),
    (
        "tournaments",
        "mtgo_event_id token name format started_at name_synthesized",
        Imported(
            "tournaments.name with name_synthesized 0 and name_checked_at (real names only; a missing row gets 0.x's format and start)",
        ),
    ),
    (
        "tournaments",
        "id created_at updated_at",
        Skipped("row key and bookkeeping"),
    ),
    // cards
    (
        "cards",
        "id mtgo_id oracle_id name type sub_type rarity color_identity colors cmc mana_cost set_name set_code image art_crop scryfall_id",
        Imported("cards"),
    ),
    (
        "cards",
        "local_image local_art_crop",
        Skipped("copied or re-downloaded by v1, spec ruling 3"),
    ),
    ("cards", "created_at updated_at", Skipped("row bookkeeping")),
    // decks
    (
        "decks",
        "id mtgo_id name format account_id original_name color_identity cover_id deleted_at created_at updated_at",
        Imported(
            "decks (constructed; 0.x's limited decks are no deck rows: their versions become registered decks per match)",
        ),
    ),
    (
        "decks",
        "archetype_id",
        Imported("decks.archetype_id where v1's is NULL"),
    ),
    (
        "decks",
        "synced_hash synced_at cloud_sync_enabled",
        Imported(
            "synced_rows (deck_client_id) and decks.cloud_sync_enabled (the next manifest corrects it)",
        ),
    ),
    (
        "decks",
        "deck_file_synced_at",
        Skipped("0.x's scan time of the deck files; v1 rescans them"),
    ),
    (
        "deck_versions",
        "id deck_id signature modified_at",
        Imported(
            "deck_versions (constructed); a limited version becomes limited_deck_snapshots (imported) for its matches without a snapshot",
        ),
    ),
    (
        "deck_versions",
        "created_at updated_at",
        Skipped("row bookkeeping"),
    ),
    // drafts and limited (Plan N)
    (
        "draft_picks",
        "draft_id ordinal pack_number pick_number pack_id direction cards_available picked_catalog_id picked_card_id reservations shown_at deadline_at picked_at",
        Imported("draft_picks of the imported draft"),
    ),
    (
        "draft_picks",
        "note",
        Imported("draft_picks.note (only where the pick has none)"),
    ),
    (
        "draft_picks",
        "id created_at updated_at",
        Skipped("row key (only orders rows) and bookkeeping"),
    ),
    (
        "drafts",
        "id league_id draft_token mtgo_draft_id pod_token seat_count seat_index booster_catalog_id state pack_size picks_expected started_at ended_at",
        Imported("drafts (source imported; never over a draft v1 has from the log)"),
    ),
    (
        "drafts",
        "tournament_id",
        Skipped("reserved in 0.x and never set"),
    ),
    (
        "drafts",
        "created_at updated_at",
        Skipped("row bookkeeping"),
    ),
    (
        "limited_deck_snapshots",
        "league_id match_id source cards signature captured_at match_token",
        Imported("limited_deck_snapshots (source imported, per match)"),
    ),
    (
        "limited_deck_snapshots",
        "id game_id created_at updated_at",
        Skipped("row key (only orders rows), a game 0.x never set, bookkeeping"),
    ),
    // match detail (Plan O)
    (
        "game_logs",
        "match_token decoded_entries",
        Imported("game_logs.decoded_entries"),
    ),
    (
        "game_logs",
        "id file_path created_at updated_at decoded_at byte_offset decoded_version first_timestamp players",
        Skipped("0.x log bookkeeping; the blob keeps each entry's own timestamp"),
    ),
    (
        "game_timelines",
        "game_id content",
        Imported("game_timelines (one zstd row of frames per game)"),
    ),
    (
        "game_timelines",
        "timestamp",
        Imported(
            "game_timelines.times (each kept frame's time of day; Y-m-d H:i:s keeps its time part)",
        ),
    ),
    (
        "game_timelines",
        "id created_at updated_at",
        Skipped("row key (only an ordering tie-break) and bookkeeping"),
    ),
    // game_player
    (
        "game_player",
        "game_id player_id is_local on_play dice_roll",
        Imported("games (seats)"),
    ),
    (
        "game_player",
        "mulligan_count",
        Imported("games (seats; optional, an older 0.x has none)"),
    ),
    (
        "game_player",
        "opening_hand_json",
        Imported("game_cards (opening_hand, bottomed, starting_hand) and game_mulligan_cards"),
    ),
    (
        "game_player",
        "deck_json",
        Imported("game_cards (local main_qty and side_qty, opponent seen)"),
    ),
    (
        "game_player",
        "starting_hand_size",
        Imported("games.local_starting_hand_size and opp_starting_hand_size"),
    ),
    (
        "game_player",
        "id instance_id created_at updated_at",
        Skipped("row key, in-game id only meaningful inside one game, or bookkeeping"),
    ),
    (
        "game_player",
        "clock_remaining_ms_start clock_remaining_ms_end clock_remaining_ms_min sideboard_ms_used",
        Skipped("no v1 home (Plan K left out clocks); owner option in the audit"),
    ),
    // games
    (
        "games",
        "id match_id mtgo_id won started_at ended_at turn_count",
        Imported("games"),
    ),
    ("games", "created_at updated_at", Skipped("row bookkeeping")),
    (
        "games",
        "replay_share_uuid replay_share_url replay_shared_at",
        Skipped("pre-release per-game link; switch it off on mymtgo.com (counted, never imported)"),
    ),
    // leagues
    (
        "leagues",
        "id token name format started_at created_at deleted_at state deck_version_id event_id joined_at dropped_at notes completed_at manual",
        Imported(
            "leagues (source imported with imported_seed, or manual for manual constructed ones)",
        ),
    ),
    (
        "leagues",
        "kind set_code mtgo_course_id",
        Imported(
            "leagues.imported_seed kind, set_code, mtgo_course_id (draft and sealed leagues seed the limited runs)",
        ),
    ),
    (
        "leagues",
        "synced_hash synced_at",
        Imported("synced_rows (league_client_id of token and started_at)"),
    ),
    (
        "leagues",
        "deck_change_detected",
        Skipped(
            "dead in 0.x: only a removed phantom-league migration set it; v1's fold never puts another deck version's match in a constructed run",
        ),
    ),
    ("leagues", "updated_at", Skipped("row bookkeeping")),
    // matches
    (
        "matches",
        "id token mtgo_id deck_version_id format match_type started_at ended_at state notes outcome tournament_event_id tournament_round manual",
        Imported("matches"),
    ),
    (
        "matches",
        "league_id",
        Imported("matches.league_id with league_linked"),
    ),
    (
        "matches",
        "games_won games_lost",
        Imported(
            "matches.games_won_imported, games_lost_imported (a match without games; stale on matches with games)",
        ),
    ),
    (
        "matches",
        "submitted_at synced_hash synced_at tournament_token",
        Imported(
            "outbox match_report rows sent (submitted_at), synced_rows (the token) and matches.tournament_token",
        ),
    ),
    (
        "matches",
        "tournament_id",
        Skipped("the league sync links tournaments by tournament_event_id"),
    ),
    (
        "matches",
        "replay_share_uuid replay_share_url replay_shared_at",
        Imported("matches.replay_share_* (when v1's are null; a link missing a part is counted)"),
    ),
    (
        "matches",
        "imported",
        Imported(
            "matches.legacy_history (which games are history games for the imported postboard rule, issue #313)",
        ),
    ),
    (
        "matches",
        "result created_at updated_at failed_at attempts archetype_detection_queued_at",
        Skipped("legacy result, pipeline state or bookkeeping"),
    ),
    // players
    (
        "players",
        "id username",
        Imported("games (opponent) and accounts"),
    ),
    (
        "players",
        "is_player created_at updated_at",
        Skipped("0 on every row, bookkeeping"),
    ),
    // 0.x's own staging and log reader state
    (
        "import_scan_matches",
        "id import_scan_id history_id started_at opponent format format_display games_won games_lost outcome game_log_token confidence round description game_ids local_player created_at updated_at",
        Skipped("0.x's history scan staging; Plan M has its own scan"),
    ),
    (
        "import_scans",
        "id deck_version_id status progress total error created_at updated_at stage",
        Skipped("0.x's history scan staging; Plan M has its own scan"),
    ),
    (
        "log_cursors",
        "id log_instance_id byte_offset last_observed_size last_advance_at stuck_ticks verify_anchor_offset verify_anchor_hash created_at updated_at",
        Skipped("0.x's log reader state; v1 reads mtgo.log itself"),
    ),
    (
        "log_events",
        "id file_path byte_offset_start byte_offset_end timestamp level category context raw_text ingested_at processed_at match_token game_id match_id event_type logged_at created_at updated_at username tournament_token log_instance_id draft_token",
        Skipped("a pruned subset of raw events; imported matches are frozen"),
    ),
    (
        "log_instances",
        "id file_path identity_hash file_ctime head_hash anchor_offset anchor_hash tail_hash local_username first_seen_at last_seen_at sealed_at seal_reason created_at updated_at",
        Skipped("0.x's log reader state; v1 reads mtgo.log itself"),
    ),
];

/// Every ledger entry, one per column.
pub fn entries() -> Vec<LedgerEntry> {
    let mut entries = Vec::new();
    for (table, columns, disposition) in GROUPS {
        for column in columns.split_whitespace() {
            entries.push(LedgerEntry {
                table,
                column,
                disposition: *disposition,
            });
        }
    }
    entries
}

/// One 0.x `settings.json` key and what happens to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsEntry {
    pub key: &'static str,
    pub disposition: Disposition,
}

/// The overlay keys of 0.x's `settings.json` (overlay spec section 12,
/// survey section 6). Kept keys are stored in `legacy_0x_settings` by
/// `filter_settings` and read from there; skipped ones are dropped by it.
const SETTINGS: &[(&str, Disposition)] = &[
    (
        "game_overlay draft_notes_window league_window overlay_show_opponent overlay_show_draw_odds overlay_show_reveals overlay_show_sideboard overlay_artwork overlay_size overlay_publish",
        Imported(
            "typed Settings fields, seeded from legacy_0x_settings (and written by the import where v1 is at the default)",
        ),
    ),
    (
        "deck_window opponent_window",
        Imported("Settings game_overlay when 0.x has no game_overlay (0.x showGameOverlay)"),
    ),
    (
        "overlay_collapsed overlay_expanded_height",
        Imported("UI state rows, read from legacy_0x_settings until v1 writes its own"),
    ),
    (
        "overlay_deck_labels",
        Imported(
            "decks.overlay_label where v1's is NULL (trimmed, 40 characters), each 0.x deck id mapped by 0.x decks.mtgo_id; ids of no 0.x deck, or of a deck with no v1 row (0.x limited decks), are counted",
        ),
    ),
    (
        "overlay_background_path",
        Imported(
            "the file copied from 0.x's storage/app/overlay/ into <app data>/overlay/ with the upload's checks (overlay_background_file); skipped and counted when missing or refused, and when v1 has its own",
        ),
    ),
    (
        "overlay_publish_error overlay_background_remote overlay_last_published_at",
        Skipped(
            "internal publish state: v1 writes its own, and the first publish uploads the background again",
        ),
    ),
    (
        "app_server_url",
        Skipped("NativePHP's local server URL; v1 has no local server"),
    ),
];

/// Every settings ledger entry, one per key.
pub fn settings_entries() -> Vec<SettingsEntry> {
    let mut entries = Vec::new();
    for (keys, disposition) in SETTINGS {
        for key in keys.split_whitespace() {
            entries.push(SettingsEntry {
                key,
                disposition: *disposition,
            });
        }
    }
    entries
}
