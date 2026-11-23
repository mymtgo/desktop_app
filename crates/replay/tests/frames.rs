//! 0.x `tests/Feature/Actions/Replays/BuildReplayFramesTest.php`, ported as
//! `decorate_frames`, the repeated frame rule (replays spec ruling 4) and
//! 0.x `ShareReplay::linkToGame` (the `ShowControllerTest` link case).

use std::collections::BTreeMap;

use replay::{
    CardInfo, FrameText, ImageMode, ReplayFrame, decorate_frames, drop_repeats, link_to_game,
    normalise_time,
};
use serde_json::{Value, json};

/// 0.x `framesGame`: local.player (seat 1) against Opp_Name (seat 2), one
/// frame showing one card of catalog id 5001.
fn frames_game() -> Vec<ReplayFrame> {
    vec![ReplayFrame {
        timestamp: "18:00:01".to_string(),
        content: json!({
            "Players": [{"Id": 1, "Name": "local.player"}, {"Id": 2, "Name": "Opp_Name"}],
            "Cards": [{"Id": 10, "CatalogID": 5001, "Zone": "Battlefield", "Owner": 2}],
        }),
    }]
}

fn catalog(image: Option<&str>, local_image: Option<&str>) -> BTreeMap<u32, CardInfo> {
    BTreeMap::from([(
        5001,
        CardInfo {
            name: Some("Ragavan, Nimble Pilferer".to_string()),
            type_line: Some("Legendary Creature".to_string()),
            image: image.map(str::to_string),
            local_image: local_image.map(str::to_string),
            other_image: None,
        },
    )])
}

fn decorated_card(cards: &BTreeMap<u32, CardInfo>, mode: ImageMode) -> Value {
    let frames = decorate_frames(frames_game(), "local.player", cards, mode);
    frames[0].content["Cards"][0].clone()
}

#[test]
fn resolves_card_fields_and_marks_the_local_player() {
    let cards = catalog(Some("https://cards.scryfall.io/a.jpg"), None);

    let frames = decorate_frames(frames_game(), "local.player", &cards, ImageMode::App);

    let content = &frames[0].content;
    assert_eq!(
        content["Cards"][0]["name"],
        json!("Ragavan, Nimble Pilferer")
    );
    assert_eq!(content["Cards"][0]["type"], json!("Legendary Creature"));
    assert_eq!(content["Players"][0]["IsLocal"], json!(true));
    assert_eq!(content["Players"][1]["IsLocal"], json!(false));
}

/// 0.x `Card::image_url` serves the local cache first. v1 hands the window
/// the stored file name beside the remote URL and the window builds the
/// `card` scheme URL from it.
#[test]
fn prefers_the_local_image_on_the_desktop_page() {
    let cards = catalog(Some("https://cards.scryfall.io/a.jpg"), Some("a.jpg"));

    let card = decorated_card(&cards, ImageMode::App);

    assert_eq!(card["local_image"], json!("a.jpg"));
    assert_eq!(card["image"], json!("https://cards.scryfall.io/a.jpg"));
}

#[test]
fn uses_only_remote_https_images_in_portable_mode() {
    let cases = [
        (
            "remote https",
            Some("https://cards.scryfall.io/a.jpg"),
            json!("https://cards.scryfall.io/a.jpg"),
        ),
        ("plain http", Some("http://example.com/a.jpg"), Value::Null),
        ("no remote image", None, Value::Null),
    ];

    for (label, image, expected) in cases {
        let card = decorated_card(&catalog(image, Some("a.jpg")), ImageMode::Portable);

        assert_eq!(card["image"], expected, "{label}");
        assert!(
            !card.as_object().unwrap().contains_key("local_image"),
            "{label}"
        );
    }
}

#[test]
fn leaves_a_card_unknown_to_the_catalog_with_null_fields() {
    let card = decorated_card(&BTreeMap::new(), ImageMode::Portable);
    let card = card.as_object().unwrap();

    assert_eq!(card.get("name"), Some(&Value::Null));
    assert_eq!(card.get("type"), Some(&Value::Null));
    assert_eq!(card.get("image"), Some(&Value::Null));
}

#[test]
fn names_a_card_the_catalog_has_no_row_for_with_the_name_mtgo_gave_it() {
    let mut frames = frames_game();
    frames[0].content["Cards"][0]["Name"] = json!("Eldrazi Spawn");

    let frames = decorate_frames(frames, "local.player", &BTreeMap::new(), ImageMode::App);
    let card = frames[0].content["Cards"][0].as_object().unwrap();

    assert_eq!(card.get("name"), Some(&json!("Eldrazi Spawn")));
    assert!(!card.contains_key("Name"));
}

