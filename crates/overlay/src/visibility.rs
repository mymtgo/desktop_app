//! Which overlay windows are wanted (overlay spec 5.4) and the flip memo
//! (0.x `SyncGameOverlayVisibility`, `SyncDraftNotesWindowVisibility`).
//!
//! Every rule takes `available` (from `platform::overlay_windows_available`):
//! when overlay windows are unavailable, none of the three is ever wanted.

use leagues::is_limited_format_code;

/// A match as the game overlay sees it (a `matches` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveMatch {
    pub token: String,
    /// `matches.source`: only `log` matches are live.
    pub source: String,
    /// `matches.state`.
    pub state: String,
    /// The match's game count (0 until game 1's first event).
    pub game_count: u32,
    /// `matches.format`, `None` when unknown.
    pub format: Option<String>,
    /// `matches.hidden_at` is set.
    pub hidden: bool,
    /// `matches.needs_attention` (0.x `failed_at`).
    pub needs_attention: bool,
    /// `matches.started_at`, UTC unix milliseconds.
    pub started_at: i64,
}

impl LiveMatch {
    /// A constructed log match being played: not hidden, not needing
    /// attention, `in_progress` with at least one game (0.x `InProgress`; a
    /// join without a game is 0.x `Started`), format not limited (an unknown
    /// format counts as constructed).
    fn shows_overlay(&self) -> bool {
        let limited = self.format.as_deref().is_some_and(is_limited_format_code);
        self.source == "log"
            && !self.hidden
            && !self.needs_attention
            && self.state == "in_progress"
            && self.game_count > 0
            && !limited
    }
}

/// The token of the match the game overlay shows, `None` when it is not
/// wanted. With two live matches (two clients) the newest `started_at` wins;
/// on a tie the first one given.
pub fn game_overlay_wanted(setting: bool, live: &[LiveMatch], available: bool) -> Option<String> {
    if !available || !setting {
        return None;
    }
    let mut newest: Option<&LiveMatch> = None;
    for candidate in live.iter().filter(|candidate| candidate.shows_overlay()) {
        let newer = match newest {
            Some(current) => candidate.started_at > current.started_at,
            None => true,
        };
        if newer {
            newest = Some(candidate);
        }
    }
    newest.map(|found| found.token.clone())
}

/// Whether the draft notes window is wanted: the setting is on and Plan N's
/// live draft exists (connecting or picking, else finished within 30 s).
pub fn draft_window_wanted(setting: bool, live_draft: bool, available: bool) -> bool {
    available && setting && live_draft
}

/// Whether the league window is wanted: whenever its setting is on, idle
/// card included (ruling 11).
pub fn league_window_wanted(setting: bool, available: bool) -> bool {
    available && setting
}

/// A window call to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Close,
}

/// The flip memo of one window: the wanted state last applied, so a window
/// is opened or closed only when its wanted state flips (a window the player
/// closed stays closed until then). `None` until something is applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Memo {
    pub last: Option<bool>,
}

impl Memo {
    /// The call to make for a wanted state, `None` when it was already
    /// applied. `force` (start-up, a settings toggle) applies it again. The
    /// state counts as applied once a call is returned; call `failed` when the
    /// window call fails so the next wake retries.
    pub fn decide(&mut self, wanted: bool, force: bool) -> Option<Action> {
        if !force && self.last == Some(wanted) {
            return None;
        }
        self.last = Some(wanted);
        if wanted {
            Some(Action::Open)
        } else {
            Some(Action::Close)
        }
    }

