use mtgr::{HeaderError, HeaderGame, read_header};

fn game(number: u32, game_id: &str) -> String {
    format!(
        "[Mtgr \"0.3.0\"]\n[Event \"Modern League\"]\n[Player1 \"PlayerA\"]\n[Player2 \"PlayerB\"]\n\
         [Match \"AB12-cd34\"]\n[Game \"{number}\"]\n[MTGO_GameId \"{game_id}\"]\n\
         [MTGO_MatchId \"291004417\"]\n[Recorder \"MtgoReplay 0.3.0\"]\n\n1. Turn 1\n\n"
    )
}

#[test]
fn reads_token_ids_and_games() {
    let text = format!("{}{}", game(1, "966120001"), game(2, "966120002"));
    let header = read_header(&text).unwrap();
    assert_eq!(header.match_token, "ab12-cd34");
    assert_eq!(header.mtgo_match_id, Some(291004417));
    assert_eq!(header.recorder.as_deref(), Some("MtgoReplay 0.3.0"));
    assert_eq!(
        header.games,
        vec![
            HeaderGame {
                position: 0,
                game_number: 1,
                mtgo_game_id: Some(966120001)
            },
            HeaderGame {
                position: 1,
                game_number: 2,
                mtgo_game_id: Some(966120002)
            },
        ]
    );
}

#[test]
fn the_token_is_lowercased() {
    let text = game(1, "1").replace("AB12-cd34", "ABCDEF-0123");
    assert_eq!(read_header(&text).unwrap().match_token, "abcdef-0123");
}

#[test]
fn a_file_starting_at_game_two_keeps_its_numbers() {
    let header = read_header(&game(2, "966120002")).unwrap();
    assert_eq!(
        header.games,
        vec![HeaderGame {
            position: 0,
            game_number: 2,
            mtgo_game_id: Some(966120002)
        }]
    );
}

#[test]
fn non_numeric_ids_are_none() {
    let text = game(1, "x").replace("291004417", "y");
    let header = read_header(&text).unwrap();
    assert_eq!(header.mtgo_match_id, None);
    assert_eq!(header.games[0].mtgo_game_id, None);
}

#[test]
fn escaped_values_are_unescaped() {
    let text = game(1, "1").replace("AB12-cd34", "a\\\"b\\\\c");
    assert_eq!(read_header(&text).unwrap().match_token, "a\"b\\c");
}

#[test]
fn version_0_2_is_rejected() {
    let text = game(1, "1").replace("0.3.0\"]\n[Event", "0.2.0\"]\n[Event");
    assert_eq!(
        read_header(&text),
        Err(HeaderError::WrongVersion("0.2.0".into()))
    );
}

#[test]
fn no_match_tag_is_rejected() {
    let text = game(1, "1").replace("[Match \"AB12-cd34\"]\n", "");
    assert_eq!(read_header(&text), Err(HeaderError::NoMatchToken));
}

#[test]
fn a_game_without_a_game_number_is_rejected() {
    let text = game(1, "1").replace("[Game \"1\"]\n", "");
    assert_eq!(read_header(&text), Err(HeaderError::NotMtgr));
}

#[test]
fn text_that_is_not_mtgr_is_rejected() {
    assert_eq!(read_header("hello\nworld\n"), Err(HeaderError::NotMtgr));
    assert_eq!(read_header(""), Err(HeaderError::NotMtgr));
}
