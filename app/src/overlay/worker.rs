//! The `overlay` thread (overlay spec 6 step 3): one thread that computes the
//! overlay views, pushes the changed ones to their window, and opens and
//! closes the overlay windows.
//!
//! It blocks on its channel (`recv_timeout` only while a timer is due: the
//! draft's grace expiry, the end of a finished league run's 5 minutes on the
//! card, the hosted overlay's 30 s heartbeat while publishing), drains every
//! queued message, then recomputes once.
//! Nothing polls. Messages: tracker events forwarded by the bridge, settings
//! saves, answers from the `overlay-net` thread.
//!
//! Window rules (spec 5.4, 0.x `SyncGameOverlayVisibility` and
//! `SyncDraftNotesWindowVisibility`): a window is opened or closed only when
//! its wanted state flips (`overlay::Memo`), at start-up (forced), and when
//! a settings save toggles that window's own setting (forced for that window
//! only; owner answer 11). A window the player closed therefore stays closed
//! until its next flip; a new live match counts as a flip, an unrelated
//! settings save does not. A failed window call is logged
//! and retried at the next wake; it never stops the worker.
//!
//! A match is live for the game overlay only once the tracker announced
//! frames of it in this session (`LiveEventsStored`, never sent while
//! catching up a backlog or rebuilding): a rebuild or a match left in
//! progress by an earlier session never opens an overlay.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use overlay::{
    Action, LeagueOverlayState, LeagueStatus, LiveMatch, Memo, draft_window_wanted,
    game_overlay_wanted, league_window_wanted,
};
use store::Connection;
use tracker::TrackerEvent;

use crate::limited::DraftNotesView;
use crate::overlay::events::{
    DraftNotesChanged, GameOverlayChanged, LeagueOverlayChanged, OverlayPublishChanged,
};
use crate::overlay::game::{
    GameInputs, GameMemo, GameOverlayView, KnownOpponent, LiveContext, game_view,
};
use crate::overlay::game_net::{GameAnswer, GameNet};
use crate::overlay::league::league_card;
use crate::overlay::publish::{HEARTBEAT_MS, NetRequest, PUBLISH_GAP_MS, PublishPort};
use crate::overlay::windows::{WindowEvent, WindowManager, WindowSpec};
use crate::overlay::{
    DRAFT_NOTES, GAME_OVERLAY, LEAGUE_OVERLAY, MAIN_WINDOW, draft_notes_spec, game_overlay_spec,
    league_overlay_spec, league_window_size,
};
use crate::settings::Settings;

/// Stores settings rows (through the tracker in the app).
pub type SaveRowsFn = Box<dyn FnMut(Vec<(String, String)>) -> Result<(), String> + Send>;

/// What the game overlay's commands read from the `overlay` thread: its
/// latest push (the page's first read) and the live match it was computed
/// for (where a pick or a note lands).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameSlot {
    pub latest: Option<GameOverlayChanged>,
    pub live: Option<LiveContext>,
    /// The archetype the game overlay named for that match, for the league
    /// card read off the `overlay` thread (the hosted publish).
    pub opponent: Option<KnownOpponent>,
}

/// The shared slot.
pub type SharedGameSlot = Arc<Mutex<GameSlot>>;

/// What wakes the `overlay` thread.
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayMsg {
    /// Frames (or other live events) of these matches were committed.
    LiveEventsStored {
        tokens: Vec<String>,
        detected_at: Instant,
    },
    /// A log match's view changed; `league` when it was played in an
    /// event the card can show (it names a league token or a tournament).
    MatchUpdated {
        live: Box<LiveMatch>,
        league: bool,
    },
    /// A match was discarded or removed from stats.
    MatchRemoved {
        token: String,
    },
    /// A match's projection failed (it needs attention).
    MatchFailed {
        token: String,
    },
    DecksChanged,
    LeaguesChanged,
    /// The tournament sync changed a stored standing (event overlay spec
    /// 7.1), after its commit.
    TournamentsChanged,
    LimitedChanged,
    /// Card rows changed (stubs filled in): the live read's candidates,
    /// which join them, are read again at the next full compute.
    CardsChanged,
    /// The league card's custom background was chosen or removed.
    BackgroundChanged,
    /// Settings were saved. A window whose own setting changed has its
    /// wanted state reapplied (forced).
    SettingsChanged {
        settings: Box<Settings>,
    },
    /// The tracker restarted: every live view it pushed is forgotten.
    Restarting,
    /// Offline mode or sign-in changed: the heartbeat may start or stop.
    GateChanged,
    /// A publish or clear finished on the `overlay-net` thread.
    PublishAnswered(OverlayPublishChanged),
    /// Archetype rows changed (a definition, a stub, a merge).
    ArchetypesChanged,
    /// These matches' archetype rows changed (a pick, detection).
    MatchArchetypesChanged {
        match_ids: Vec<i64>,
    },
    /// A sideboard guide or matchup note of this pair changed (Plan T's
    /// `GuidesChanged`, an app event the bridge never sees: ruling C6).
    GuidesChanged {
        deck_id: i64,
        archetype_id: i64,
    },
    /// The game overlay page measured itself: fit the window (0.x
    /// `FitGameOverlayWindow`).
    FitGame {
        fixed_height: u32,
        bar_height: u32,
    },
    /// Collapse or expand the game overlay (0.x `SetGameOverlayCollapsed`).
    CollapseGame {
        collapsed: bool,
        fixed_height: u32,
        bar_height: u32,
    },
    /// A game overlay lookup finished on the `overlay-net` thread.
    GameAnswered(GameAnswer),
    Stop,
}

