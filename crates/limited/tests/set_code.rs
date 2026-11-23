//! Ports the set code cases of 0.x `ResolveLeagueSetCodeTest`. Setting the
//! run's kind from the format code belongs to the leagues fold.

mod common;

use common::{drafted, facts, named};
use limited::{
    format_name, panel_play_format, resolve_set_code, set_code_from_picks,
    set_code_from_play_format,
};

const PANEL: &str = "12:00:13 [INF] (UI|Creating GameDetailsView) League\nEventToken=tok\nEventId=11039\nPlayFormatCd=HOBx3\nGameStructureCd= HOBx3";

#[test]
fn splits_repeated_set_codes_of_any_length() {
    let cases = [
        ("DHOBHOBHOB", Some("HOB")),
        ("DM21M21M21", Some("M21")),
        ("DLTRLTRLTR", Some("LTR")),
        ("HOBx3", Some("HOB")),
        ("MSHx3", Some("MSH")),
        ("S6FRA", Some("FRA")),
        ("DC03C03C03", Some("C03")),
        ("S6MSH", Some("MSH")),
        // Not one set repeated: the whole code names the format.
        ("DINSCUBE", Some("INSCUBE")),
        ("DSUPREME", Some("SUPREME")),
        ("DINPSAP", Some("INPSAP")),
        ("DHOBLTRMSH", Some("HOBLTRMSH")),
        ("S4STX4SOS", Some("STX4SOS")),
        ("CMODERN", None),
        ("D1xC06", None),
        ("SPACKWAR2", None),
        ("Dx", None),
        ("D", None),
        ("", None),
    ];

    for (code, expected) in cases {
        assert_eq!(
            set_code_from_play_format(code).as_deref(),
            expected,
            "format code {code:?}"
        );
    }
}

#[test]
fn sets_the_set_code_from_the_match_play_format() {
    assert_eq!(
        resolve_set_code(None, Some("DHOBHOBHOB"), None, None, &facts(vec![])).as_deref(),
        Some("HOB")
    );
}

#[test]
fn falls_back_to_the_league_panel_format_when_no_match_format_is_known() {
    assert_eq!(panel_play_format(PANEL).as_deref(), Some("HOBx3"));
    assert_eq!(
        resolve_set_code(None, None, Some(PANEL), None, &facts(vec![])).as_deref(),
        Some("HOB")
    );
    // A match format that names no set falls through to the panel too.
    assert_eq!(
        resolve_set_code(None, Some("CMODERN"), Some(PANEL), None, &facts(vec![])).as_deref(),
        Some("HOB")
    );
}

#[test]
fn falls_back_to_the_picked_cards_set_code() {
    let draft = drafted(&[154228, 153988, 999]);
    let cards = facts(vec![
        (154228, named("Qeld", "qeld", "HOB")),
        (153988, named("Qorn", "qorn", "HOB")),
        (999, named("Qyst", "qyst", "PLST")),
    ]);

    assert_eq!(set_code_from_picks(&draft, &cards).as_deref(), Some("HOB"));
    assert_eq!(
        resolve_set_code(None, None, None, Some(&draft), &cards).as_deref(),
        Some("HOB")
    );
}

#[test]
fn counts_each_picked_card_once_and_breaks_ties_by_first_pick() {
    // Three copies of one PLST card do not outvote two different HOB cards
    // (0.x counts card rows, not picks); a tie goes to the first picked.
    let draft = drafted(&[999, 999, 999, 1, 2]);
    let cards = facts(vec![
        (999, named("Qyst", "qyst", "PLST")),
        (1, named("Qeld", "qeld", "HOB")),
        (2, named("Qorn", "qorn", "HOB")),
    ]);
    assert_eq!(set_code_from_picks(&draft, &cards).as_deref(), Some("HOB"));

    let tied = drafted(&[999, 1]);
    assert_eq!(set_code_from_picks(&tied, &cards).as_deref(), Some("PLST"));
    assert_eq!(set_code_from_picks(&tied, &facts(vec![])), None);
}

#[test]
fn never_overwrites_an_existing_set_code() {
    assert_eq!(
        resolve_set_code(Some("HOB"), Some("DLTRLTRLTR"), None, None, &facts(vec![])).as_deref(),
        Some("HOB")
    );
    assert_eq!(
        resolve_set_code(None, Some("CMODERN"), None, None, &facts(vec![])),
        None
    );
}

#[test]
fn a_format_name_comes_from_the_game_structure_when_it_is_not_the_set_code() {
    let cases = [
        (Some("C03"), Some(" Vintage Cube x3"), Some("Vintage Cube")),
        (Some("INSCUBE"), Some("QuickCube"), Some("QuickCube")),
        (Some("INPSAP"), Some("IN PS AP"), Some("IN PS AP")),
        (Some("STX4SOS"), Some("STXx4 SOSx4"), Some("STXx4 SOSx4")),
        (None, Some("Vintage Cube x3"), Some("Vintage Cube")),
        // A single set: the set's own name is better than its code.
        (Some("HOB"), Some("HOBx3"), None),
        (Some("MSH"), Some("MSHx6"), None),
        (Some("HOB"), Some(""), None),
        (Some("HOB"), None, None),
        (None, None, None),
    ];

    for (code, structure, expected) in cases {
        assert_eq!(
            format_name(code, structure).as_deref(),
            expected,
            "{code:?} {structure:?}"
        );
    }
}
