//! Population against Plan Q's stub server and a running tracker. The four
//! passes are Q6's (`mymtgo_api::cards`, `api::reference`); these tests
//! prove the rows fed to them and the answers stored (0.x
//! `PopulateMissingCardDataTest`).

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use fixture_tools::TestDir;
use serde_json::{Value, json};
use tracker::TrackerEvent;

use super::*;
use crate::api::session::testing::device_api;
use crate::api::stub::{Recorded, Reply, Stub};
use crate::bridge::lock;
use crate::commands::AppState;
use crate::commands::tests::ready_state;

const NOW: i64 = 1_800_000_000_000;

/// A running tracker over an empty database.
pub(crate) struct Tracked {
    pub(crate) _dir: TestDir,
    pub(crate) state: AppState,
    _events: Receiver<TrackerEvent>,
}

pub(crate) fn tracked(label: &str) -> Tracked {
    let dir = TestDir::new(label);
    let (state, events) = ready_state(&dir);
    Tracked {
        _dir: dir,
        state,
        _events: events,
    }
}

impl Tracked {
    pub(crate) fn write(&self, sql: &str) {
        store::open_writer(&self.state.db_path)
            .expect("writer")
            .execute_batch(sql)
            .expect("write");
    }

    /// `(scryfall_id, name, rarity, color_identity)` of a row.
    pub(crate) fn card(
        &self,
        catalog_id: u32,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        store::open_reader(&self.state.db_path)
            .expect("reader")
            .query_row(
                "SELECT scryfall_id, name, rarity, color_identity FROM cards WHERE catalog_id = ?1",
                [catalog_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("a card row")
    }

    pub(crate) fn unresolved_count(&self) -> i64 {
        store::open_reader(&self.state.db_path)
            .expect("reader")
            .query_row(
                "SELECT COUNT(*) FROM cards WHERE scryfall_id IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("count")
    }

    /// One population run, with `card_data` as the `CardDataSource` folder.
    pub(crate) fn populate(
        &self,
        api: &ApiState,
        card_data: Option<PathBuf>,
    ) -> Result<(), String> {
        self.populate_asking(api, card_data, false)
    }

    /// One population run, a button's (`ask_unknown`) or not.
    pub(crate) fn populate_asking(
        &self,
        api: &ApiState,
        card_data: Option<PathBuf>,
        ask_unknown: bool,
    ) -> Result<(), String> {
        let db = self.state.db_path.clone();
        let reader = move || store::open_reader(&db).ok();
        let folder = move || card_data.clone();
        let resolver = lock(&self.state.tracker)
            .as_ref()
            .expect("tracker")
            .cards_resolver();
        let resolve = |tokens, resolutions, unknown_at_ms| {
            resolver.resolve(tokens, resolutions, unknown_at_ms)
        };
        let save_settings = |rows| resolver.save_settings(rows);
        populate(&Population {
            api,
            reader: &reader,
            card_data: &folder,
            resolve: &resolve,
            save_settings: &save_settings,
            now: &|| NOW,
            ask_unknown,
        })
    }
}

fn body(request: &Recorded) -> Value {
    serde_json::from_slice(&request.body).unwrap_or(Value::Null)
}

fn ids(request: &Recorded) -> Vec<i64> {
    body(request)["ids"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

fn strings(request: &Recorded, key: &str) -> Vec<String> {
    body(request)[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn answer(cards: Value) -> Reply {
    Reply::json(200, &cards.to_string())
}

fn asked_by_name(stub: &Stub) -> bool {
    stub.requests()
        .iter()
        .any(|request| !strings(request, "names").is_empty())
}

fn fixture_folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/cards/CardDataSource")
}

// 0.x "rebuilds the api client per chunk, so a key that expires mid-run
// still resolves every chunk": the key rotation is Q6's test
// (`api::reference`); through the worker, 60 rows go out in two chunks and
// every answer is stored.
#[test]
fn every_chunk_of_a_run_is_stored() {
    let stub = Stub::start(|request| {
        let answers: Vec<Value> = ids(request)
            .into_iter()
            .map(|id| {
                json!({"value": id, "scryfall_id": format!("scryfall-{id}"),
                    "oracle_id": format!("oracle-{id}"), "name": format!("Qcard {id}"),
                    "image": "https://example.test/card.png"})
            })
            .collect();
        answer(Value::Array(answers))
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-chunks");
    let values: Vec<String> = (1..=60).map(|id| format!("({id})")).collect();
    run.write(&format!(
        "INSERT INTO cards (catalog_id) VALUES {}",
        values.join(",")
    ));
    run.populate(&api.state, None).expect("populated");
    assert_eq!(stub.requests().len(), 2);
    assert_eq!(run.unresolved_count(), 0);
    assert_eq!(run.card(60).1.as_deref(), Some("Qcard 60"));
}

// 0.x "resolves a back face by retrying the front face catalog id, which
// sits two below it".
#[test]
fn resolves_a_back_face_by_retrying_the_front_face_two_below() {
    let stub = Stub::start(|request| {
        if ids(request).contains(&126_517) {
            answer(json!([{"value": 126517, "scryfall_id": "scryfall-boggart",
                "oracle_id": "oracle-boggart", "name": "Boggart Trawler // Boggart Bog",
                "image": "https://example.test/boggart.png"}]))
        } else {
            answer(json!([]))
        }
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-back-face");
    run.write("INSERT INTO cards (catalog_id) VALUES (126519)");
    run.populate(&api.state, None).expect("populated");
    let (scryfall, name, _, _) = run.card(126_519);
    assert_eq!(scryfall.as_deref(), Some("scryfall-boggart"));
    assert_eq!(name.as_deref(), Some("Boggart Trawler // Boggart Bog"));
}

// 0.x "leaves a card alone when the id two below it is a different
// single-faced card".
#[test]
fn leaves_a_card_alone_when_two_below_is_a_single_faced_card() {
    let stub = Stub::start(|request| {
        if ids(request).contains(&16_154) {
            answer(json!([{"value": 16154, "scryfall_id": "scryfall-vindicate",
                "oracle_id": "oracle-vindicate", "name": "Vindicate",
                "image": "https://example.test/vindicate.png"}]))
        } else {
            answer(json!([]))
        }
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-single-faced");
    run.write("INSERT INTO cards (catalog_id) VALUES (16156)");
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.card(16_156).0, None);
}

/// Fails the id lookup of 61460 with `own`, and answers 61458 (two below)
/// with a multi-face card.
fn evolution_stub(own: u16) -> Stub {
    Stub::start(move |request| {
        let asked = ids(request);
        if asked.contains(&61_460) {
            if own == 200 {
                return answer(json!([]));
            }
            return Reply::json(own, r#"{"message":"Too Many Attempts."}"#);
        }
        if asked.contains(&61_458) {
            return answer(json!([{"value": 61458, "scryfall_id": "scryfall-captive",
                "oracle_id": "oracle-captive",
                "name": "Ulvenwald Captive // Ulvenwald Abomination",
                "image": "https://example.test/captive.png"}]));
        }
        answer(json!([]))
    })
}

// 0.x "does not guess a neighbouring back face for a card whose own lookup
// failed".
#[test]
fn does_not_guess_a_back_face_for_a_card_whose_own_lookup_failed() {
    let stub = evolution_stub(429);
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-failed-lookup");
    run.write("INSERT INTO cards (catalog_id, name) VALUES (61460, 'Eldritch Evolution')");
    run.populate(&api.state, None)
        .expect("a failed request is no error");
    let (scryfall, name, _, _) = run.card(61_460);
    assert_eq!(scryfall, None);
    assert_eq!(name.as_deref(), Some("Eldritch Evolution"));
    assert!(!asked_by_name(&stub));
}

#[test]
fn a_back_face_is_resolved_only_after_its_own_lookup_answered_nothing() {
    let run = tracked("populate-back-face-after-nothing");
    run.write("INSERT INTO cards (catalog_id) VALUES (61460)");
    // Its own lookup failed: nothing is guessed, and only the id was asked.
    let failing = evolution_stub(500);
    let api_failing = device_api(&failing.base_url, false);
    run.populate(&api_failing.state, None).expect("ran");
    assert_eq!(run.card(61_460).0, None);
    let asked: Vec<Vec<i64>> = failing.requests().iter().map(ids).collect();
    assert_eq!(asked, vec![vec![61_460]]);
    // The next run's lookup answers nothing: now the back face is tried.
    let answering = evolution_stub(200);
    let api_answering = device_api(&answering.base_url, false);
    run.populate(&api_answering.state, None).expect("ran");
    let (scryfall, name, _, _) = run.card(61_460);
    assert_eq!(scryfall.as_deref(), Some("scryfall-captive"));
    assert_eq!(
        name.as_deref(),
        Some("Ulvenwald Captive // Ulvenwald Abomination")
    );
}

// 0.x "fetches cards missing a scryfall id even when every card already has
// a name".
#[test]
fn fetches_cards_missing_a_scryfall_id_even_when_named() {
    let stub = Stub::start(|_| {
        answer(json!([{"value": 12345, "scryfall_id": "scryfall-bolt",
            "oracle_id": "oracle-bolt", "name": "Lightning Bolt",
            "image": "https://example.test/bolt.png"}]))
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-named");
    run.write("INSERT INTO cards (catalog_id, name) VALUES (12345, 'Lightning Bolt')");
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.card(12_345).0.as_deref(), Some("scryfall-bolt"));
}

// 0.x "resolves a card the api only knows by name, such as a split card
// half".
#[test]
fn resolves_a_card_the_api_only_knows_by_name() {
    let stub = Stub::start(|request| {
        if !strings(request, "names").contains(&"Tear".to_string()) {
            return answer(json!([]));
        }
        answer(
            json!([{"query": "Tear", "value": null, "scryfall_id": "scryfall-weartear",
            "oracle_id": "oracle-weartear", "name": "Wear // Tear",
            "image": "https://example.test/weartear.png"}]),
        )
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-by-name");
    run.write("INSERT INTO cards (catalog_id, name) VALUES (48556, 'Tear')");
    run.populate(&api.state, None).expect("populated");
    let (scryfall, name, _, _) = run.card(48_556);
    assert_eq!(scryfall.as_deref(), Some("scryfall-weartear"));
    assert_eq!(name.as_deref(), Some("Wear // Tear"));
}

// 0.x "does not ask by name for a card that has no name".
#[test]
fn does_not_ask_by_name_for_a_card_without_a_name() {
    let stub = Stub::start(|_| answer(json!([])));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-nameless");
    run.write("INSERT INTO cards (catalog_id) VALUES (99999)");
    run.populate(&api.state, None).expect("populated");
    assert!(!stub.requests().is_empty());
    assert!(!asked_by_name(&stub));
}

// 0.x "finds a front face one below when the printing has no foil id
// between them".
#[test]
fn finds_a_front_face_one_below_without_a_foil_id_between() {
    let stub = Stub::start(|request| {
        let asked = ids(request);
        if asked.contains(&154_067) {
            return answer(
                json!([{"value": 154067, "scryfall_id": "scryfall-riddlemaster",
                "oracle_id": "oracle-riddlemaster", "name": "Gollum, Riddle Master",
                "image": "https://example.test/riddle.png"}]),
            );
        }
        if asked.contains(&154_068) {
            return answer(json!([{"value": 154068, "scryfall_id": "scryfall-slinker",
                "oracle_id": "oracle-slinker", "name": "Gollum, Silent Slinker // Meager Meal",
                "image": "https://example.test/slinker.png"}]));
        }
        answer(json!([]))
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-one-below");
    run.write("INSERT INTO cards (catalog_id) VALUES (154069)");
    run.populate(&api.state, None).expect("populated");
    let (scryfall, name, _, _) = run.card(154_069);
    assert_eq!(scryfall.as_deref(), Some("scryfall-slinker"));
    assert_eq!(
        name.as_deref(),
        Some("Gollum, Silent Slinker // Meager Meal")
    );
}

// 0.x "prefers the front face two below when both neighbours are
// multi-face".
#[test]
fn prefers_the_front_face_two_below() {
    let stub = Stub::start(|request| {
        let asked = ids(request);
        if asked.contains(&126_517) {
            return answer(json!([{"value": 126517, "scryfall_id": "scryfall-nonfoil",
                "oracle_id": "oracle-boggart", "name": "Boggart Trawler // Boggart Bog",
                "image": "https://example.test/boggart.png"}]));
        }
        if asked.contains(&126_518) {
            return answer(json!([{"value": 126518, "scryfall_id": "scryfall-foil",
                "oracle_id": "oracle-boggart", "name": "Boggart Trawler // Boggart Bog",
                "image": "https://example.test/boggart.png"}]));
        }
        answer(json!([]))
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-two-below");
    run.write("INSERT INTO cards (catalog_id) VALUES (126519)");
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.card(126_519).0.as_deref(), Some("scryfall-nonfoil"));
}

fn cat_stub(id_known: bool) -> Stub {
    Stub::start(move |request| {
        let mut answers = Vec::new();
        if id_known && ids(request).contains(&125_873) {
            answers.push(
                json!({"value": "125873", "scryfall_id": "mh3-cat", "oracle_id": "o-cat",
                "name": "Cat", "type": "Token Creature", "layout": "token", "set": "TMH3",
                "image": "https://cards.scryfall.io/mh3-cat.jpg"}),
            );
        }
        if strings(request, "tokens").contains(&"Cat".to_string()) {
            answers.push(
                json!({"scryfall_id": "green-cat", "oracle_id": "o-cat", "name": "Cat",
                "type": "Token Creature", "layout": "token", "set": "PLST",
                "image": "https://cards.scryfall.io/green-cat.jpg"}),
            );
        }
        answer(Value::Array(answers))
    })
}

// 0.x "resolves a token by its catalog id ahead of its name".
#[test]
fn resolves_a_token_by_its_catalog_id_ahead_of_its_name() {
    let stub = cat_stub(true);
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-token-by-id");
    run.write(
        "INSERT INTO cards (catalog_id, name, type, rarity)
         VALUES (125873, 'Cat', 'Token Creature', 'token')",
    );
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.card(125_873).0.as_deref(), Some("mh3-cat"));
}

// 0.x "still resolves a token by name when its catalog id is unknown".
#[test]
fn still_resolves_a_token_by_name_when_its_catalog_id_is_unknown() {
    let stub = cat_stub(false);
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-token-by-name");
    run.write(
        "INSERT INTO cards (catalog_id, name, type, rarity)
         VALUES (999001, 'Cat', 'Token Creature', 'token')",
    );
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.card(999_001).0.as_deref(), Some("green-cat"));
}

#[test]
fn population_runs_while_offline_and_before_sign_in() {
    let stub = Stub::start(|_| {
        answer(json!([{"value": 12345, "scryfall_id": "scryfall-bolt",
            "name": "Lightning Bolt", "image": "https://example.test/bolt.png"}]))
    });
    // Offline, no account linked.
    let api = device_api(&stub.base_url, true);
    let run = tracked("populate-offline");
    run.write("INSERT INTO cards (catalog_id) VALUES (12345)");
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.card(12_345).0.as_deref(), Some("scryfall-bolt"));
    let seen = stub.requests();
    assert_eq!(seen[0].target, "/api/cards");
    assert_eq!(seen[0].header("x-api-key"), Some("key-one"));
}

#[test]
fn nameless_stubs_get_their_token_names_from_mtgos_xml_first() {
    // The API knows neither the id nor the name: the XML names stay.
    let stub = Stub::start(|_| answer(json!([])));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-token-xml");
    run.write("INSERT INTO cards (catalog_id) VALUES (1001), (1002), (9999)");
    run.populate(&api.state, Some(fixture_folder()))
        .expect("populated");
    assert_eq!(
        run.card(1001),
        (
            None,
            Some("Goblin".to_string()),
            Some("token".to_string()),
            Some("R".to_string())
        )
    );
    assert_eq!(run.card(1002).1, None, "a foil clone gives nothing");
    // The token was asked about by id and name in one call, as a token.
    let token_calls: Vec<Recorded> = stub
        .requests()
        .into_iter()
        .filter(|request| !strings(request, "tokens").is_empty())
        .collect();
    assert_eq!(token_calls.len(), 1);
    assert_eq!(
        strings(&token_calls[0], "tokens"),
        vec!["Goblin".to_string()]
    );
    assert_eq!(ids(&token_calls[0]), vec![1001]);
}

#[test]
fn a_token_answer_without_an_identity_keeps_the_xml_identity() {
    let stub = Stub::start(|request| {
        if ids(request).contains(&1006) {
            return answer(
                json!([{"value": 1006, "scryfall_id": "q-food", "name": "Food",
                "layout": "token", "image": "https://cards.scryfall.io/food.jpg"}]),
            );
        }
        answer(json!([]))
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-token-identity");
    run.write("INSERT INTO cards (catalog_id) VALUES (1006)");
    run.populate(&api.state, Some(fixture_folder()))
        .expect("populated");
    let (scryfall, name, rarity, identity) = run.card(1006);
    assert_eq!(scryfall.as_deref(), Some("q-food"));
    assert_eq!(name.as_deref(), Some("Food"));
    assert_eq!(rarity.as_deref(), Some("token"));
    assert_eq!(identity.as_deref(), Some("W,U"));
}

#[test]
fn a_run_with_nothing_to_resolve_sends_nothing() {
    let stub = Stub::start(|_| answer(json!([])));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-nothing");
    run.write(
        "INSERT INTO cards (catalog_id, scryfall_id, name, mana_cost) VALUES (1, 'q', 'Qdone', '{R}')",
    );
    run.populate(&api.state, Some(fixture_folder()))
        .expect("populated");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

// 0.x PopulateTokensFromXmlTest "returns 0 when data path is missing".
#[test]
fn a_missing_card_data_folder_gives_no_tokens() {
    let dir = TestDir::new("populate-no-folder");
    assert_eq!(find_card_data_source(&dir.join("nowhere")), None);
    assert!(read_tokens(None, &[1001]).is_empty());
    dir.write("CardDataSource/CARDNAME_STRING.xml", b"<X/>");
    assert!(
        read_tokens(Some(&dir.join("CardDataSource")), &[1001]).is_empty(),
        "no token file"
    );
}

#[test]
fn the_first_card_data_source_folder_is_found_depth_first() {
    let dir = TestDir::new("populate-find-folder");
    dir.write("data/a/deep/CardDataSource/client_TOK.xml", b"");
    dir.write("data/b/CardDataSource/client_TOK.xml", b"");
    dir.write("data/0-file", b"");
    assert_eq!(
        find_card_data_source(&dir.join("data")),
        Some(dir.join("data/a/deep/CardDataSource"))
    );
    let tokens = read_tokens(Some(&fixture_folder()), &[1001, 1004]);
    let names: Vec<&str> = tokens.iter().map(|token| token.name.as_str()).collect();
    assert_eq!(names, vec!["Goblin", "Treasure"]);
}

impl Tracked {
    fn mana_cost(&self, catalog_id: u32) -> Option<String> {
        store::open_reader(&self.state.db_path)
            .expect("reader")
            .query_row(
                "SELECT mana_cost FROM cards WHERE catalog_id = ?1",
                [catalog_id],
                |row| row.get(0),
            )
            .expect("a card row")
    }

    fn mana_costs_filled(&self) -> bool {
        let conn = store::open_reader(&self.state.db_path).expect("reader");
        store::cards::mana_costs_filled(&conn).expect("marker")
    }

    /// `count` resolved spells from catalog id 1, stored without a cost.
    fn resolved_without_costs(&self, count: u32) {
        let values: Vec<String> = (1..=count)
            .map(|id| format!("({id}, 'q-{id}', 'Qcard {id}', 'Instant')"))
            .collect();
        self.write(&format!(
            "INSERT INTO cards (catalog_id, scryfall_id, name, type) VALUES {}",
            values.join(",")
        ));
    }
}

/// Answers every asked id, with `"{1}"` as its cost when `with_costs`,
/// else as the API answered before it sent costs (no `mana_cost` key).
fn costs_stub(with_costs: bool) -> Stub {
    Stub::start(move |request| {
        let answers: Vec<Value> = ids(request)
            .into_iter()
            .map(|id| {
                let mut card = json!({"value": id, "scryfall_id": format!("q-{id}"),
                    "oracle_id": format!("o-{id}"), "name": format!("Qcard {id}"),
                    "type": "Instant", "image": "https://example.test/card.png"});
                if with_costs {
                    card["mana_cost"] = json!("{1}");
                }
                card
            })
            .collect();
        answer(Value::Array(answers))
    })
}

#[test]
fn rows_resolved_before_the_api_sent_costs_are_looked_up_again_once() {
    let stub = Stub::start(|request| {
        let answers: Vec<Value> = ids(request)
            .into_iter()
            .filter_map(|id| match id {
                1 => Some(json!({"value": 1, "scryfall_id": "q-1", "oracle_id": "o-1",
                    "name": "Qspell", "type": "Instant", "mana_cost": "{R}"})),
                3 => Some(json!({"value": 3, "scryfall_id": "q-3", "oracle_id": "o-3",
                    "name": "Qfront // Qback", "type": "Creature", "mana_cost": null})),
                _ => None,
            })
            .collect();
        answer(Value::Array(answers))
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-mana-cost-refresh");
    run.write(
        "INSERT INTO cards (catalog_id, scryfall_id, name, type) VALUES
             (1, 'q-1', 'Qspell', 'Instant'),
             (2, 'q-2', 'Qmountain', 'Basic Land'),
             (3, 'q-3', 'Qfront // Qback', 'Creature');
         INSERT INTO cards (catalog_id, scryfall_id, name, type, mana_cost)
             VALUES (4, 'q-4', 'Qisland', 'Basic Land', '');",
    );
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.mana_cost(1).as_deref(), Some("{R}"));
    assert_eq!(run.mana_cost(3), None, "the API holds no cost for it");
    assert_eq!(run.mana_cost(4).as_deref(), Some(""), "a land stays empty");
    let asked: Vec<i64> = stub.requests().iter().flat_map(ids).collect();
    assert!(asked.contains(&1) && asked.contains(&3));
    assert!(
        !asked.contains(&2) && !asked.contains(&4),
        "lands are not asked"
    );
    assert!(run.mana_costs_filled());

    // Done: a card the API has no cost for is never asked about again.
    let before = stub.requests().len();
    run.populate(&api.state, None).expect("populated again");
    assert_eq!(stub.requests().len(), before);
}

#[test]
fn an_api_that_sends_no_costs_is_asked_once_a_run_and_the_refresh_waits() {
    let stub = costs_stub(false);
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-mana-cost-old-api");
    run.resolved_without_costs(120);
    run.populate(&api.state, None).expect("populated");
    assert_eq!(stub.requests().len(), 1, "one probe, not every chunk");
    assert!(!run.mana_costs_filled());
    assert_eq!(run.mana_cost(1), None);

    run.populate(&api.state, None).expect("populated again");
    assert_eq!(stub.requests().len(), 2, "asked again on the next run");

    // Once the API sends costs, the next run fills every row and finishes.
    let sending = costs_stub(true);
    let api = self::device_api(&sending.base_url, false);
    run.populate(&api.state, None)
        .expect("populated with costs");
    assert_eq!(sending.requests().len(), 3, "120 rows in chunks of 50");
    assert_eq!(run.mana_cost(120).as_deref(), Some("{1}"));
    assert!(run.mana_costs_filled());
}

#[test]
fn a_failed_refresh_request_leaves_the_rest_for_the_next_run() {
    let stub = Stub::start(|request| {
        if ids(request).contains(&51) {
            return Reply::json(429, r#"{"message":"Too Many Attempts."}"#);
        }
        costs_answer(request)
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-mana-cost-failed");
    run.resolved_without_costs(60);
    run.populate(&api.state, None)
        .expect("a failed request is no error");
    assert_eq!(run.mana_cost(50).as_deref(), Some("{1}"));
    assert_eq!(run.mana_cost(51), None);
    assert!(!run.mana_costs_filled());

    // The next run asks only for the rows still without a cost.
    let sending = costs_stub(true);
    let api = self::device_api(&sending.base_url, false);
    run.populate(&api.state, None).expect("populated again");
    let asked: Vec<i64> = sending.requests().iter().flat_map(ids).collect();
    assert_eq!(asked, (51..=60).collect::<Vec<i64>>());
    assert_eq!(run.mana_cost(60).as_deref(), Some("{1}"));
    assert!(run.mana_costs_filled());
}

fn costs_answer(request: &Recorded) -> Reply {
    let answers: Vec<Value> = ids(request)
        .into_iter()
        .map(|id| {
            json!({"value": id, "scryfall_id": format!("q-{id}"), "name": format!("Qcard {id}"),
                "type": "Instant", "mana_cost": "{1}"})
        })
        .collect();
    answer(Value::Array(answers))
}

impl Tracked {
    fn mana_cost_attempts(&self) -> u32 {
        let conn = store::open_reader(&self.state.db_path).expect("reader");
        store::cards::mana_cost_attempts(&conn).expect("attempts")
    }
}

#[test]
fn an_answer_whose_only_cost_is_null_still_shows_the_api_sends_costs() {
    let stub = Stub::start(|_| {
        answer(
            json!([{"value": 1, "scryfall_id": "q-1", "oracle_id": "o-1",
            "name": "Qfront // Qback", "type": "Creature", "mana_cost": null}]),
        )
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-mana-cost-null-only");
    run.resolved_without_costs(1);
    run.populate(&api.state, None).expect("populated");
    assert_eq!(run.mana_cost(1), None);
    assert!(run.mana_costs_filled());
}

#[test]
fn a_request_failing_before_any_answer_stops_the_refresh_for_the_run() {
    let stub = Stub::start(|_| Reply::json(429, r#"{"message":"Too Many Attempts."}"#));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-mana-cost-throttled");
    run.resolved_without_costs(120);
    run.populate(&api.state, None)
        .expect("a failed request is no error");
    assert_eq!(stub.requests().len(), 1, "one request, not every chunk");
    assert!(!run.mana_costs_filled());
    assert_eq!(run.mana_cost_attempts(), 0, "no costs seen: not an attempt");
}

#[test]
fn a_request_that_always_fails_is_given_up_after_a_few_runs() {
    let stub = Stub::start(|request| {
        if ids(request).contains(&51) {
            return Reply::json(500, r#"{"message":"Server Error"}"#);
        }
        costs_answer(request)
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-mana-cost-give-up");
    run.resolved_without_costs(60);
    for attempt in 1..store::cards::MANA_COST_ATTEMPTS_MAX {
        run.populate(&api.state, None).expect("populated");
        assert_eq!(run.mana_cost_attempts(), attempt);
        assert!(!run.mana_costs_filled());
    }
    run.populate(&api.state, None).expect("populated");
    assert!(run.mana_costs_filled(), "given up: bounded");
    let before = stub.requests().len();
    run.populate(&api.state, None).expect("populated again");
    assert_eq!(stub.requests().len(), before, "never asked again");
    assert_eq!(run.mana_cost(51), None);
}

#[test]
fn nothing_to_refresh_records_the_refresh_as_done_without_asking() {
    let stub = Stub::start(|_| answer(json!([])));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-mana-cost-nothing");
    run.write(
        "INSERT INTO cards (catalog_id, scryfall_id, name, type, mana_cost)
         VALUES (1, 'q-1', 'Qspell', 'Instant', '{R}')",
    );
    run.populate(&api.state, None).expect("populated");
    assert!(run.mana_costs_filled());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

// #501: an id the API answers without is not asked about again on the next
// run; a new app version asks about it once more.
#[test]
fn an_id_the_api_does_not_know_is_not_asked_every_run() {
    let stub = Stub::start(|_| answer(json!([])));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-api-unknown");
    run.write("INSERT INTO cards (catalog_id) VALUES (5)");
    run.populate(&api.state, None).expect("populated");
    let first = stub.requests().len();
    assert!(first > 0);
    run.populate(&api.state, None).expect("populated");
    assert_eq!(stub.requests().len(), first, "not asked again");
    run.write(
        "UPDATE settings SET value = '\"0.0.1\"' WHERE key = 'cards_unknown_checked_version'",
    );
    run.populate(&api.state, None).expect("populated");
    assert!(stub.requests().len() > first, "a new version asks again");
}

// Review of #501: "Download card data" (and the Cards page's "Fetch
// missing") asks about the ids the API did not know, within the week too.
#[test]
fn a_download_asks_about_unknown_ids_again() {
    let stub = Stub::start(|_| answer(json!([])));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-api-unknown-download");
    run.write("INSERT INTO cards (catalog_id) VALUES (5)");
    run.populate(&api.state, None).expect("populated");
    let first = stub.requests().len();
    run.populate_asking(&api.state, None, true)
        .expect("populated");
    assert!(stub.requests().len() > first, "the button asks again");
}

// A failed lookup is not an answer: the id is asked about on the next run.
#[test]
fn a_failed_lookup_does_not_mark_an_id_unknown() {
    let stub = Stub::start(|_| Reply::json(500, "boom"));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-api-failed");
    run.write("INSERT INTO cards (catalog_id) VALUES (5)");
    let _ = run.populate(&api.state, None);
    let first = stub.requests().len();
    let _ = run.populate(&api.state, None);
    assert!(stub.requests().len() > first);
}

// Review of #501: an id is marked unknown only when every request of the
// run was answered. Here the id pass answers without card 5 but its
// back-face pass fails, so card 5 is asked about again next run.
#[test]
fn a_failed_later_pass_marks_nothing_unknown() {
    let stub = Stub::start(|request| {
        if ids(request).contains(&5) {
            answer(json!([]))
        } else {
            Reply::json(500, "boom")
        }
    });
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-api-partial");
    run.write("INSERT INTO cards (catalog_id) VALUES (5)");
    let _ = run.populate(&api.state, None);
    let first = stub.requests().len();
    assert!(first >= 2, "the id pass and a back-face pass");
    let _ = run.populate(&api.state, None);
    assert!(stub.requests().len() > first, "asked again");
}

// Review of #501: a run whose lookups failed has not asked about every id,
// so a new version's recheck is not used up.
#[test]
fn a_failed_run_keeps_the_version_recheck() {
    let stub = Stub::start(|_| Reply::json(500, "boom"));
    let api = device_api(&stub.base_url, false);
    let run = tracked("populate-api-version-failed");
    run.write("INSERT INTO cards (catalog_id) VALUES (5)");
    let _ = run.populate(&api.state, None);
    let reader = store::open_reader(&run.state.db_path).expect("reader");
    assert_eq!(
        store::cards::unknown_checked_version(&reader).expect("read"),
        None
    );
}