/// A window change the game overlay page asked for, made at the next
/// update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameWindowOp {
    Fit {
        fixed: u32,
        bar: u32,
    },
    Collapse {
        collapsed: bool,
        fixed: u32,
        bar: u32,
    },
}

/// The overlay's message for a tracker event, if it has one (overlay spec 6
/// step 2). Manual matches and Arena matches never show an overlay (the
/// overlay reads MTGO only).
pub fn from_tracker(event: &TrackerEvent) -> Option<OverlayMsg> {
    match event {
        TrackerEvent::LiveEventsStored {
            tokens,
            detected_at,
        } => Some(OverlayMsg::LiveEventsStored {
            tokens: tokens.clone(),
            detected_at: *detected_at,
        }),
        TrackerEvent::MatchUpdated {
            view,
            manual: false,
            platform: store::Platform::Mtgo,
            ..
        } => Some(OverlayMsg::MatchUpdated {
            live: Box::new(LiveMatch {
                token: view.token.clone(),
                source: "log".to_string(),
                state: store::names::state_str(view.state).to_string(),
                game_count: view.games.len() as u32,
                format: view.format.clone(),
                hidden: false,
                needs_attention: false,
                started_at: view.started_at_ms,
            }),
            league: view.league_token.is_some() || view.tournament_event_id.is_some(),
        }),
        TrackerEvent::MatchRemoved { token } => Some(OverlayMsg::MatchRemoved {
            token: token.clone(),
        }),
        TrackerEvent::MatchFailed { token, .. } => Some(OverlayMsg::MatchFailed {
            token: token.clone(),
        }),
        TrackerEvent::Restarting { .. } => Some(OverlayMsg::Restarting),
        TrackerEvent::DecksChanged => Some(OverlayMsg::DecksChanged),
        TrackerEvent::ArchetypesChanged => Some(OverlayMsg::ArchetypesChanged),
        TrackerEvent::MatchArchetypesChanged { match_ids } => {
            Some(OverlayMsg::MatchArchetypesChanged {
                match_ids: match_ids.clone(),
            })
        }
        TrackerEvent::LeaguesChanged => Some(OverlayMsg::LeaguesChanged),
        TrackerEvent::TournamentsChanged => Some(OverlayMsg::TournamentsChanged),
        TrackerEvent::LimitedChanged => Some(OverlayMsg::LimitedChanged),
        TrackerEvent::CardsChanged => Some(OverlayMsg::CardsChanged),
        _ => None,
    }
}