    /// The last window call failed: forget it so the next decision retries.
    pub fn failed(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(token: &str, started_at: i64) -> LiveMatch {
        LiveMatch {
            token: token.to_string(),
            source: "log".to_string(),
            state: "in_progress".to_string(),
            game_count: 1,
            format: Some("CMODERN".to_string()),
            hidden: false,
            needs_attention: false,
            started_at,
        }
    }

    fn wanted(setting: bool, live: &[LiveMatch]) -> Option<String> {
        game_overlay_wanted(setting, live, true)
    }

    #[test]
    fn opens_the_overlay_when_a_match_is_in_progress_and_the_setting_is_enabled() {
        assert_eq!(wanted(true, &[live("t1", 10)]), Some("t1".to_string()));
    }

    #[test]
    fn does_not_open_the_overlay_when_the_setting_is_disabled() {
        assert_eq!(wanted(false, &[live("t1", 10)]), None);
    }

    #[test]
    fn does_not_open_the_overlay_for_a_match_that_is_only_started() {
        let mut joined = live("t1", 10);
        joined.game_count = 0;
        let mut pending = live("t2", 10);
        pending.state = "pending".to_string();

        assert_eq!(wanted(true, &[joined, pending]), None);
    }

    #[test]
    fn does_not_open_the_overlay_for_a_failed_in_progress_match() {
        let mut failed = live("t1", 10);
        failed.needs_attention = true;

        assert_eq!(wanted(true, &[failed]), None);
    }

    #[test]
    fn closes_an_open_overlay_when_no_match_is_in_progress() {
        let mut ended = live("t1", 10);
        ended.state = "ended".to_string();

        assert_eq!(wanted(true, &[ended]), None);
        assert_eq!(wanted(true, &[]), None);
    }

    #[test]
    fn does_not_open_the_overlay_for_a_limited_format_match_in_progress() {
        let mut draft = live("t1", 10);
        draft.format = Some("DHOBHOBHOB".to_string());
        let mut sealed = live("t2", 10);
        sealed.format = Some("S1".to_string());

        assert_eq!(wanted(true, &[draft, sealed]), None);
    }

    #[test]
    fn still_opens_the_overlay_when_the_match_format_is_unknown() {
        let mut unknown = live("t1", 10);
        unknown.format = Some("Unknown".to_string());
        let mut missing = live("t2", 5);
        missing.format = None;

        assert_eq!(wanted(true, &[unknown]), Some("t1".to_string()));
        assert_eq!(wanted(true, &[missing]), Some("t2".to_string()));
    }

    #[test]
    fn a_hidden_or_non_log_match_never_opens_the_overlay() {
        let mut hidden = live("t1", 10);
        hidden.hidden = true;
        let mut imported = live("t2", 10);
        imported.source = "imported".to_string();

        assert_eq!(wanted(true, &[hidden, imported]), None);
    }

    #[test]
    fn the_newest_live_match_wins_with_two_clients() {
        let mut limited_newer = live("limited", 40);
        limited_newer.format = Some("DMKMMKM".to_string());
        let matches = [live("older", 10), live("newer", 30), limited_newer];

        assert_eq!(wanted(true, &matches), Some("newer".to_string()));
    }

    #[test]
    fn opens_the_draft_window_while_a_draft_is_live_and_the_setting_is_on() {
        assert!(draft_window_wanted(true, true, true));
        assert!(!draft_window_wanted(false, true, true));
        assert!(!draft_window_wanted(true, false, true));
    }

    #[test]
    fn the_league_window_follows_its_setting() {
        assert!(league_window_wanted(true, true));
        assert!(!league_window_wanted(false, true));
    }

    #[test]
    fn nothing_is_wanted_when_overlays_are_unavailable() {
        assert_eq!(game_overlay_wanted(true, &[live("t1", 10)], false), None);
        assert!(!draft_window_wanted(true, true, false));
        assert!(!league_window_wanted(true, false));
    }

    #[test]
    fn does_not_touch_the_window_api_twice_for_an_unchanged_desired_state() {
        let mut memo = Memo::default();

        assert_eq!(memo.decide(true, false), Some(Action::Open));
        assert_eq!(memo.decide(true, false), None);
    }

    #[test]
    fn does_not_touch_the_window_api_twice_while_nothing_is_live() {
        let mut memo = Memo::default();

        assert_eq!(memo.decide(false, false), Some(Action::Close));
        assert_eq!(memo.decide(false, false), None);
    }

    #[test]
    fn reopens_the_window_when_the_desired_state_flips_back() {
        let mut memo = Memo::default();

        let actions = [
            memo.decide(true, false),
            memo.decide(false, false),
            memo.decide(true, false),
        ];

        assert_eq!(
            actions,
            [Some(Action::Open), Some(Action::Close), Some(Action::Open)]
        );
    }

    #[test]
    fn closes_an_already_synced_window_when_the_setting_flips_off_and_the_sync_is_forced() {
        let mut memo = Memo::default();
        memo.decide(true, false);

        assert_eq!(memo.decide(false, true), Some(Action::Close));
    }

    #[test]
    fn reapplies_the_same_desired_state_when_forced() {
        let mut memo = Memo::default();
        memo.decide(false, false);

        assert_eq!(memo.decide(false, true), Some(Action::Close));
    }

    #[test]
    fn retries_on_the_next_tick_after_a_window_api_failure() {
        let mut memo = Memo::default();

        assert_eq!(memo.decide(true, false), Some(Action::Open));
        memo.failed();

        assert_eq!(memo.decide(true, false), Some(Action::Open));
        assert_eq!(memo.decide(true, false), None);
    }
}
