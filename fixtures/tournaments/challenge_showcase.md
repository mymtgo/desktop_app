# Challenge fixture: notes

`challenge_showcase.log` is one tester MTGO session (2026-11-22), trimmed to whole entries and anonymised. It is the source of the event overlay's tournament facts (event overlay spec 7a; `mtgo_protocol::tournament`, `leagues::standing`).

## What the fixture keeps

- The two login lines. The account's fake name is in `challenge_showcase.meta.json`; its login id is `900001`.
- Every entry naming the account's **Legacy Showcase Challenge** (`6ab6648d-3371-4daa-ae6b-e81bb8001148`, event 12858913): snapshots, the registered deck, tournament state changes, rounds 1 and 2 (round info, the round 1 result), eliminations, player state changes, chat. Only the first registration list (`FlsPlayerEventNewPlayerOrderMessage`) is kept.
- Every entry naming the **Modern Challenge 32** seen from the lobby (`63c90ead-8c2c-407d-9f10-67378e0b96d0`, event 12858947): round 5 result to the semifinal pairings, its cut, eliminations and `InPlayoffs` changes.
- Every `Match State Changed` line of a match paired in either event's round info (both accounts' own matches included, without their games).
- The 4 finished Challenge snapshots (`PlayerEventState: 25`, full `Rounds[]` and `Results[]`) and one scheduled `Limited Preliminary` snapshot.
- The 4 UI entries whose stack traces name `FlsTournamentRoundInfoMessage` (`(UI|CreateAndFlipToDuelScene)`, `(UI|Flip To Game Side: DuelSceneViewModel)`).
- 15 entries of an unrelated lobby event (`271adf3d-...`) as noise.

It lives outside `fixtures/logs/` on purpose: those fixtures are swept by every whole-log suite (projection, engine, protocol snapshots), and this one carries matches without their games.

## Anonymisation

Every player name (`ScreenName`, `PlayerName`, player `Name`, the login name) was replaced by a fake of the same length (plain replace), so byte offsets match the trimmed source. The registered deck's name was replaced too. Every login id was then replaced by a fake of the same digit count, one fake per real id (consistent across the fixture, its tests and these notes), so relationships between players stay intact. The trimmed fixture was searched for every replaced name: no hits.

## Findings (line numbers refer to the original 33,421-line log, not to this fixture)

Correction after the backend review: `ceil(log2(players))` held on every Challenge here (27 to 46 players) but is not MTGO's rule above 128 players (the Showcase prize text says `213-384 9`). The rule used is in spec 7a and `docs/known_mtgo_lies_and_traps.md`.

Source: one MTGO session, `16:47:45` to `18:32:08` local (UTC+01:00), 33,421 lines, 12.8 MB. Account: login id `900001` (`(Login|MtGO Login Success) Username: ACCOUNT (900001)`, line 185). Player names and login ids below are placeholders.


## 1. Events the account joined

Only one: **Legacy Showcase Challenge**.

| Field | Value | Source |
|---|---|---|
| Event token / id | `6ab6648d-3371-4daa-ae6b-e81bb8001148` / `12858913` | `EventSyncData_t` `EventToken`, `EventID` (line 194) |
| Name | `Legacy Showcase Challenge` | `EventSyncData_t.Description` |
| Format | `CLEGACY` | `GameStructureCd` |
| Structure | `SWISS`, top 8 (`Playoff[0].PlayerCount: 8`, `MatchTypeCd: "BES"` Bo3), `RoundLengthInMinutes: 50`, `NumberOfRounds: 0`, `EventLevelTypeCd: "PREM"` | `PremiereEventSyncData.TournamentSyncData` |
| Start | `StartDate: "2026-11-22T17:00:00.0000000+01:00"` (client local time **with offset**; `EndDate` is start + 24 h and means nothing) | `EventSyncData_t.StartDate` |
| Players | 302 at round 1 (151 matches, no bye). Registration counter grew 220 (16:48:31) to 302 (16:59:25) | round 1 `FlsTournamentRoundInfoMessage`, `FlsTournamentRoundResultMessage.Results` (302 rows), `FlsPlayerEventNewPlayerOrderMessage.NewPlayerOrder` length |
| Entry fee | 40 x catalog 43081 | `EntryFees.EntryFees[].EntryFeeItems` |
| Registered deck | `FlsTournamentDeckRespMessage` at 16:48:41 (line 323), `ReturnCode: 0` | |

