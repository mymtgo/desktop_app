//! Synthetic events for unit tests, shaped like real mtgo.log lines.

use mtgo_protocol::{PreparedEvent, prepare_at};

pub struct Synth {
    offset: u64,
    at_ms: i64,
    local: Option<String>,
}

const JOINED_CONTEXT: &str =
    "Processing Registered Handler for GsMessageMessage in MatchJoinedEventUnderwayState";

impl Synth {
    pub fn new(local: Option<&str>) -> Self {
        Self {
            offset: 0,
            at_ms: 1_767_000_000_000,
            local: local.map(str::to_string),
        }
    }

    fn next(&mut self, text: String) -> PreparedEvent {
        self.offset += 100;
        self.at_ms += 1_000;
        prepare_at(1, self.offset, self.at_ms, &text, self.local.clone())
            .expect("synthetic event is whitelisted")
    }

    pub fn state_change(&mut self, token: &str, from: &str, to: &str) -> PreparedEvent {
        self.next(format!(
            "12:00:00 [INF] (Game Management|Match State Changed for {token} from {from} to {to})"
        ))
    }

    pub fn joined(
        &mut self,
        token: &str,
        match_id: u64,
        game_id: u64,
        kv: &[(&str, &str)],
    ) -> PreparedEvent {
        let kv_lines: Vec<String> = kv.iter().map(|(k, v)| format!("{k}={v}")).collect();
        self.next(format!(
            "12:00:00 [INF] (Game Management|{JOINED_CONTEXT}) Processor: X Message: {{\"MatchToken\":\"{token}\",\"MatchID\":{match_id},\"GameID\":{game_id},\"MetaMessage\":[]}} Receiver: Event Token={token}\n{}",
            kv_lines.join("\n")
        ))
    }

    pub fn entry(
        &mut self,
        token: &str,
        match_id: u64,
        game_id: u64,
        message: &str,
    ) -> PreparedEvent {
        let mut bytes = vec![0u8; 24];
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
        let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
        self.next(format!(
            "12:00:00 [INF] (Game Management|{JOINED_CONTEXT}) Processor: X Message: {{\"MatchToken\":\"{token}\",\"MatchID\":{match_id},\"GameID\":{game_id},\"MetaMessage\":[{}]}}",
            list.join(",")
        ))
    }

    pub fn game_state(&mut self, match_id: u64, game_id: u64, players: &[&str]) -> PreparedEvent {
        let players_json: Vec<String> = players
            .iter()
            .enumerate()
            .map(|(id, name)| format!("{{\"Id\":{id},\"Name\":\"{name}\",\"Life\":20}}"))
            .collect();
        self.next(format!(
            "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: {match_id}, Event ID: {match_id}) {{\"Players\":[{}],\"Cards\":[]}}",
            players_json.join(",")
        ))
    }

    pub fn deck_used(&mut self, game_id: u64, username: &str) -> PreparedEvent {
        self.next(format!(
            "12:00:00 [INF] (Twitch Info|Username: {username} Deck Used in Game ID: {game_id}) []"
        ))
    }

    // Tournament lines (event overlay). Each template is a line of
    // `fixtures/tournaments/challenge_showcase.log` with the fields the
    // decoder does not read left out; only tokens, ids and numbers vary.

    pub fn tournament_state(&mut self, token: &str, from: &str, to: &str) -> PreparedEvent {
        self.next(format!(
            "12:00:00 [INF] (Game Management|Tournament State Changed for {token} from {from} to {to}) "
        ))
    }

