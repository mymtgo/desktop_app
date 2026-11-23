//! Snapshot of every fixture's projected matches. Review changes deliberately.

use fixture_tools::{fixture_names, load_fixture, prepared_events};
use mtgo_match::Projector;

#[test]
fn league_matches_carry_their_league_token_and_game_structure() {
    let fixture = load_fixture("league_join_drop");
    let mut projector = Projector::new(vec![]);
    for event in prepared_events(&fixture) {
        projector.apply(&event);
    }
    projector.finalize(fixture.meta.now_ms());
    let views = projector.views();
    assert!(!views.is_empty(), "the fixture has a league match");
    for view in views {
        assert_eq!(
            view.league_token.as_deref(),
            Some("d2050286-53fd-4072-804f-190d6a3c030a"),
            "{}",
            view.token
        );
        assert_eq!(
            view.game_structure.as_deref(),
            Some("Modern"),
            "{}",
            view.token
        );
    }
}

#[test]
fn projected_matches_per_fixture() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let mut projector = Projector::new(vec![]);
        for event in prepared_events(&fixture) {
            projector.apply(&event);
        }
        projector.finalize(fixture.meta.now_ms());
        insta::assert_json_snapshot!(format!("projection_{name}"), projector.views());
    }
}