/// The overlay worker's state between wakes.
pub struct Worker {
    settings: Settings,
    collapsed: bool,
    /// The height the game overlay had before it was collapsed.
    expanded_height: Option<u32>,
    /// The game view's lookups and their answers.
    game_net: GameNet,
    /// The match parts of the last game view, kept for frame-only wakes.
    game_parts: Option<GameMemo>,
    /// Something besides frames changed: the match parts are computed again.
    game_full: bool,
    game_window_ops: Vec<GameWindowOp>,
    /// Shared with the game overlay's commands.
    game_slot: Option<SharedGameSlot>,
    /// Stores the collapse rows (through the tracker in the app).
    save_rows: Option<SaveRowsFn>,
    /// Overlay windows can open in this session (spec 5.3).
    available: bool,
    /// Log matches in progress, by token.
    matches: BTreeMap<String, LiveMatch>,
    /// Matches the tracker announced live events of in this session.
    announced: BTreeSet<String>,
    game_memo: Memo,
    draft_memo: Memo,
    league_memo: Memo,
    /// The match the game overlay was last wanted for.
    game_token: Option<String>,
    /// The next reconcile reapplies every window's wanted state.
    force: Forced,
    game_dirty: bool,
    draft_dirty: bool,
    /// The earliest unpushed live event of this wake, for the budget.
    detected_at: Option<Instant>,
    game_revision: u32,
    last_game: Option<GameOverlayView>,
    last_draft: Option<Option<DraftNotesView>>,
    live_draft: bool,
    /// When the finished draft's grace ends (wall clock, ms).
    grace_ends_at_ms: Option<i64>,
    last_push_latency: Option<Duration>,
    /// `<app data>/overlay/`, where the card's custom background is.
    overlay_folder: Option<PathBuf>,
    league_dirty: bool,
    last_league: Option<LeagueOverlayState>,
    /// The live matches of the runs the card last read, by token.
    league_tokens: BTreeSet<String>,
    /// When the card next changes by itself (a finished run's grace ends).
    league_changes_at_ms: Option<i64>,
    /// The card size changed: an open league window is resized.
    resize_league: bool,
    /// The league card has the other event strip below it, so its window
    /// is taller by the strip (event overlay spec 8.4).
    league_strip: bool,
    /// The hosted overlay's side (`None`: nothing is ever published).
    publish: Option<Box<dyn PublishPort>>,
    /// A push to the hosted overlay is due (the card changed, publishing
    /// was switched on). Stays due while the publish gap holds it back.
    publish_due: bool,
    /// When the next heartbeat push is due; `None` while not publishing.
    heartbeat_at_ms: Option<i64>,
    /// When the last publish was queued; the publish gap starts here.
    last_publish_at_ms: Option<i64>,
    /// A publish answer for the settings card, pushed at the next update.
    publish_answer: Option<OverlayPublishChanged>,
    /// Game lookups answered since the last update.
    pending_answers: Vec<GameAnswer>,
}

impl Worker {
    /// A worker that reconciles every window at its first update.
    pub fn new(settings: Settings, collapsed: bool, available: bool) -> Worker {
        Worker {
            settings,
            collapsed,
            expanded_height: None,
            game_net: GameNet::default(),
            game_parts: None,
            game_full: true,
            game_window_ops: Vec::new(),
            game_slot: None,
            save_rows: None,
            available,
            matches: BTreeMap::new(),
            announced: BTreeSet::new(),
            game_memo: Memo::default(),
            draft_memo: Memo::default(),
            league_memo: Memo::default(),
            game_token: None,
            force: Forced::ALL,
            game_dirty: true,
            draft_dirty: true,
            detected_at: None,
            game_revision: 0,
            last_game: None,
            last_draft: None,
            live_draft: false,
            grace_ends_at_ms: None,
            last_push_latency: None,
            overlay_folder: None,
            league_dirty: true,
            last_league: None,
            league_tokens: BTreeSet::new(),
            league_changes_at_ms: None,
            resize_league: false,
            league_strip: false,
            publish: None,
            publish_due: false,
            heartbeat_at_ms: None,
            last_publish_at_ms: None,
            publish_answer: None,
            pending_answers: Vec::new(),
        }
    }

    /// Publish the league card to the hosted overlay through `port`.
    pub fn with_publish(mut self, port: Box<dyn PublishPort>) -> Worker {
        self.publish = Some(port);
        self
    }

    /// The game overlay's lookups.
    pub fn with_game_net(mut self, game_net: GameNet) -> Worker {
        self.game_net = game_net;
        self
    }

    #[cfg(test)]
    pub fn game_net(&self) -> &GameNet {
        &self.game_net
    }

    /// Share the game overlay's latest push and live match.
    pub fn with_game_slot(mut self, slot: SharedGameSlot) -> Worker {
        self.game_slot = Some(slot);
        self
    }

    /// Store settings rows through `save` (the collapse choice).
    pub fn with_save_rows(mut self, save: SaveRowsFn) -> Worker {
        self.save_rows = Some(save);
        self
    }

    /// The height to restore when the collapsed overlay expands.
    pub fn with_expanded_height(mut self, height: Option<u32>) -> Worker {
        self.expanded_height = height;
        self
    }

    /// Where the league card's custom background is kept.
    pub fn with_overlay_folder(mut self, folder: PathBuf) -> Worker {
        self.overlay_folder = Some(folder);
        self
    }