The account was **already registered before the log starts**: the first event line moves `TournamentUninitializedState` to `PremierJoinedAwaitingMaxPlayersState` at login. No registration moment is in this log.

**No league play, no second live match.** No `League...State`, no `League Token=`, no league `Description`. The only `League` lines are lobby list requests (`FlsLeague*ReqMessage`) and 32 `(Leagues|Insufficient Participants)` warnings at 16:48 (UI noise). The only account matches are the two tournament matches below. **Open question 2 of the spec is still open:** ask for a log with a league match during a Challenge wait.

Other events in the log are not the account's:
- At login, `EventSyncData_t` snapshots for about 30 scheduled Challenges and Preliminaries (`PlayerEventState: 18`, empty `Rounds`), 4 finished Challenges (Pioneer 16, Vintage 16, Modern 32, Legacy 16: `PlayerEventState: 25`, full `Rounds` with `Results`), and many Swiss and Cube drafts (`PlayerEventType: 3`).
- Live `NotJoined` messages for a **Modern Challenge 32** (`63c90ead...`, id 12858947) through its round 5 to its semifinals. This is a complete example of a Swiss-to-top-8 transition (section 2b).
- Account membership: the account's login id is in the round's `Matches[].Players[].LoginID`, the tournament states contain `Joined` but not `NotJoined`, and the match key-value block has `JoinedToGame=True` and `PlayerIds=900001,...`.

## 2. Timeline of the account's tournament

### 2a. Legacy Showcase Challenge 12858913

| Log time | Line | What | Message / state |
|---|---|---|---|
| 16:48:31 | 194, 197 | Snapshot at login, already registered | `EventSyncData_t` (220 players, `PlayerEventState: 19`), `TournamentUninitializedState -> PremierJoinedAwaitingMaxPlayersState` |
| 16:48:41 | 323 | Registered deck | `FlsTournamentDeckRespMessage` |
| 16:48:41 to 16:59:25 | 324 to 1838 | Registration count grows, 98 messages | `FlsPlayerEventNewPlayerOrderMessage` (`RevisionNumber` 307 to 404, list length 221 to 302) |
| 16:48:45 | 886, 896 | Duplicate snapshots (one handled in `TournamentUninitializedState` again) | `EventSyncData_t` x2 |
| 16:48:46 | 998 | Self transition | `PremierJoinedAwaitingMaxPlayersState -> PremierJoinedAwaitingMaxPlayersState` |
| 17:00:08 | 1892, 1905 | Round 1 pairings, logged **twice** in the same second under two states | `FlsTournamentRoundInfoMessage` (`Round.Number: 1`, 151 matches) |
| 17:00:08 | 1893 | | `PremierJoinedAwaitingMaxPlayersState -> PremierJoinedAwaitingStartState` |
| 17:00:08 | 2052 | R1 match starts, match 3550e0e4 (id 291193107) against login id 900067 | `TournamentMatchUninitializedState -> TournamentMatchJoinedEventUnderwayState` |
| 17:00:13 | 2177 | | `PremierJoinedAwaitingStartState -> TournamentJoinedRoundInProgressState` |
| 17:09:14, 17:17:50 | 8903, 12916 | Sideboarding after games 1 and 2 | `TournamentMatchJoinedSideboardingState`, then `...SideboardingDeckSubmittedState`, `...SideboardingDeckAcceptedState` |
| 17:22:03 | 16318 | R1 match ends, **won 2-1** (from round result) | `TournamentMatchJoinedEventUnderwayState -> TournamentMatchJoinedCompletedState` |
| 17:22 to 17:54 | | **32 minute wait**; the tournament state stays `TournamentJoinedRoundInProgressState` | |
| 17:15:08 to 17:49:52 | 11348 to 17862 | 15 opponents eliminated during R1 | `FlsTournamentPlayerIsEliminatedMessage` |
| 17:54:05 | 18038 | R1 standings: ACCOUNT rank 139 of 302, 3 points | `FlsTournamentRoundResultMessage` (`Round: 1`) |
| 17:54:05 | 18039 to 18042 | Round over | `TournamentJoinedRoundInProgressState -> TournamentJoinedBetweenRoundsState`; chat x2 "round 1 has completed. The next round will begin in approximately 2 minutes."; `FlsPlayerEventStateChangedMessage` `NewState: 24, InPlayoffs: false` |
| 17:56:12 | 18137 | | `FlsPlayerEventStateChangedMessage` `NewState: 23` |
| 17:56:14 | 18139 | Round 2 pairings: 143 matches + `ByeList: [9000169]` (302 - 15 = 287 players) | `FlsTournamentRoundInfoMessage` (`Round.Number: 2`) |
| 17:56:14 | 18168 | R2 match starts, match f6413b0c (id 291193969) against login id 9000413 | `TournamentMatchJoinedEventUnderwayState` |
| 17:56:19 | 18400 | | `TournamentJoinedBetweenRoundsState -> TournamentJoinedRoundInProgressState` |
| 18:26:49 | 33089 | R2 match ends after 3 games, **lost** (the match-end MetaMessage lists the opponent first, and lists the account first in R1, which it won) | `TournamentMatchJoinedCompletedState` |
| 18:32:08 | end | Log ends with 15 of 143 R2 matches still open. No R2 result, no R3. The account was not eliminated or dropped | |

