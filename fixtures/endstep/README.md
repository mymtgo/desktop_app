# Endstep fixtures

## ranked-modern-bo3

One real ranked Modern Bo3 on endstep.cc against a human opponent, captured by the MyMTGO browser extension. The user lost 1-2; all three games ended by concession. Endstep moved the match to another gateway during game 1, so the game socket reconnected and the match is split across two match units.

The files are the capture units exactly as MyMTGO stored them (decompressed), in the extension's own capture format version 1: line 1 is the unit header (`mymtgo_endstep_capture: 1`, `unit_id`, `kind`, `started_at`, `counter`), every other line is one captured frame or REST body. They are numbered by `(started_at, counter)`:

1. `01-session-a7a2056a.jsonl`: session unit (site load, queue, lobby, decks, the match REST body).
2. `02-match-329f3d80.jsonl`: match unit, game 1 up to the gateway move.
3. `03-session-e3f486f1.jsonl`: session unit (after the move, with the match REST body again).
4. `04-match-946164b1.jsonl`: match unit, games 1 to 3 after the reconnect, with the ranked result REST body.
5. `05-session-1a930f54.jsonl`: session unit (after the match, header and one frame).

Privacy: every person name was replaced. The user is `Player`, the opponent is `Opponent`, and every other person (live match lists, queue rows, tournament organisers) is `Person1`, `Person2` and so on. Endstep user ids were replaced with fake uuids (`00000000-0000-4000-8000-00000000000a` for the user, `...0b` for the opponent, `...0c` onwards for everyone else), and tournament and series names with `Tournament N` and `Series N`. Em dashes in card text are written as the JSON escape `\u2014`. Match, game, lobby, deck, pairing, result, replay, row, action, tournament and other server ids were replaced with fake uuids of the same length (`00000000-0000-4000-8000-e00000000001` onwards, one fake per real id, used the same way in the tests and snapshots). Unit ids (extension-generated, also in the file names) and card and illustration ids (public catalog data) are unchanged.

Frames from `/api/matches/live` (a site-wide list of other players' matches) were removed; the extension no longer captures them.

### What it shows for match detail and replays

Lines are `unit:line` (02 and 04 are the match units). Plan 4's tests and snapshots (`crates/endstep-match/tests/fold.rs`, `tests/mtgr.rs` and their `snapshots/`) pin these facts.

- **Hands.** Game 1: the game socket attached after the deal, so its first state (02:5) already holds the user's seven cards. The opponent chose to mulligan (02:7), redrew to 6 (02:23) and bottomed one hidden card; the user kept 7 (02:28). Game 2: both kept 7. Game 3: the user chose to mulligan (04:696) and sent back Trinisphere, Sowing Mycospawn, Expedition Map, Ugin, Eye of the Storms, Talisman of Resilience, Forest and Karn, the Great Creator (04:693); redrew Ugin, Eldrazi Temple twice, Karn, Disruptor Flute, Sowing Mycospawn and Kozilek's Command (04:713); kept (04:721) and bottomed the Karn (id 20, 04:729), so the first state after (04:740) holds 6 cards. `HAND_SETTLED` comes once per seat per game (02:37, 02:38, 04:294, 04:295, 04:733, 04:734).
- **Sideboarding.** Only the user's. Games 2 and 3 open with a sideboarding state whose prompt lists 75 options, the first 60 the deck of the game before. The user's `SIDEBOARD_SUBMIT` goes out on `lobby_ws` (04:250 for game 2, 04:669 for game 3): game 2 takes two Giant's Boulder out and puts Chalice of the Void and Disruptor Flute in; game 3 resubmits the same deck.
- **Reveals.** Nine `CARD_REVEALED`, every one with `playerIndex` `"0"` (the viewer): 02:160 (Thundering Falls in the opponent's hand), 02:275 (Urza's Tower, the user's), 02:368 (the cascade's look at 21 cards of the opponent's library), 04:292 (Devourer of Destiny from the user's opening hand), 04:426 (Temple Garden), 04:501 (Stomping Ground), 04:562 (Devourer of Destiny in the user's exile), 04:616 (Violent Outburst and Living End) and 04:869 (Thundering Falls).
- **Two casts taken back.** Of the 23 named moves onto the stack, 21 are followed by a `SPELL_CAST`. The user's Talisman of Resilience in game 2 goes onto the stack (04:572) and back to hand after the user's `CANCEL` (04:579). The opponent's Force of Negation in game 3 goes onto the stack (04:923) and back to hand with no `SPELL_CAST`; no state ever lists that card, so it is known only from the event.