    /// `EventSyncData_t` at registration: no rounds yet. `cut_size: None` is
    /// an event without playoffs (a Preliminary).
    pub fn tournament_sync(
        &mut self,
        token: &str,
        event_id: u64,
        name: &str,
        cut_size: Option<u32>,
        prize_text: &str,
    ) -> PreparedEvent {
        let playoff = cut_size.map_or(String::new(), |count| {
            format!("{{\"PlayerCount\":{count},\"KeepProduct\":true}}")
        });
        self.next(format!(
            "12:00:00 [INF] (Game Management|Processing Registered Handler for EventSyncData_t in TournamentUninitializedState) Processor: TournamentUninitializedState Message: {{\"EventToken\":\"{token}\",\"EventID\":{event_id},\"PlayerEventType\":4,\"GameStructureCd\":\"CLEGACY\",\"Players\":[],\"Description\":\"{name}\",\"StartDate\":\"2026-11-22T17:00:00.0000000+01:00\",\"TournamentSyncData\":null,\"PremiereEventSyncData\":{{\"TournamentSyncData\":{{\"TournamentStructureCd\":\"SWISS\",\"NumberOfRounds\":0,\"RoundLengthInMinutes\":50,\"PrizeStructure\":{{\"Description\":\"{prize_text}\"}},\"Rounds\":[],\"EliminatedPlayers\":[]}},\"InPlayoffs\":false,\"Playoff\":[{playoff}]}},\"TournamentQueueSyncData\":null}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
        ))
    }

    /// `FlsTournamentRoundInfoMessage`, its outer `}` missing as in every real
    /// line. `matches`: (match token, the two players' login ids).
    pub fn round_info(
        &mut self,
        token: &str,
        event_id: u64,
        number: u32,
        matches: &[(&str, u64, u64)],
        byes: &[u64],
    ) -> PreparedEvent {
        let matches: Vec<String> = matches
            .iter()
            .map(|(match_token, first, second)| {
                format!(
                    "{{\"EventToken\":\"{match_token}\",\"ParentToken\":\"{token}\",\"Players\":[{{\"LoginID\":{first},\"State\":2,\"IsMatchConceded\":false}},{{\"LoginID\":{second},\"State\":2,\"IsMatchConceded\":false}}],\"Description\":\"Tournament:{event_id} Round:{number}\",\"StartDate\":\"2026-11-22T17:00:09.6680000+01:00\",\"MatchSyncData\":{{\"MatchCreateInfo\":{{\"MatchToken\":\"{match_token}\",\"MatchTimeInMinutes\":50}}}}}}"
                )
            })
            .collect();
        let byes: Vec<String> = byes.iter().map(u64::to_string).collect();
        self.next(format!(
            "12:00:00 [INF] (Game Management|Processing Registered Handler for FlsTournamentRoundInfoMessage in TournamentNotJoinedBetweenRoundsState) Processor: TournamentNotJoinedBetweenRoundsState Message: {{\"Token\":\"{token}\",\"ID\":{event_id},\"Round\":{{\"Number\":{number},\"Matches\":[{}],\"ByeList\":[{}]}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament",
            matches.join(","),
            byes.join(",")
        ))
    }

    /// `FlsTournamentRoundResultMessage` with this round's games only. `rows`:
    /// (login id, rank, games won, games lost); points 3 for a win, 1 for a
    /// draw.
    pub fn round_result(
        &mut self,
        token: &str,
        number: u32,
        rows: &[(u64, u32, u32, u32)],
    ) -> PreparedEvent {
        let rows: Vec<String> = rows
            .iter()
            .map(|(login_id, rank, won, lost)| {
                let points = match won.cmp(lost) {
                    std::cmp::Ordering::Greater => 3,
                    std::cmp::Ordering::Equal => 1,
                    std::cmp::Ordering::Less => 0,
                };
                format!(
                    "{{\"LoginID\":{login_id},\"Rank\":{rank},\"Points\":{points},\"OpponentResults\":[{{\"Round\":{number},\"LoginID\":0,\"Win\":{won},\"Loss\":{lost},\"Draw\":0,\"Bye\":0}}],\"OpponentMatchWinPercentage\":\"0.00000\",\"GameWinPercentage\":\"0.00000\",\"OpponentGameWinPercentage\":\"0.00000\"}}"
                )
            })
            .collect();
        self.next(format!(
            "12:00:00 [INF] (Game Management|Processing Registered Handler for FlsTournamentRoundResultMessage in TournamentNotJoinedRoundInProgressState) Processor: TournamentNotJoinedRoundInProgressState Message: {{\"Token\":\"{token}\",\"Round\":{number},\"Results\":[{}]}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament",
            rows.join(",")
        ))
    }