    /// Note what a message changes; `update` acts on it.
    pub fn handle(&mut self, conn: &Connection, message: OverlayMsg) {
        match message {
            OverlayMsg::LiveEventsStored {
                tokens,
                detected_at,
            } => {
                // A frame or state change of the card's live match (its
                // sideboarding, spec 6 step 3).
                if tokens
                    .iter()
                    .any(|token| self.league_tokens.contains(token))
                {
                    self.league_dirty = true;
                }
                self.announced.extend(tokens);
                self.detected_at = Some(match self.detected_at {
                    Some(earlier) => earlier.min(detected_at),
                    None => detected_at,
                });
                self.game_dirty = true;
            }
            OverlayMsg::MatchUpdated { live, league } => {
                self.game_full = true;
                self.league_dirty |= league || self.league_tokens.contains(&live.token);
                if live.state == "in_progress" {
                    // A failure is kept: 0.x's `failed_at` stays set.
                    let failed = self
                        .matches
                        .get(&live.token)
                        .is_some_and(|known| known.needs_attention);
                    let mut live = *live;
                    live.needs_attention = live.needs_attention || failed;
                    self.matches.insert(live.token.clone(), live);
                } else {
                    // Over: it can never be wanted again; memory stays bounded.
                    self.matches.remove(&live.token);
                    self.announced.remove(&live.token);
                }
                self.game_dirty = true;
            }
            OverlayMsg::MatchRemoved { token } => {
                self.game_full = true;
                // Removed from stats: never pushed again, never wanted again.
                self.matches.remove(&token);
                self.announced.remove(&token);
                self.game_dirty = true;
                self.league_dirty = true;
            }
            OverlayMsg::Restarting => {
                self.game_full = true;
                // As the bridge forgets its live views (the memos stay).
                self.matches.clear();
                self.announced.clear();
                self.game_dirty = true;
                self.league_dirty = true;
            }
            OverlayMsg::MatchFailed { token } => {
                self.game_full = true;
                if let Some(known) = self.matches.get_mut(&token) {
                    known.needs_attention = true;
                }
                self.game_dirty = true;
            }
            OverlayMsg::DecksChanged => {
                // A deck's name, colours, cover or label.
                self.game_dirty = true;
                self.game_full = true;
                self.league_dirty = true;
            }
            // A league match, game or run changed, or a tournament's
            // standing, after its commit. Many in one wake push once.
            OverlayMsg::LeaguesChanged | OverlayMsg::TournamentsChanged => self.league_dirty = true,
            OverlayMsg::BackgroundChanged => {
                self.league_dirty = true;
                // A removed background's uploaded copy is deleted (the job
                // keeps a background that is still chosen).
                self.submit(NetRequest::ForgetBackground);
            }
            OverlayMsg::LimitedChanged => self.draft_dirty = true,
            OverlayMsg::CardsChanged => self.game_net.forget_candidates(),
            OverlayMsg::SettingsChanged { settings } => {
                self.force.game |= settings.game_overlay != self.settings.game_overlay;
                self.force.draft |= settings.draft_notes_window != self.settings.draft_notes_window;
                self.force.league |= settings.league_window != self.settings.league_window;
                self.resize_league |= settings.overlay_size != self.settings.overlay_size
                    || settings.overlay_league_theme != self.settings.overlay_league_theme;
                if settings.overlay_publish != self.settings.overlay_publish {
                    if settings.overlay_publish {
                        // Switched on: pushed at once (0.x).
                        self.publish_due = true;
                    } else {
                        // Switched off: the hosted page is blanked. The
                        // last publish time is kept: switched back on
                        // inside the gap, the push waits for its end.
                        self.publish_due = false;
                        self.heartbeat_at_ms = None;
                        self.submit(NetRequest::Clear);
                    }
                }
                self.settings = *settings;
                self.game_full = true;
                match store::read::settings(conn) {
                    Ok(rows) => {
                        self.collapsed = crate::settings::overlay_collapsed(&rows);
                        self.expanded_height = crate::settings::overlay_expanded_height(&rows);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not read the overlay's settings rows")
                    }
                }
                self.game_dirty = true;
                self.draft_dirty = true;
                self.league_dirty = true;
            }
            // The next update starts or stops the heartbeat.
            OverlayMsg::GateChanged => {}
            OverlayMsg::PublishAnswered(answer) => self.publish_answer = Some(answer),
            OverlayMsg::ArchetypesChanged => {
                // An archetype's name or colours: the league card's title
                // and the game overlay's header. The live read's
                // candidates are read again.
                self.game_net.forget_candidates();
                self.game_dirty = true;
                self.game_full = true;
                self.league_dirty = true;
            }
            OverlayMsg::MatchArchetypesChanged { .. } => {
                // A pick: the league card names the opponent too.
                self.game_dirty = true;
                self.game_full = true;
                self.league_dirty = true;
            }
            OverlayMsg::GuidesChanged { .. } => {
                self.game_dirty = true;
                self.game_full = true;
            }
            OverlayMsg::FitGame {
                fixed_height,
                bar_height,
            } => self.game_window_ops.push(GameWindowOp::Fit {
                fixed: fixed_height,
                bar: bar_height,
            }),
            OverlayMsg::CollapseGame {
                collapsed,
                fixed_height,
                bar_height,
            } => self.game_window_ops.push(GameWindowOp::Collapse {
                collapsed,
                fixed: fixed_height,
                bar: bar_height,
            }),
            OverlayMsg::GameAnswered(answer) => {
                // A clock read here would differ from the update's only by
                // the drain; the answer is kept from the next update's now.
                self.pending_answers.push(answer);
                self.game_dirty = true;
                self.game_full = true;
                // A scouting or estimate answer can name the opponent.
                self.league_dirty = true;
            }
            OverlayMsg::Stop => {}
        }
    }

