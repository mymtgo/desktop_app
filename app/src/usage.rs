//! Anonymous usage counts through Aptabase (usage analytics spec,
//! `docs/superpowers/specs/2026-11-25-usage-analytics-design.md`).
//!
//! Only the events in `UsageEvent` and the period markers of
//! `usage_marks` are ever sent, with fixed names and no player data. Nothing is sent unless a key was built in
//! (`APTABASE_APP_KEY` at compile time), "Share anonymous usage data" is on
//! and offline mode is off. The plugin queues events in memory and sends
//! them in batches from Rust.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tauri_plugin_aptabase::EventTracker;

use crate::api::session::ApiState;
use crate::commands::CommandError;

/// The Aptabase app key, read when the app is compiled. Local and fork
/// builds without it register no plugin and send nothing.
pub const APP_KEY: Option<&str> = option_env!("APTABASE_APP_KEY");

/// The longest route name `page_viewed` sends.
const MAX_PAGE_LEN: usize = 40;

/// The built-in key, when there is one.
pub fn app_key() -> Option<&'static str> {
    APP_KEY.map(str::trim).filter(|key| !key.is_empty())
}

/// A usage event: the only things the app ever counts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UsageEvent {
    /// A route change; `page` is the route's name.
    PageViewed { page: String },
    /// A feature was used.
    FeatureUsed { feature: Feature },
    /// A Wrapped card was copied as an image; `card` is its kind only.
    WrappedCardCopied { card: WrappedCardKind },
}

/// The features counted by `FeatureUsed`, one name each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Feature {
    /// "Copy as image" anywhere in the app.
    CopyImage,
}

impl Feature {
    fn name(self) -> &'static str {
        match self {
            Feature::CopyImage => "copy_image",
        }
    }
}

/// The kinds of Wrapped card, as the UI names them (`WrappedCard`'s `kind`
/// tag). A closed list, so nothing but a card kind is ever sent. The UI
/// passes `card.kind` straight in, so a new card kind that is missing here
/// fails the type check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum WrappedCardKind {
    MagicYear,
    Banned,
    YearInNumbers,
    Formats,
    WhenYouPlay,
    DeckOfYear,
    BestDeck,
    MvpCard,
    OpeningHand,
    SideboardHero,
    MostFaced,
    BestMatchup,
    ToughestRival,
    Comebacks,
    PlayDraw,
    Trophies,
    Limited,
    PlayerType,
    Wrap,
}

impl WrappedCardKind {
    fn name(self) -> &'static str {
        match self {
            WrappedCardKind::MagicYear => "magic_year",
            WrappedCardKind::Banned => "banned",
            WrappedCardKind::YearInNumbers => "year_in_numbers",
            WrappedCardKind::Formats => "formats",
            WrappedCardKind::WhenYouPlay => "when_you_play",
            WrappedCardKind::DeckOfYear => "deck_of_year",
            WrappedCardKind::BestDeck => "best_deck",
            WrappedCardKind::MvpCard => "mvp_card",
            WrappedCardKind::OpeningHand => "opening_hand",
            WrappedCardKind::SideboardHero => "sideboard_hero",
            WrappedCardKind::MostFaced => "most_faced",
            WrappedCardKind::BestMatchup => "best_matchup",
            WrappedCardKind::ToughestRival => "toughest_rival",
            WrappedCardKind::Comebacks => "comebacks",
            WrappedCardKind::PlayDraw => "play_draw",
            WrappedCardKind::Trophies => "trophies",
            WrappedCardKind::Limited => "limited",
            WrappedCardKind::PlayerType => "player_type",
            WrappedCardKind::Wrap => "wrap",
        }
    }
}

/// Whether an event may be sent: a key was built in, the player shares
/// usage data and offline mode is off.
pub fn sends(has_key: bool, share_usage: bool, offline: bool) -> bool {
    has_key && share_usage && !offline
}

/// A route name as sent: lowercase letters, digits, `-` and `_`, at most
/// `MAX_PAGE_LEN` long. Anything else is not a route name and is dropped.
pub fn clean_page(page: &str) -> Option<String> {
    let page = page.trim();
    if page.is_empty() || page.len() > MAX_PAGE_LEN {
        return None;
    }
    let allowed = page
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if allowed {
        Some(page.to_string())
    } else {
        None
    }
}

/// The Aptabase event name and props, or `None` for an event that fails
/// its checks.
pub fn event_payload(event: &UsageEvent) -> Option<(&'static str, Option<Value>)> {
    match event {
        UsageEvent::PageViewed { page } => {
            let page = clean_page(page)?;
            Some(("page_viewed", Some(json!({ "page": page }))))
        }
        UsageEvent::FeatureUsed { feature } => {
            Some(("feature_used", Some(json!({ "feature": feature.name() }))))
        }
        UsageEvent::WrappedCardCopied { card } => {
            Some(("wrapped_card_copied", Some(json!({ "card": card.name() }))))
        }
    }
}