So this log has only rounds 1 and 2. It has **no playoffs, no missed cut, no drop and no end for the account's event**.

### 2b. Swiss to top 8, seen live (Modern Challenge 32, NotJoined, 46 players)

| Log time | Line | What |
|---|---|---|
| 17:09:37 | 9068 to 9072 | `FlsTournamentRoundResultMessage` round 5, then `-> TournamentNotJoinedBetweenRoundsState`, chat "round 5 has completed...", `NewState: 24, InPlayoffs: false` |
| 17:11:39 | 9660, 9676 | `NewState: 23`, round info round 6 (13 matches + 1 bye) |
| 18:02:56 | 21195 to 21213 | Round result 6, then chat **"normal play has completed. The playoff rounds will begin in approximately 2 minutes."**, then **14 `FlsTournamentPlayerIsEliminatedMessage` with `EliminationReason: 1` in the same second** (everyone who missed the cut and had not dropped), then `NewState: 24, InPlayoffs: true` |
| 18:05:03 | 21684, 21685 | `NewState: 23, InPlayoffs: true`, round info **round 7 with 4 matches** (quarterfinals; round numbers continue after Swiss) |
| 18:19:14 to 18:20:56 | 29386 to 30327 | 4 quarterfinal losers, `EliminationReason: 1` |
| 18:20:57 | 30328 | Round result 7 (all 46 rows, ranks 1 to 46) |
| 18:23:03 | 31783, 31784 | Round info round 8, 2 matches (semifinals) |

The finished Challenges in the login snapshots confirm the shape. Pioneer 16 (32 players): rounds 1 to 5 are Swiss and rounds 6, 7 and 8 have 4, 2 and 1 matches. Modern 32 (44 players): 6 Swiss rounds, then 7, 8 and 9. After the final, the last round's `Results[].Rank` 1 to 8 is the playoff finish: 1 is the winner, 2 the finalist, 3 and 4 the semifinal losers, 5 to 8 the quarterfinal losers. Points keep accumulating through the playoffs.

### State names seen