    /// Recompute what changed, push the changed views, then reconcile the
    /// windows.
    pub fn update(&mut self, conn: &Connection, windows: &mut dyn WindowManager, now_ms: i64) {
        if self.grace_ends_at_ms.is_some_and(|at| now_ms >= at) {
            self.draft_dirty = true;
        }
        for answer in std::mem::take(&mut self.pending_answers) {
            self.game_net.answered(answer, now_ms);
        }
        for op in std::mem::take(&mut self.game_window_ops) {
            self.game_window(windows, op);
        }
        // A held-back estimate's wait ended (#502): recompute in full, so
        // the match parts ask for it.
        if self.game_net.take_due_estimates(now_ms) {
            self.game_dirty = true;
            self.game_full = true;
        }
        let token = self.wanted_game_token();
        if std::mem::take(&mut self.game_dirty) {
            self.push_game(conn, windows, token.as_deref(), now_ms);
        }
        self.detected_at = None;
        if std::mem::take(&mut self.draft_dirty) {
            self.push_draft(conn, windows, now_ms);
        }
        if self.league_changes_at_ms.is_some_and(|at| now_ms >= at) {
            self.league_dirty = true;
        }
        if std::mem::take(&mut self.league_dirty) {
            self.push_league(conn, windows, now_ms);
        }
        if std::mem::take(&mut self.resize_league) {
            self.resize_league_window(windows);
        }
        self.publish_tick(windows, now_ms);
        self.reconcile(windows, token);
    }

    /// Push the settings card's publish answer, then queue a publish when
    /// one is due: the card changed, publishing was switched on, the gate
    /// opened, or the 30 s heartbeat. Only while publishing is on and the
    /// gate allows `Sync`; otherwise the heartbeat stops. Publishes are at
    /// least `PUBLISH_GAP_MS` apart: a due publish inside the gap waits for
    /// the gap's end (`next_wake`).
    fn publish_tick(&mut self, windows: &mut dyn WindowManager, now_ms: i64) {
        if let Some(answer) = self.publish_answer.take() {
            let push = WindowEvent::new(answer);
            if let Err(error) = windows.emit(MAIN_WINDOW, &push) {
                tracing::warn!(%error, "could not push the publish state");
            }
        }
        // The gate is asked only while publishing is on.
        let allowed = self.settings.overlay_publish
            && self.publish.as_ref().is_some_and(|port| port.allowed());
        if !allowed {
            self.publish_due = false;
            self.heartbeat_at_ms = None;
            return;
        }
        // Not yet beating (start-up, switched on, the gate just opened) or
        // the heartbeat is due. It beats only while the card shows a run
        // (#505): the hosted page shows nothing for an idle card, as for
        // none, so an idle card is published once (its change) and not kept
        // alive.
        let beat = match self.heartbeat_at_ms {
            None => true,
            Some(at) => self.card_live() && now_ms >= at,
        };
        if !self.publish_due && !beat {
            return;
        }
        // Inside the gap: stay due; `next_wake` wakes at the gap's end.
        if self.gap_ends_at().is_some_and(|ends| now_ms < ends) {
            self.publish_due = true;
            return;
        }
        self.publish_due = false;
        self.submit(NetRequest::Publish);
        self.heartbeat_at_ms = Some(now_ms + HEARTBEAT_MS);
        self.last_publish_at_ms = Some(now_ms);
    }

    /// The league card shows a run (anything but idle).
    fn card_live(&self) -> bool {
        self.last_league
            .as_ref()
            .is_some_and(|card| card.status != LeagueStatus::Idle)
    }

    /// When the publish gap after the last publish ends.
    fn gap_ends_at(&self) -> Option<i64> {
        self.last_publish_at_ms.map(|at| at + PUBLISH_GAP_MS)
    }