/// The "Share anonymous usage data" switch as the app runs: set from the
/// stored settings at startup and on every settings save.
pub struct UsageGate {
    share: AtomicBool,
}

impl UsageGate {
    pub fn new(share: bool) -> Self {
        Self {
            share: AtomicBool::new(share),
        }
    }

    pub fn set(&self, share: bool) {
        self.share.store(share, Ordering::SeqCst);
    }

    fn share(&self) -> bool {
        self.share.load(Ordering::SeqCst)
    }
}

/// Whether an event may be sent right now (see `sends`).
pub fn gate_open(app: &AppHandle) -> bool {
    let share = app
        .try_state::<UsageGate>()
        .is_some_and(|gate| gate.share());
    // Offline mode fails closed: no API state means no sending.
    let offline = app
        .try_state::<ApiState>()
        .is_none_or(|api| api.offline_mode().0);
    sends(app_key().is_some(), share, offline)
}

/// Queue an event by name, if the gate allows it. Callers pass only fixed
/// names (`UsageEvent` and the `usage_marks` markers). Never fails: usage counts
/// must not get in the way of anything.
pub(crate) fn send(app: &AppHandle, name: &str, props: Option<Value>) {
    if !gate_open(app) {
        return;
    }
    if let Err(error) = app.track_event(name, props) {
        tracing::debug!(%error, "could not queue a usage event");
    }
}

/// Count one event, if the gate allows it.
pub fn track(app: &AppHandle, event: &UsageEvent) {
    if let Some((name, props)) = event_payload(event) {
        send(app, name, props);
    }
}

/// The UI's way to count an event (page views, features).
#[tauri::command]
#[specta::specta]
pub fn track_usage(app: AppHandle, event: UsageEvent) -> Result<(), CommandError> {
    track(&app, &event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_only_with_a_key_the_switch_on_and_online() {
        assert!(sends(true, true, false));
        assert!(!sends(false, true, false), "no key built in");
        assert!(!sends(true, false, false), "switch off");
        assert!(!sends(true, true, true), "offline mode");
    }

    #[test]
    fn route_names_pass_and_anything_else_is_dropped() {
        assert_eq!(
            clean_page("deck-matchups"),
            Some("deck-matchups".to_string())
        );
        assert_eq!(clean_page("wrapped_2026"), Some("wrapped_2026".to_string()));
        assert_eq!(clean_page(""), None);
        assert_eq!(clean_page("Decks"), None, "uppercase is not a route name");
        assert_eq!(clean_page("decks/123"), None);
        assert_eq!(clean_page("my deck"), None);
        assert_eq!(clean_page(&"a".repeat(41)), None);
    }

    #[test]
    fn each_event_has_a_fixed_name_and_small_props() {
        assert_eq!(
            event_payload(&UsageEvent::PageViewed {
                page: "dashboard".to_string()
            }),
            Some(("page_viewed", Some(json!({ "page": "dashboard" }))))
        );
        assert_eq!(
            event_payload(&UsageEvent::FeatureUsed {
                feature: Feature::CopyImage
            }),
            Some(("feature_used", Some(json!({ "feature": "copy_image" }))))
        );
        assert_eq!(
            event_payload(&UsageEvent::WrappedCardCopied {
                card: WrappedCardKind::MvpCard
            }),
            Some(("wrapped_card_copied", Some(json!({ "card": "mvp_card" }))))
        );
        assert_eq!(
            event_payload(&UsageEvent::PageViewed {
                page: "C:/Users/someone".to_string()
            }),
            None
        );
    }

    #[test]
    fn the_ui_sends_events_tagged_by_kind() {
        let event: UsageEvent =
            serde_json::from_value(json!({ "kind": "featureUsed", "feature": "copyImage" }))
                .expect("parse");
        assert_eq!(
            event,
            UsageEvent::FeatureUsed {
                feature: Feature::CopyImage
            }
        );
        let copied: UsageEvent =
            serde_json::from_value(json!({ "kind": "wrappedCardCopied", "card": "yearInNumbers" }))
                .expect("parse");
        assert_eq!(
            copied,
            UsageEvent::WrappedCardCopied {
                card: WrappedCardKind::YearInNumbers
            }
        );
        let not_a_card = serde_json::from_value::<UsageEvent>(
            json!({ "kind": "wrappedCardCopied", "card": "Some Deck Name" }),
        );
        assert!(not_a_card.is_err(), "only listed card kinds parse");
        let unknown = serde_json::from_value::<UsageEvent>(json!({ "kind": "anything" }));
        assert!(unknown.is_err(), "only listed events parse");
    }

    #[test]
    fn the_gate_follows_the_switch() {
        let gate = UsageGate::new(true);
        assert!(gate.share());
        gate.set(false);
        assert!(!gate.share());
    }
}
