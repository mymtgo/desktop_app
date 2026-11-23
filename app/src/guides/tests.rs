//! 0.x `SideboardGuidesControllerTest` (three) and `EditControllerTest`
//! (three), the archetype page's block and deck select, and the edits
//! through the tracker with the pair they announce.

use super::*;
use crate::commands::tests::ready_state;
use fixture_tools::TestDir;

/// 2026-07-01T12:00:00Z.
const NOW: i64 = 1_782_907_200_000;
const UTC: Tz = chrono_tz::UTC;

fn insert(conn: &Connection, sql: &str, values: impl store::Params) -> i64 {
    conn.query_row(sql, values, |row| row.get(0)).expect(sql)
}

fn database(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    (dir, conn)
}

/// A Modern deck named `name`.
fn deck(conn: &Connection, name: &str) -> i64 {
    insert(
        conn,
        "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
         VALUES ('q-deck-' || (SELECT COUNT(*) FROM decks), ?1, 'CMODERN', 0, 0, 'file')
         RETURNING id",
        [name],
    )
}

fn archetype(conn: &Connection, name: &str) -> i64 {
    insert(
        conn,
        "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
                                 updated_at)
         VALUES ('q-' || ?1, ?1, 'modern', 'WUB', 'downloaded', 0, 0) RETURNING id",
        [name],
    )
}

fn guide(conn: &Connection, deck_id: i64, archetype_id: i64) -> i64 {
    insert(
        conn,
        "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3) RETURNING id",
        (deck_id, archetype_id, NOW),
    )
}

fn note(conn: &Connection, deck_id: i64, archetype_id: i64, body: &str) -> i64 {
    insert(
        conn,
        "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
        (deck_id, archetype_id, body, NOW),
    )
}

/// 0.x `EditControllerTest`'s cards and version: Lightning Bolt (202) four
/// in the maindeck, Rest in Peace (201) two in the sideboard.
fn editable_version(conn: &Connection, deck_id: i64) {
    for (catalog_id, oracle, name, card_type) in [
        (201, "o-rip", "Rest in Peace", "Enchantment"),
        (202, "o-bolt", "Lightning Bolt", "Instant"),
    ] {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type) VALUES (?1, ?2, ?3, ?4)",
            (catalog_id, oracle, name, card_type),
        )
        .expect("card");
    }
    let cards = [
        decks::DeckCard {
            catalog_id: 202,
            quantity: 4,
            sideboard: false,
        },
        decks::DeckCard {
            catalog_id: 201,
            quantity: 2,
            sideboard: true,
        },
    ];
    conn.execute(
        "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, 1)",
        (deck_id, decks::signature(&cards)),
    )
    .expect("version");
}

// ---- SideboardGuidesControllerTest -------------------------------------------

#[test]
fn lists_the_deck_guides_with_summaries() {
    let (_dir, conn) = database("guides-app-list");
    let deck_id = deck(&conn, "Qdeck");
    let blink = archetype(&conn, "Esper Blink");
    let g = guide(&conn, deck_id, blink);

    let guides = list_view(&conn, deck_id, UTC).expect("list");
    assert_eq!(guides.len(), 1);
    assert_eq!(guides[0].id, g);
    assert_eq!(guides[0].archetype_name, "Esper Blink");
    assert_eq!(guides[0].colors, ["W", "U", "B"]);
    assert_eq!(guides[0].match_record, None);
    assert_eq!(guides[0].updated_label, "1 Jul 2026");

    let json = serde_json::to_value(&guides).expect("json");
    assert_eq!(json[0]["archetypeName"], "Esper Blink");
    assert_eq!(json[0]["matchRecord"], serde_json::Value::Null);
    assert_eq!(json[0]["cardsIn"], 0);
}

/// 0.x deferred the create dialog's archetypes: the list carries none, the
/// dialog asks `archetype_options` for the deck's format after first
/// paint (the format's archetypes not retired, and the fallbacks).
#[test]
fn the_archetype_options_are_a_separate_read() {
    let (_dir, conn) = database("guides-app-options");
    let deck_id = deck(&conn, "Qdeck");
    let blink = archetype(&conn, "Esper Blink");
    let retired = archetype(&conn, "Qretired");
    conn.execute(
        "UPDATE archetypes SET retired_at = 1 WHERE id = ?1",
        [retired],
    )
    .expect("retire");
    guide(&conn, deck_id, blink);

    let json = serde_json::to_value(list_view(&conn, deck_id, UTC).expect("list")).expect("json");
    assert!(json[0].get("archetypes").is_none());

    let options = crate::matches::archetype_option_views(&conn, Some("CMODERN")).expect("options");
    let names: Vec<&str> = options.iter().map(|option| option.name.as_str()).collect();
    assert!(names.contains(&"Esper Blink"));
    assert!(names.contains(&"Homebrew"));
    assert!(!names.contains(&"Qretired"));
}