    /// Queue a request on the `overlay-net` thread, if publishing exists.
    fn submit(&mut self, request: NetRequest) {
        if let Some(port) = self.publish.as_mut() {
            port.submit(request);
        }
    }

    /// How long until a timer is due; `None` when none is.
    pub fn next_wake(&self, now_ms: i64) -> Option<Duration> {
        let at = [
            self.grace_ends_at_ms,
            self.league_changes_at_ms,
            self.heartbeat_at_ms.filter(|_| self.card_live()),
            self.game_net.estimate_wake_at(),
            self.publish_due.then(|| self.gap_ends_at()).flatten(),
        ]
        .into_iter()
        .flatten()
        .min()?;
        Some(Duration::from_millis(
            at.saturating_sub(now_ms).max(0) as u64
        ))
    }

    /// The league card's latest push.
    pub fn league_state(&self) -> Option<&LeagueOverlayState> {
        self.last_league.as_ref()
    }

    /// The game overlay's latest push (for its page's first read).
    pub fn game_state(&self) -> Option<GameOverlayChanged> {
        Some(GameOverlayChanged {
            revision: self.game_revision,
            view: self.last_game.clone()?,
        })
    }

    /// The match the game overlay is wanted for: a live, announced log
    /// match (spec 5.4), the newest of two.
    fn wanted_game_token(&self) -> Option<String> {
        let live: Vec<LiveMatch> = self
            .matches
            .values()
            .filter(|live| self.announced.contains(&live.token))
            .cloned()
            .collect();
        game_overlay_wanted(self.settings.game_overlay, &live, self.available)
    }

    /// Compute the game view and push it when it changed. Only frames were
    /// stored since the last compute: the match parts are kept unless the
    /// reveals changed.
    fn push_game(
        &mut self,
        conn: &Connection,
        windows: &mut dyn WindowManager,
        token: Option<&str>,
        now_ms: i64,
    ) {
        let full = std::mem::take(&mut self.game_full);
        let previous = if full { None } else { self.game_parts.clone() };
        let inputs = GameInputs {
            settings: &self.settings,
            collapsed: self.collapsed,
            now_ms,
            previous: previous.as_ref(),
        };
        self.game_net.prune(now_ms);
        let (view, memo) = match game_view(conn, token, &inputs, &mut self.game_net) {
            Ok(computed) => computed,
            Err(error) => {
                tracing::warn!(%error, "could not read the game overlay's match");
                // Tried in full at the next wake.
                self.game_full = true;
                return;
            }
        };
        self.game_parts = memo;
        let live = self.game_parts.as_ref().map(|memo| memo.live.clone());
        if self.last_game.as_ref() == Some(&view) {
            self.share_game(live);
            return;
        }
        self.game_revision += 1;
        let push = WindowEvent::new(GameOverlayChanged {
            revision: self.game_revision,
            view: view.clone(),
        });
        if let Err(error) = windows.emit(GAME_OVERLAY, &push) {
            tracing::warn!(%error, "could not push the game overlay");
        }
        self.last_game = Some(view);
        self.share_game(live);
        if let Some(detected_at) = self.detected_at {
            let latency = detected_at.elapsed();
            self.last_push_latency = Some(latency);
            tracing::debug!(
                target: "mymtgo::timing",
                push_us = latency.as_micros() as u64,
                revision = self.game_revision,
                "overlay_push"
            );
        }
    }

    /// Give the commands the latest push and live match.
    fn share_game(&mut self, live: Option<LiveContext>) {
        let Some(slot) = self.game_slot.as_ref() else {
            return;
        };
        let latest = self.game_state();
        let opponent = self.game_parts.as_ref().and_then(GameMemo::known_opponent);
        let mut shared = crate::bridge::lock(slot);
        shared.latest = latest;
        shared.live = live;
        shared.opponent = opponent;
    }