    pub fn eliminated(
        &mut self,
        token: &str,
        event_id: u64,
        login_id: u64,
        reason: u32,
    ) -> PreparedEvent {
        self.next(format!(
            "12:00:00 [INF] (Game Management|Processing Registered Handler for FlsTournamentPlayerIsEliminatedMessage in TournamentNotJoinedRoundInProgressState) Processor: TournamentNotJoinedRoundInProgressState Message: {{\"Token\":\"{token}\",\"ID\":{event_id},\"LoginID\":{login_id},\"EliminationReason\":{reason}}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
        ))
    }

    /// `FlsPlayerEventStateChangedMessage` (`InPlayoffs`).
    pub fn player_state(
        &mut self,
        token: &str,
        new_state: u32,
        in_playoffs: bool,
    ) -> PreparedEvent {
        self.next(format!(
            "12:00:00 [INF] (Game Management|Processing Registered Handler for FlsPlayerEventStateChangedMessage in TournamentNotJoinedBetweenRoundsState) Processor: TournamentNotJoinedBetweenRoundsState Message: {{\"Token\":\"{token}\",\"NewState\":{new_state},\"InPlayoffs\":{in_playoffs}}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtgo_protocol::EventKind;
    use mtgo_protocol::tournament::{TournamentFact, decode_fact, decode_player_state};

    const TOKEN: &str = "6ab6648d-3371-4daa-ae6b-e81bb8001148";
    const MATCH: &str = "3550e0e4-0000-4000-8000-000000000001";

    #[test]
    fn tournament_builders_classify_and_decode() {
        let mut synth = Synth::new(None);
        let decode = |event: &PreparedEvent| {
            assert_eq!(event.subjects.tournament_token.as_deref(), Some(TOKEN));
            decode_fact(event.kind, &event.text, Some(7))
        };

        let state = synth.tournament_state(TOKEN, "A", "PremierJoinedAwaitingMaxPlayersState");
        assert_eq!(state.kind, EventKind::TournamentStateChanged);
        assert!(matches!(
            decode(&state),
            Some(TournamentFact::StateChanged { .. })
        ));

        let sync = synth.tournament_sync(TOKEN, 5, "Legacy Challenge", Some(8), "Rounds: 33-64 6");
        assert_eq!(sync.kind, EventKind::TournamentSync);
        let Some(TournamentFact::Snapshot(snapshot)) = decode(&sync) else {
            panic!("a snapshot")
        };
        assert_eq!(snapshot.cut_size, Some(8));
        assert_eq!(snapshot.rounds_table.len(), 1);
        let prelim = synth.tournament_sync(TOKEN, 5, "Preliminary", None, "");
        let Some(TournamentFact::Snapshot(prelim)) = decode(&prelim) else {
            panic!("a snapshot")
        };
        assert_eq!(prelim.cut_size, None);

        let round = synth.round_info(TOKEN, 5, 1, &[(MATCH, 7, 8)], &[9]);
        assert_eq!(round.kind, EventKind::TournamentRoundInfo);
        let Some(TournamentFact::Round(pairing)) = decode(&round) else {
            panic!("a round")
        };
        assert_eq!(pairing.account_match_token.as_deref(), Some(MATCH));
        assert_eq!((pairing.match_tokens.len(), pairing.bye_count), (1, 1));

        let result = synth.round_result(TOKEN, 1, &[(7, 1, 2, 1), (8, 2, 1, 2)]);
        let Some(TournamentFact::Result(result)) = decode(&result) else {
            panic!("a result")
        };
        assert_eq!(result.rows, 2);
        assert_eq!(
            result.account.map(|row| (row.rank, row.points)),
            Some((1, 3))
        );

        let eliminated = synth.eliminated(TOKEN, 5, 7, 2);
        assert_eq!(
            decode(&eliminated),
            Some(TournamentFact::Eliminated {
                of_account: true,
                reason: 2
            })
        );

        let playoffs = synth.player_state(TOKEN, 24, true);
        assert_eq!(playoffs.kind, EventKind::TournamentPlayerState);
        assert_eq!(playoffs.subjects.tournament_token.as_deref(), Some(TOKEN));
        assert_eq!(
            decode_player_state(&playoffs.text),
            Some(TournamentFact::PlayerState {
                new_state: 24,
                in_playoffs: true
            })
        );
    }
}