- Tournament (account): `PremierJoinedAwaitingMaxPlayersState`, `PremierJoinedAwaitingStartState`, `TournamentJoinedRoundInProgressState`, `TournamentJoinedBetweenRoundsState`.
- Tournament (others): `PremierNotJoinedAwaitingMinPlayersState`, `TournamentNotJoinedRoundInProgressState`, `TournamentNotJoinedBetweenRoundsState`, `TournamentNotJoinedWaitingForFirstRoundToStartState`, `TournamentNotJoinedDraftingState`, `TournamentNotJoinedDeckBuildingState`, `TournamentNotJoinedFiredState`, `TournamentCompletedState`.
- Match (account): `TournamentMatchJoinedEventUnderwayState`, `TournamentMatchJoinedSideboardingState`, `TournamentMatchSideboardingDeckSubmittedState`, `TournamentMatchSideboardingDeckAcceptedState`, `TournamentMatchJoinedCompletedState`.
- Match (others in the same event): `TournamentMatchNotJoinedEventUnderwayState`, `...NotJoinedSideboardingState`, `...NotJoinedWaitingForGameToStartState`, `TournamentMatchClosedState`.
- `FlsPlayerEventStateChangedMessage.NewState`: 23 = round starting or in progress, 24 = between rounds. `PlayerEventState` in snapshots: 18 = scheduled, 19 = registering (the account's event), 23 = underway, 25 = finished. Drafts use 12 to 17.
- **Not seen for the account** (needs another log): the account's eliminated, dropped, playoff and completed states. A guess: `TournamentJoinedPlayoff...` or `TournamentCompletedState` with the account's login id in `FlsTournamentPlayerIsEliminatedMessage`.

## 3. Spec section 5 fields

| Field | Status | How |
|---|---|---|
| Event name | **FOUND** | `EventSyncData_t.Description` (`Legacy Showcase Challenge`). Also in the UI key-value block `Description=` after `CreateAndFlipToDuelScene` and `Flip To Details Side`. Showcase Challenges have no size suffix ("64"); only regular Challenges do (`Modern Challenge 32`). Sent at login and registration only, not updated |
| Round number | **FOUND**, live | `Description=Tournament:12858913 Round:N` in every match key-value block, and `FlsTournamentRoundInfoMessage.Round.Number` at each round start. Playoff rounds continue the numbering |
| Total Swiss rounds | **DERIVABLE** (`NumberOfRounds: 0`) | `ceil(log2(round 1 players))`: 302 gives 9 (matches the prize text "213-384 9"). Checks out on every finished Challenge: 27, 28 and 32 players give 5 rounds, 44 and 46 give 6. Round 1 players = 2 x round 1 matches + byes (or the round 1 `Results` count). The prize description's `Rounds:` table is **cut off at 255 characters** (`"... Rounds: 129-212 8; 213-384 9; 3"`), so it cannot be relied on. Confirmed after the fact by the "normal play has completed" chat or `InPlayoffs: true`. Preliminaries carry `NumberOfRounds: 4` and no playoff |
| Playoff stage, top cut size | **FOUND** | Cut size: `PremiereEventSyncData.Playoff[0].PlayerCount` (8). Stage: `FlsPlayerEventStateChangedMessage.InPlayoffs: true` (live), then the playoff round's match count in round info (4 = top 8, 2 = semifinal, 1 = final) or `round - swiss_rounds`. Seen live only for another player's event |
| Record | **DERIVABLE**, live | From the account's own tournament matches, the same way as leagues (match results via the match-end MetaMessage). Confirmed at each round end by `FlsTournamentRoundResultMessage.Results[LoginID=900001].OpponentResults[]` (`Win`/`Loss` are games; `Bye: 1` for a bye) and `Points` (3 per match win) |
| Standing / rank, player count | **FOUND, at round ends only** | `FlsTournamentRoundResultMessage.Results[].Rank` for the account's login id (139 after R1, unique ranks 1 to 302 with tiebreaks; OMW/GW/OGW are **strings** like `"0.33333"`). No mid-round updates. Player count: the result's row count (it **keeps eliminated and dropped players**: 302 rows). Live during registration: `NewPlayerOrder` length |
| Matches left in current round | **DERIVABLE**, live | Take the match `EventToken`s from the round's `FlsTournamentRoundInfoMessage.Round.Matches[]` and count those not yet moved to `TournamentMatchClosedState` (others) or `TournamentMatchJoinedCompletedState` (account). The client logs a state transition for **every** match of the joined event. R1: 74 open when the account finished, 14 at 17:40, 2 at 17:50, 0 at 17:54:04, then the round result at 17:54:05. Needs the round info JSON repaired (trap 1) or a regex over `"EventToken":"..."...,"PlayerEventType":1` |
| Next round start time | **DERIVABLE** (estimate) | Not logged ahead. Two estimates: (a) during the round, `Round.Matches[].StartDate` (`2026-11-22T17:00:09.668+01:00`) + `MatchTimeInMinutes` (50) gives the round clock end; real round ends were about 4 minutes later (17:54:05, for extra turns); (b) at the round end, the chat says "next round will begin in approximately 2 minutes" (real: 2 min 09 s). Card: "Round 2 by about 17:56" or "starts in about 2 minutes" |
| Registered / start time | **FOUND** | `EventSyncData_t.StartDate` (local time with UTC offset, so convertible to UTC ms). Registered: the `PremierJoinedAwaiting*` states. Before the start, `PremierJoinedAwaitingMaxPlayersState` (registration open), then `PremierJoinedAwaitingStartState` only in the second before round 1 |
| Final result: top 8 place | **DERIVABLE** (other events only) | The final round's `Results[].Rank` 1 to 8, or `FinalMatchResults[].FinalPlace` per match in snapshots (2 = match winner, 3 = loser; **not** a placing). Winner (trophy): won the final round's match |
| Missed cut rank | **DERIVABLE** | A burst of `FlsTournamentPlayerIsEliminatedMessage` with `EliminationReason: 1` right after the last Swiss round result plus "normal play has completed" chat. Rank = that round result's `Rank` |
| Dropped | **PARTLY**: `EliminationReason: 2` | Every R1 drop had 0 points, and R2 drops were 0-point players leaving after a second loss. `Reason: 2` = player dropped (inferred). `Reason: 1` = removed by the event (missed cut, playoff loss, and 4 R1 players within 5 seconds at 17:15, likely no-shows or disqualifications). The `EliminationReason` meanings are inferred, not documented. The account's own drop state was not seen |
| Eliminated | **FOUND** (others) | `FlsTournamentPlayerIsEliminatedMessage {Token, ID, LoginID, EliminationReason}`, one per player, no duplicates (48 for this event), arriving live. Also `TournamentSyncData.EliminatedPlayers` (login ids) in snapshots |

## 4. Traps

1. **Round info JSON is missing exactly one closing brace, every time.** All 4 `FlsTournamentRoundInfoMessage`s for the account's event and all 3 for the Modern Challenge fail `json.loads` and parse after adding one `}`. The traps doc says they are "sometimes truncated"; here the failure is systematic. `RepairJson`-style bracket balancing fixes it. Lines are 220 to 234 KB for 150 matches.
2. **v1 misclassifies UI stack traces as round info.** The app log has 4 warnings, `skipping a tournament event kind="tournament_round_info" ... reason="no tournament token"` (16:00:43Z and 16:56:25Z, at offsets 4384982, 4392081, 8772287, 8779207). These offsets are the `(UI|CreateAndFlipToDuelScene)` and `(UI|Flip To Game Side: DuelSceneViewModel)` blocks (lines 2063, 2120, 18289, 18344). Their `Stack:` contains `FlsTournamentRoundInfoMessage`, and `TOURNAMENT_MARKERS` in `crates/mtgo-protocol/src/classify.rs` matches marker text anywhere. Harmless today (they are skipped), but an overlay fold would read them. Anchor on `Handler for FlsTournamentRoundInfoMessage`.
3. **"Joined" in a state name does not mean the account joined.** Hundreds of `Match State Changed ... from MatchNotJoinedEventUnderwayState to MatchJoinedCompletedState`, then `MatchClosedState`, are other players' matches. Use `TournamentJoined*`/`PremierJoined*` tournament states, `JoinedToGame=True`, or the login id in `PlayerIds`.
4. **Duplicates.** Round 1 info is logged twice in the same second under two states. `EventSyncData_t` for the account's event appears 3 times at login, one handled again in `TournamentUninitializedState` after the joined state. Every tournament chat message is logged twice. A self transition `PremierJoinedAwaitingMaxPlayersState -> PremierJoinedAwaitingMaxPlayersState` occurs. A fold must be idempotent per round number.
5. **Times.** Log lines are local `HH:MM:SS` without a date. JSON dates (`StartDate`, match `StartDate`, `EndTime`) are client-local ISO strings with an offset (`+01:00`; October events show `+02:00`, so DST is applied per date). Use those to anchor. `SideboardingEnds` is the placeholder `1970-01-01T01:00:00+01:00`.
6. **Template versus actual match time.** The snapshot's `TournamentSyncData.MatchCreateInfo.MatchTimeInMinutes` is **25**, but real round matches have **50** (same as `RoundLengthInMinutes`). Read the round's match, not the template.
7. **Description length cap.** `PrizeStructure.Description` is cut at 255 characters mid-table. `FormatDescription` carries markup (`[a:url=...]Details[/a] - 8 Go to Showcase Qualifier`).
8. **Results keep departed players.** Round results list all 302 registrants (eliminated ones too, 0 points). `Rank` is unique with tiebreaks, so "139th" is among 151 players on 3 points.
9. **No mid-event snapshots.** `EventSyncData_t` for the account's event arrives only at login and registration (empty `Rounds`, no `Results` key). An app or MTGO restart mid-event gives a snapshot with full `Rounds[].Results` (as for the finished events here), so a restart can rebuild. Live, only round info, round result, elimination and state-change messages update it.
10. **The account's match does not close the round.** After the account's match ends, the tournament stays `TournamentJoinedRoundInProgressState` until every match finishes (32 minutes in R1). A "waiting" state must come from the match end, not the tournament state.
11. **Login ids versus names.** Results, eliminations, `ByeList` and `EliminatedPlayers` carry only `LoginID`. Names come from `Players[].PlayerName` (snapshot, round info) and `NewPlayerOrder[].ScreenName`, which are real player names (privacy). The match key-value block has `PlayerIds=900001,9000413` (ids only).
12. **Privacy.** Real names appear in snapshots, round info, `Twitch Info` status updates and match-end MetaMessages. Windows paths in the source logs contain a real user name (this trimmed fixture keeps none).
13. **Lobby noise.** About 450 `TournamentMatchUninitializedState -> TournamentMatchClosedState` lines at login and 212 underway transitions belong to other events. Link matches to the event through `Round.Matches[].EventToken` or `ParentToken`.

## 5. Other things useful for a stream card

- **Opponent name** per round: `Round.Matches[]` (`Players[].PlayerName`, `LoginID`, `ParentToken`, `Description: "Tournament:12858913 Round:2"`). The opponent's record and rank come from the previous round's result rows by login id ("vs PLAYER_B (1-0, 45th)").
- **Round timer**: the match `StartDate` + `MatchTimeInMinutes` gives a countdown (R2: 17:56:15.542 + 50 min).
- **Prize info**: `PrizeStructure.Name` (`TRK | Legacy Showcase Challenge | 2026 S3`), `Prizes[]` by `Position` (1 to 128, catalog ids) and the description text ("Top 8 earn a Token for... Showcase Qualifier", "X-2 finishers below 16th..."). Gives a "prize line" hint (top 128 here).
- **Registration counter** before the start: `NewPlayerOrder` length ("302 registered"), with `MinPlayers` 33 and `MaxPlayers` 672.
- **Byes**: `Round.ByeList` (login ids); a bye shows as `OpponentResults[].Bye: 1`.
- **Live life totals and hand counts**: the `(Twitch Info|Game Play Status Update ...)` JSON `Players[]` (`Life`, `HandCount`, `LibraryCount`), same as leagues.
- **Between-round chat**: `DosPlayerEventChatMessage` texts "round N has completed. The next round will begin in approximately 2 minutes.", "normal play has completed. The playoff rounds will begin in approximately 2 minutes." and "Tournament N has finished." (drafts). Usable stage triggers.

## MyMTGO app logs

The 3 files (2026-11-20 and 21 on 1.2.1; 2026-11-22 on 1.3.0, restarted 3 times, last at 15:36:57Z) are mostly `mymtgo::timing` DEBUG lines. Warnings:
- 426 `skipping card stats for a game ... "no player-side card stats"` (old matches re-shipped at each start).
- 4 `skipping a tournament event kind="tournament_round_info" ... "no tournament token"` (trap 2).
- 13 `deck_sync: skipping a grouping file that is not a readable deck`.

There are no errors and nothing about the overlay. The 22nd's file runs to 17:32:08Z, which is 18:32:08 local, the end of the MTGO log.

## Still needed (ask the tester)

- A log where the **account** makes or misses the top 8, drops, and sees the event end (the account's own `TournamentJoined*` playoff and end states, its own elimination message, `FlsTournamentEndRespMessage` for a Challenge).
- A **league match during a Challenge wait** (spec open question 2).
- Showcase Qualifier and Qualifier structure (open question 3).