    /// Fit, collapse or expand the game overlay (0.x
    /// `FitGameOverlayWindow`, `SetGameOverlayCollapsed`). The collapse
    /// choice is stored even when the window is not open.
    fn game_window(&mut self, windows: &mut dyn WindowManager, op: GameWindowOp) {
        let open = windows.is_open(GAME_OVERLAY);
        let size = if open {
            windows.size(GAME_OVERLAY)
        } else {
            None
        };
        let width = size.map_or(f64::from(overlay::height::DEFAULT_WIDTH), |(width, _)| {
            width
        });
        let sections = crate::overlay::game::sections(&self.settings);
        let height = match op {
            GameWindowOp::Fit { fixed, bar } => {
                if !open {
                    return;
                }
                let current = size.map(|(_, height)| height.round() as u32);
                let fitted = self.fit_height(fixed, bar, current);
                if current == Some(fitted) {
                    return;
                }
                fitted
            }
            GameWindowOp::Collapse {
                collapsed,
                fixed,
                bar,
            } => {
                if collapsed
                    && !self.collapsed
                    && let Some((_, height)) = size
                    && height > 0.0
                {
                    // A repeated collapse keeps the first height.
                    self.expanded_height = Some(height.round() as u32);
                }
                self.collapsed = collapsed;
                self.game_dirty = true;
                let height = if collapsed {
                    overlay::height::collapsed(fixed, bar)
                } else {
                    self.expanded_height
                        .take()
                        .unwrap_or_else(|| overlay::height::from_settings(&sections, Some(fixed)))
                };
                self.save_collapse_rows();
                if !open {
                    return;
                }
                height
            }
        };
        if let Err(error) = windows.resize(GAME_OVERLAY, width, f64::from(height)) {
            tracing::warn!(%error, "could not resize the game overlay");
        }
    }

    /// The fitted height: the strip when collapsed, else the measured fixed
    /// content, plus the tab area when a tab section is on (the window's
    /// `current` height kept while it has room, `overlay::height::refit`).
    fn fit_height(&self, fixed: u32, bar: u32, current: Option<u32>) -> u32 {
        if self.collapsed {
            overlay::height::collapsed(fixed, bar)
        } else {
            let sections = crate::overlay::game::sections(&self.settings);
            let has_tabs = overlay::height::has_tab_sections(&sections);
            overlay::height::refit(has_tabs, fixed, current)
        }
    }

    fn save_collapse_rows(&mut self) {
        let rows = vec![
            crate::settings::overlay_collapsed_row(self.collapsed),
            crate::settings::overlay_expanded_height_row(self.expanded_height),
        ];
        if let Some(save) = self.save_rows.as_mut()
            && let Err(error) = save(rows)
        {
            tracing::warn!(%error, "could not store the game overlay's collapse");
        }
    }

    /// The collapse choice and the remembered height, as held now.
    pub fn collapse_state(&self) -> (bool, Option<u32>) {
        (self.collapsed, self.expanded_height)
    }

    /// Compute the draft notes and push them when they changed.
    fn push_draft(&mut self, conn: &Connection, windows: &mut dyn WindowManager, now_ms: i64) {
        let live = match crate::limited::live_notes(conn, now_ms) {
            Ok(live) => live,
            Err(error) => {
                tracing::warn!(?error, "could not read the live draft");
                // Retried at the next message, never in a loop on a due timer.
                self.grace_ends_at_ms = None;
                return;
            }
        };
        self.grace_ends_at_ms = live.as_ref().and_then(|live| live.grace_ends_at_ms);
        let view = live.map(|live| live.view);
        self.live_draft = view.is_some();
        if self.last_draft.as_ref() == Some(&view) {
            return;
        }
        let push = WindowEvent::new(DraftNotesChanged { view: view.clone() });
        if let Err(error) = windows.emit(DRAFT_NOTES, &push) {
            tracing::warn!(%error, "could not push the draft notes");
        }
        self.last_draft = Some(view);
    }

    /// Compute the league card and push it when it changed, to the league
    /// window and the main window (the settings preview). It is computed
    /// even when overlay windows are unavailable: the main window and the
    /// hosted overlay still show it.
    fn push_league(&mut self, conn: &Connection, windows: &mut dyn WindowManager, now_ms: i64) {
        let known = self.game_parts.as_ref().and_then(GameMemo::known_opponent);
        let card = match league_card(
            conn,
            &self.settings,
            self.overlay_folder.as_deref(),
            now_ms,
            known.as_ref(),
            &mut self.game_net,
        ) {
            Ok(card) => card,
            Err(error) => {
                tracing::warn!(%error, "could not read the league card");
                // Retried at the next message, never in a loop on a due timer.
                self.league_changes_at_ms = None;
                return;
            }
        };
        self.league_tokens = card.live_tokens;
        self.league_changes_at_ms = card.changes_at_ms;
        self.show_league_card(windows, card.state);
    }