#[test]
fn app_frames_get_the_active_player_repaired_but_never_the_midnight_wrap() {
    let frame = |time: &str, step: &str| ReplayFrame {
        timestamp: time.to_string(),
        content: json!({
            "Turn": 1, "Step": step, "ActivePlayer": 1,
            "Players": [{"Id": 1, "Name": "local.player"}, {"Id": 2, "Name": "Opp_Name"}],
            "Cards": [],
        }),
    };
    let frames = vec![
        frame("2026-09-26T00:00:10Z", "Upkeep"),
        frame("2026-09-26T00:00:20Z", "EndOfTurn"),
        frame("2026-09-26T23:00:00Z", "Upkeep"),
    ];

    let decorated = decorate_frames(frames, "local.player", &BTreeMap::new(), ImageMode::App);

    let seats: Vec<Value> = decorated
        .iter()
        .map(|frame| frame.content["ActivePlayer"].clone())
        .collect();
    assert_eq!(seats, vec![json!(1), json!(1), json!(2)]);
    assert_eq!(decorated[0].timestamp, "2026-09-26T00:00:10Z");
}

fn text(time: &str, text: &str) -> FrameText {
    FrameText {
        time: time.to_string(),
        text: text.to_string(),
    }
}

/// Replays spec ruling 4: a run of identical frames keeps its first frame
/// and that frame's time; a game's last frame is always kept.
#[test]
fn drop_repeats_keeps_the_first_time_and_the_last_frame() {
    // A run of three at seconds 1, 1 and 4 keeps one frame at second 1.
    assert_eq!(
        drop_repeats(vec![
            text("12:00:01", "A"),
            text("12:00:01", "A"),
            text("12:00:04", "A"),
            text("12:00:05", "B"),
        ]),
        vec![text("12:00:01", "A"), text("12:00:05", "B")]
    );

    // A trailing repeat is kept with its own time.
    assert_eq!(
        drop_repeats(vec![
            text("12:00:01", "A"),
            text("12:00:02", "B"),
            text("12:00:03", "B"),
            text("12:00:09", "B"),
        ]),
        vec![
            text("12:00:01", "A"),
            text("12:00:02", "B"),
            text("12:00:09", "B"),
        ]
    );

    // A frame that differs only after a repeat is kept, and a frame equal
    // to an earlier one but not to the one just kept is kept too.
    assert_eq!(
        drop_repeats(vec![
            text("12:00:01", "A"),
            text("12:00:02", "A"),
            text("12:00:03", "B"),
            text("12:00:04", "A"),
            text("12:00:05", "C"),
        ]),
        vec![
            text("12:00:01", "A"),
            text("12:00:03", "B"),
            text("12:00:04", "A"),
            text("12:00:05", "C"),
        ]
    );

    // An empty or one-frame game is unchanged.
    assert_eq!(drop_repeats(Vec::new()), Vec::new());
    assert_eq!(
        drop_repeats(vec![text("12:00:01", "A")]),
        vec![text("12:00:01", "A")]
    );
    // Two identical frames: the last one is the game's last, so both stay.
    assert_eq!(
        drop_repeats(vec![text("12:00:01", "A"), text("12:00:02", "A")]),
        vec![text("12:00:01", "A"), text("12:00:02", "A")]
    );
}

#[test]
fn links_open_on_the_game() {
    let url = "https://mymtgo.test/players/Qplayer/replays/9b2f6f4e-0000-4000-8000-000000000001";

    assert_eq!(link_to_game(url, 1), format!("{url}?game=1"));
    assert_eq!(
        link_to_game("https://mymtgo.test/r/abc?ref=app", 3),
        "https://mymtgo.test/r/abc?ref=app&game=3"
    );
}

// Replays spec section 10, ruling 5: 0.x `game_timelines.timestamp` comes in
// three shapes. Time of day (with or without milliseconds) is kept as it is;
// an older `Y-m-d H:i:s` row keeps its time part, which is what the viewer
// reads and pairs log lines by.
#[test]
fn frame_times_keep_their_time_of_day() {
    assert_eq!(normalise_time("12:00:01"), "12:00:01");
    assert_eq!(normalise_time("12:00:01.250"), "12:00:01.250");
    assert_eq!(normalise_time("2026-01-02 03:04:06"), "03:04:06");
    assert_eq!(normalise_time("2026-08-19 10:01:00.750"), "10:01:00.750");
    // Anything else is kept as written (trimmed): nothing is lost. A pulled
    // bundle's ISO 8601 instant stays whole, as the website's replays carry
    // it.
    assert_eq!(
        normalise_time("2026-08-19T10:01:00Z"),
        "2026-08-19T10:01:00Z"
    );
    assert_eq!(normalise_time(" later "), "later");
    assert_eq!(normalise_time(""), "");
}
