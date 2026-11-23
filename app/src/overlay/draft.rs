//! The draft notes window (overlay spec 8).
//!
//! The window's Rust side is Plan N's, shared rather than copied:
//! - its first read is the `draft_notes` command (`limited::notes_window`,
//!   0.x `BuildDraftNotes`);
//! - its updates are the worker's `DraftNotesChanged { view }`, the same view
//!   pushed on `LimitedChanged` and on the grace timer, so the page never
//!   polls (0.x polled every second);
//! - its notes are saved with `set_pick_note`, keyed by the draft token,
//!   through the tracker's `DraftNoteEdit`, so a draft not linked to a
//!   league yet takes notes too (spec ruling 12; 0.x refused them).
//!
//! The view carries each pick's `note_saved_at_ms`, so the page's autosave
//! never lets an older pushed note replace one it saved later. 0.x's
//! `serverNow` is dropped: the countdown runs on the same machine's clock.

#[cfg(test)]
mod tests {
    use fixture_tools::TestDir;
    use serde_json::json;
    use store::{Connection, params};

    use crate::commands::tests::ready_state;
    use crate::limited::{DraftNotesView, notes_window, set_note};
    use crate::overlay::DRAFT_NOTES;
    use crate::overlay::windows::testing::FakeWindows;
    use crate::overlay::worker::{OverlayMsg, Worker};
    use crate::settings::Settings;