#[test]
fn an_archived_deck_lists_its_guides() {
    let (_dir, conn) = database("guides-app-archived");
    let deck_id = deck(&conn, "Qdeck");
    let g = guide(&conn, deck_id, archetype(&conn, "Esper Blink"));
    conn.execute("UPDATE decks SET archived_at = 1 WHERE id = ?1", [deck_id])
        .expect("archive");

    let guides = list_view(&conn, deck_id, UTC).expect("list");
    assert_eq!(guides.iter().map(|g| g.id).collect::<Vec<_>>(), [g]);
}

// ---- EditControllerTest ------------------------------------------------------

#[test]
fn the_editor_has_every_card_planned_quantities_notes_and_the_header() {
    let (_dir, conn) = database("guides-app-editor");
    let deck_id = deck(&conn, "Qdeck");
    editable_version(&conn, deck_id);
    let blink = archetype(&conn, "Esper Blink");
    let g = guide(&conn, deck_id, blink);
    conn.execute(
        "INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
         VALUES (?1, 'o-rip', 'in', 2)",
        [g],
    )
    .expect("plan");
    note(&conn, deck_id, blink, "Race them");

    let page = editor_view(&conn, deck_id, g, UTC)
        .expect("read")
        .expect("editor");
    assert_eq!(page.guide.id, g);
    assert_eq!(page.guide.archetype_name, "Esper Blink");
    assert!(page.has_version);
    assert_eq!(page.sideboard_size, 2);
    let columns = page.sideboard.expect("columns");
    assert_eq!(columns.bring_in.len(), 1);
    assert_eq!(columns.bring_in[0].oracle_id, "o-rip");
    assert_eq!(columns.bring_in[0].quantity, 2);
    assert_eq!(columns.bring_in[0].planned, Some(2));
    assert_eq!(columns.take_out.len(), 1);
    assert_eq!(columns.take_out[0].oracle_id, "o-bolt");
    assert_eq!(columns.take_out[0].quantity, 4);
    assert_eq!(columns.take_out[0].planned, None);
    assert_eq!(columns.take_out[0].type_line.as_deref(), Some("Instant"));
    assert_eq!(page.notes.current.len(), 1);
    assert_eq!(page.notes.current[0].body, "Race them");
    assert_eq!(page.notes.current[0].created_label, "1 Jul 2026, 12:00");

    let json =
        serde_json::to_value(editor_view(&conn, deck_id, g, UTC).expect("read")).expect("json");
    assert_eq!(json["sideboard"]["bringIn"][0]["oracleId"], "o-rip");
    assert_eq!(
        json["sideboard"]["takeOut"][0]["planned"],
        serde_json::Value::Null
    );
}

#[test]
fn a_deck_without_a_version_has_an_empty_editor() {
    let (_dir, conn) = database("guides-app-editor-empty");
    let deck_id = deck(&conn, "Qdeck");
    let g = guide(&conn, deck_id, archetype(&conn, "Esper Blink"));

    let page = editor_view(&conn, deck_id, g, UTC)
        .expect("read")
        .expect("editor");
    assert!(!page.has_version);
    assert_eq!(page.sideboard, None);
}

/// 0.x answered 404; v1 has no editor for it.
#[test]
fn a_guide_of_another_deck_is_refused() {
    let (_dir, conn) = database("guides-app-editor-other");
    let deck_id = deck(&conn, "Qdeck");
    let other = deck(&conn, "Qother");
    let g = guide(&conn, deck_id, archetype(&conn, "Esper Blink"));

    assert_eq!(editor_view(&conn, other, g, UTC).expect("read"), None);
    assert_eq!(editor_view(&conn, deck_id, g + 1, UTC).expect("read"), None);
}

// ---- The archetype page ------------------------------------------------------