    /// Push a changed league card to the league window and the main window
    /// (the settings preview), resizing the league window when the other
    /// event strip comes or goes (spec 8.4): grown before the card that
    /// gains the strip and shrunk after the card that loses it, so the strip
    /// is never clipped while the page catches up.
    fn show_league_card(&mut self, windows: &mut dyn WindowManager, state: LeagueOverlayState) {
        let strip = crate::overlay::league::strip_showing(&state);
        let grows = strip && !self.league_strip;
        let shrinks = !strip && self.league_strip;
        self.league_strip = strip;
        if grows {
            self.resize_league_window(windows);
        }
        if self.last_league.as_ref() != Some(&state) {
            let push = WindowEvent::new(LeagueOverlayChanged {
                state: state.clone(),
            });
            for label in [LEAGUE_OVERLAY, MAIN_WINDOW] {
                if let Err(error) = windows.emit(label, &push) {
                    tracing::warn!(%error, label, "could not push the league card");
                }
            }
            self.last_league = Some(state);
            // The hosted overlay follows every changed card (spec 6 step 3).
            self.publish_due = true;
        }
        if shrinks {
            self.resize_league_window(windows);
        }
    }

    /// Resize an open league window to the card size (0.x "resizes an open
    /// league overlay when the size changes"), the strip included while it
    /// shows. A closed one opens at the new size anyway. Its top left stays
    /// put: a size change never moves the window.
    fn resize_league_window(&mut self, windows: &mut dyn WindowManager) {
        if !windows.is_open(LEAGUE_OVERLAY) {
            return;
        }
        let (width, height) = league_window_size(
            self.settings.overlay_league_theme,
            self.settings.overlay_size,
            self.league_strip,
        );
        if let Err(error) = windows.resize(LEAGUE_OVERLAY, width, height) {
            tracing::warn!(%error, "could not resize the league window");
        }
    }

    /// Open or close each window whose wanted state flipped (every one when
    /// forced). A new live match counts as a flip of the game overlay.
    fn reconcile(&mut self, windows: &mut dyn WindowManager, token: Option<String>) {
        let force = std::mem::take(&mut self.force);
        let game_force = force.game;
        let new_match = token.is_some() && token != self.game_token;
        let game_wanted = token.is_some();
        self.game_token = token;
        reconcile_window(
            windows,
            &mut self.game_memo,
            game_wanted,
            game_force || new_match,
            &game_overlay_spec(&self.settings),
        );
        let draft_wanted = draft_window_wanted(
            self.settings.draft_notes_window,
            self.live_draft,
            self.available,
        );
        reconcile_window(
            windows,
            &mut self.draft_memo,
            draft_wanted,
            force.draft,
            &draft_notes_spec(),
        );
        let league_wanted = league_window_wanted(self.settings.league_window, self.available);
        reconcile_window(
            windows,
            &mut self.league_memo,
            league_wanted,
            force.league,
            &league_overlay_spec(
                self.settings.overlay_league_theme,
                self.settings.overlay_size,
                self.league_strip,
            ),
        );
    }

    /// Detection to emit of the latest frame-driven push.
    pub fn last_push_latency(&self) -> Option<Duration> {
        self.last_push_latency
    }
}

/// Which windows the next reconcile reapplies whatever their memo says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Forced {
    game: bool,
    draft: bool,
    league: bool,
}

impl Forced {
    /// Every window (start-up).
    const ALL: Forced = Forced {
        game: true,
        draft: true,
        league: true,
    };
}

/// The thread's loop: update once (start-up, forced), then wake on messages
/// or a due timer until `Stop` or every sender is gone. `clock` is now in
/// UTC ms (the wall clock; tests pass their own).
pub fn run_overlay_worker(
    inbox: &Receiver<OverlayMsg>,
    worker: &mut Worker,
    conn: &Connection,
    windows: &mut dyn WindowManager,
    clock: &dyn Fn() -> i64,
) {
    worker.update(conn, windows, clock());
    loop {
        let mut messages = Vec::new();
        match worker.next_wake(clock()) {
            Some(wait) => match inbox.recv_timeout(wait) {
                Ok(message) => messages.push(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match inbox.recv() {
                Ok(message) => messages.push(message),
                Err(_) => return,
            },
        }
        // Everything already waiting joins this wake: a burst recomputes once.
        while let Ok(message) = inbox.try_recv() {
            messages.push(message);
        }
        for message in messages {
            if message == OverlayMsg::Stop {
                return;
            }
            worker.handle(conn, message);
        }
        worker.update(conn, windows, clock());
    }
}

/// Apply one window's wanted state through its memo.
fn reconcile_window(
    windows: &mut dyn WindowManager,
    memo: &mut Memo,
    wanted: bool,
    force: bool,
    spec: &WindowSpec,
) {
    let Some(action) = memo.decide(wanted, force) else {
        return;
    };
    let done = match action {
        Action::Open => windows.open(spec),
        Action::Close => windows.close(&spec.label),
    };
    if let Err(error) = done {
        // Not remembered as done: the next wake tries again.
        tracing::warn!(%error, label = %spec.label, "overlay window sync failed");
        memo.failed();
    }
}

#[cfg(test)]
mod tests;