    /// 2026-08-22T11:12:00Z (0.x's test clock).
    const NOW: i64 = 1_787_397_120_000;
    const SECOND: i64 = 1_000;
    const MINUTE: i64 = 60 * SECOND;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        account(&conn);
        (dir, conn)
    }

    fn account(conn: &Connection) {
        conn.execute(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0)",
            [],
        )
        .expect("account");
    }

    /// A picking draft of the local account, not linked to a league.
    fn picking_draft(conn: &Connection, id: i64, now_ms: i64) {
        conn.execute(
            "INSERT INTO drafts (id, draft_token, account_id, source, seat_count, seat_index,
                 state, pack_size, picks_expected, started_at, imported_from)
             VALUES (?1, 'q-draft-' || ?1, 1, 'imported', 8, 5, 'picking', 14, 42, ?2, ?1)",
            params![id, now_ms - MINUTE],
        )
        .expect("draft");
    }

    /// A pick of pack 1 with `available` cards in the pack.
    fn pick(
        conn: &Connection,
        draft_id: i64,
        ordinal: u32,
        available: &[u32],
        deadline_at: Option<i64>,
    ) {
        let available = serde_json::to_string(available).expect("available as JSON");
        conn.execute(
            "INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number, pack_id,
                 cards_available, reservations, deadline_at)
             VALUES (?1, ?2, 1, ?2, 500, ?3, '[]', ?4)",
            params![draft_id, ordinal, available, deadline_at],
        )
        .expect("pick");
    }

    fn note(conn: &mut Connection, ordinal: u32, text: &str) {
        store::limited::edit_draft_note(
            conn,
            &store::limited::DraftNoteEdit::Set {
                draft_token: "q-draft-1".to_string(),
                ordinal,
                note: text.to_string(),
                now_ms: NOW,
            },
        )
        .expect("note");
    }

    /// The league sync the tracker runs before the worker reads. The write
    /// lock is taken first: a running tracker may commit meanwhile.
    fn sync(conn: &Connection, now_ms: i64) {
        let tx = store::Transaction::new_unchecked(conn, store::TransactionBehavior::Immediate)
            .expect("transaction");
        store::league_sync::sync_every_owner(&tx, chrono_tz::UTC, now_ms).expect("league sync");
        tx.commit().expect("commit");
    }

    /// The page's first read (the `draft_notes` command).
    fn command(conn: &Connection, now_ms: i64) -> Option<DraftNotesView> {
        notes_window(conn, now_ms).expect("draft notes")
    }

    /// What the worker pushes to the window after a `LimitedChanged`.
    fn pushed(conn: &Connection, now_ms: i64) -> serde_json::Value {
        let settings = Settings {
            draft_notes_window: true,
            ..Settings::default()
        };
        let mut worker = Worker::new(settings, false, true);
        let mut windows = FakeWindows::default();
        worker.handle(conn, OverlayMsg::LimitedChanged);
        worker.update(conn, &mut windows, now_ms);
        // The window's own push (the league card is pushed too).
        let (label, event) = windows
            .emitted()
            .into_iter()
            .rfind(|(label, _)| label == DRAFT_NOTES)
            .expect("a push");
        assert_eq!(label, DRAFT_NOTES);
        assert_eq!(event.name, "draft-notes-changed");
        event.payload
    }

    // 0.x DraftNotesControllerTest, as the command and the pushed view.

    #[test]
    fn renders_the_window_with_no_draft() {
        let (_dir, conn) = db("overlay-draft-none");
        assert_eq!(command(&conn, NOW), None);
        assert_eq!(pushed(&conn, NOW), json!({ "view": null }));
    }

    #[test]
    fn renders_the_current_pick_for_a_live_draft() {
        let (_dir, conn) = db("overlay-draft-live");
        picking_draft(&conn, 1, NOW);
        let pack: Vec<u32> = (154000..154012).collect();
        pick(&conn, 1, 3, &pack, Some(NOW + 41 * SECOND));
        sync(&conn, NOW);

        let view = command(&conn, NOW).expect("live");
        assert_eq!(view.draft_token, "q-draft-1");
        assert_eq!(view.run_key, None);
        assert_eq!(view.state, "picking");
        assert_eq!(view.current_ordinal, Some(3));
        assert_eq!(view.picks.len(), 1);

        let payload = pushed(&conn, NOW);
        let notes = &payload["view"];
        assert_eq!(notes["draftToken"], "q-draft-1");
        assert_eq!(notes["runKey"], json!(null));
        assert_eq!(notes["state"], "picking");
        assert_eq!(notes["currentOrdinal"], 3);
        assert_eq!(notes["picks"].as_array().map(Vec::len), Some(1));
        let first = &notes["picks"][0];
        assert_eq!(first["ordinal"], 3);
        assert_eq!(first["label"], "P1p3");
        assert_eq!(first["cardsInPack"], 12);
        assert_eq!(first["deadlineAtMs"], NOW + 41 * SECOND);
        assert_eq!(first["pickedName"], json!(null));
        assert_eq!(first["note"], json!(null));
        assert_eq!(first["noteSavedAtMs"], json!(null));
    }

    #[test]
    fn ships_every_pick_so_the_window_can_walk_back_without_a_round_trip() {
        let (_dir, mut conn) = db("overlay-draft-every-pick");
        picking_draft(&conn, 1, NOW);
        for ordinal in 1..=4 {
            pick(&conn, 1, ordinal, &[154000], None);
        }
        sync(&conn, NOW);
        note(&mut conn, 2, "wheeled the bomb");

        let view = command(&conn, NOW).expect("live");
        assert_eq!(view.picks.len(), 4);
        assert_eq!(view.current_ordinal, Some(4));
        assert_eq!(view.picks[1].note.as_deref(), Some("wheeled the bomb"));
        assert_eq!(view.picks[1].note_saved_at_ms, Some(NOW));

        let payload = pushed(&conn, NOW);
        assert_eq!(payload["view"]["picks"].as_array().map(Vec::len), Some(4));
        assert_eq!(payload["view"]["currentOrdinal"], 4);
        assert_eq!(payload["view"]["picks"][1]["note"], "wheeled the bomb");
        assert_eq!(payload["view"]["picks"][1]["noteSavedAtMs"], NOW);
    }

    /// 0.x "answers a partial reload of notes only": the push carries the
    /// notes and nothing else (no `serverNow`), the same view the command
    /// reads.
    #[test]
    fn the_push_carries_the_notes_only() {
        let (_dir, conn) = db("overlay-draft-notes-only");
        picking_draft(&conn, 1, NOW);
        pick(&conn, 1, 1, &[154000], None);
        sync(&conn, NOW);

        let payload = pushed(&conn, NOW);
        let keys: Vec<&String> = payload.as_object().expect("an object").keys().collect();
        assert_eq!(keys, vec!["view"]);
        assert_eq!(payload["view"]["currentOrdinal"], 1);
        let read = serde_json::to_value(command(&conn, NOW)).expect("as JSON");
        assert_eq!(payload["view"], read);
    }

    /// Spec ruling 12: notes are keyed by draft, so a draft that no league
    /// holds yet saves them (0.x refused: "not linked to a league yet").
    #[test]
    fn an_unlinked_draft_can_save_a_note() {
        let dir = TestDir::new("overlay-draft-unlinked-note");
        let (state, _events) = ready_state(&dir);
        // The tracker syncs on its own clock, so the draft is live now.
        let now = crate::wall_clock_ms();
        {
            let writer = store::open_writer(&state.db_path).expect("writer");
            account(&writer);
            picking_draft(&writer, 1, now);
            pick(&writer, 1, 1, &[154000, 154001], None);
            sync(&writer, now);
        }

        set_note(&state, "q-draft-1", 1, "Q took the removal", now).expect("saved");

        let reader = store::open_reader(&state.db_path).expect("reader");
        let view = command(&reader, now).expect("live");
        assert_eq!(view.run_key, None, "not linked to a league");
        assert_eq!(view.picks[0].note.as_deref(), Some("Q took the removal"));
        assert_eq!(view.picks[0].note_saved_at_ms, Some(now));
        let payload = pushed(&reader, now);
        assert_eq!(payload["view"]["picks"][0]["note"], "Q took the removal");
        crate::commands::stop_tracker(&state);
    }
}