#[test]
fn the_archetype_block_has_each_decks_plan_record_and_notes() {
    let (_dir, conn) = database("guides-app-block");
    let deck_id = deck(&conn, "Qdeck");
    editable_version(&conn, deck_id);
    let blink = archetype(&conn, "Esper Blink");
    let g = guide(&conn, deck_id, blink);
    conn.execute(
        "INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
         VALUES (?1, 'o-rip', 'in', 2), (?1, 'o-bolt', 'out', 2)",
        [g],
    )
    .expect("plan");
    note(&conn, deck_id, blink, "Race them");

    let rows = archetype_view(&conn, blink, UTC).expect("block");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.deck_name, "Qdeck");
    assert_eq!(row.guide_id, Some(g));
    assert_eq!((row.cards_in, row.cards_out), (2, 2));
    assert_eq!(row.planned_in[0].name, "Rest in Peace");
    assert_eq!(row.planned_out[0].name, "Lightning Bolt");
    assert_eq!(row.match_record, None);
    assert_eq!(row.notes[0].body, "Race them");
    assert_eq!(row.notes[0].created_label, "1 Jul 2026, 12:00");
}

#[test]
fn the_deck_select_is_the_formats_decks() {
    let (_dir, conn) = database("guides-app-deck-options");
    let modern = deck(&conn, "Qmodern");
    let legacy = deck(&conn, "Qlegacy");
    conn.execute(
        "UPDATE decks SET format = 'CLEGACY' WHERE id = ?1",
        [legacy],
    )
    .expect("format");
    let blink = archetype(&conn, "Esper Blink");

    let options = deck_options_view(&conn, blink).expect("options");
    assert_eq!(
        options,
        [GuideDeckOptionView {
            deck_id: modern,
            name: "Qmodern".to_string(),
            matches: 0,
        }]
    );
}

// ---- Edits through the tracker -------------------------------------------------

/// Every edit announces its pair; a refused one changes nothing and says
/// 0.x's message.
#[test]
fn the_edits_announce_their_pair() {
    let dir = TestDir::new("guides-app-edits");
    let (state, _events) = ready_state(&dir);
    let (deck_id, other, blink) = {
        let conn = store::open_writer(&state.db_path).expect("writer");
        let deck_id = deck(&conn, "Qdeck");
        editable_version(&conn, deck_id);
        (
            deck_id,
            deck(&conn, "Qother"),
            archetype(&conn, "Esper Blink"),
        )
    };
    let pair = GuidesChanged {
        deck_id,
        archetype_id: blink,
    };

    let (guide_id, changed) = create(&state, deck_id, blink, NOW).expect("create");
    assert_eq!(changed, pair);
    assert_eq!(
        create(&state, deck_id, blink, NOW),
        Err(CommandError::Refused(
            "A guide for this archetype already exists for this deck.".to_string()
        ))
    );

    let card = |oracle: &str, direction: &str, quantity: i64| GuideCardInputView {
        oracle_id: oracle.to_string(),
        direction: direction.to_string(),
        quantity,
    };
    let (errors, changed) =
        save_cards(&state, deck_id, guide_id, vec![card("o-rip", "in", 2)], NOW).expect("save");
    assert_eq!((errors, changed), (Vec::new(), Some(pair.clone())));
    let (errors, changed) =
        save_cards(&state, deck_id, guide_id, vec![card("o-rip", "in", 3)], NOW).expect("save");
    assert_eq!(
        errors,
        [PlanErrorView {
            index: 0,
            field: "quantity".to_string(),
            message: "Only 2 in the sideboard.".to_string(),
        }]
    );
    assert_eq!(changed, None);
    assert_eq!(
        save_cards(&state, other, guide_id, Vec::new(), NOW),
        Err(CommandError::Refused(
            "The guide was not found.".to_string()
        ))
    );

    let (note_id, changed) =
        add_one_note(&state, deck_id, blink, "  Race them  ".to_string(), NOW).expect("note");
    assert_eq!(changed, pair);
    assert_eq!(
        add_one_note(&state, deck_id, blink, "   ".to_string(), NOW),
        Err(CommandError::Refused(
            "The body field is required.".to_string()
        ))
    );
    {
        let reader = lock(&state.reader);
        let page = editor_view(&reader, deck_id, guide_id, UTC)
            .expect("read")
            .expect("editor");
        assert_eq!(page.guide.cards_in, 2);
        assert_eq!(page.notes.current[0].body, "Race them");
    }

    assert_eq!(delete_one_note(&state, note_id).expect("delete note"), pair);
    assert_eq!(
        delete_one_note(&state, note_id),
        Err(CommandError::Refused("The note was not found.".to_string()))
    );
    assert_eq!(
        delete(&state, other, guide_id),
        Err(CommandError::Refused(
            "The guide was not found.".to_string()
        ))
    );
    assert_eq!(delete(&state, deck_id, guide_id).expect("delete"), pair);
    assert!(
        list_view(&lock(&state.reader), deck_id, UTC)
            .expect("list")
            .is_empty()
    );
}
