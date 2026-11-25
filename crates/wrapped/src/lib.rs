//! MyMTGO Wrapped: the player's year in Magic as a sequence of cards
//! (spec `docs/superpowers/specs/2026-11-24-wrapped-design.md`).
//!
//! This crate is pure. It owns the bundle (the JSON the Wrapped page renders),
//! the year window, the rule for when Wrapped is shown, the input rows and the
//! builder that turns them into cards. The clock and the timezone are passed
//! in.

pub mod build;
pub mod bundle;
mod card_rules;
mod decks;
pub mod input;
mod numbers;
#[cfg(test)]
mod profiles;
mod story;
pub mod when;
pub mod window;

pub use build::build;
pub use bundle::*;
pub use input::*;
pub use when::{play_period, prefers_weekends};
pub use window::{wrapped_year, year_window};
