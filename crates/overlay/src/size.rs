//! The league card's size per theme (league overlay themes spec 5): the one
//! table the window, the OBS source and (through a TS copy in
//! `ui/src/lib/overlaySettings.ts` and the API) the preview and hosted page
//! use.

use crate::options::{LeagueTheme, OverlaySize};

/// The card's width and height in CSS pixels, padding not included.
pub fn card_size(theme: LeagueTheme, size: OverlaySize) -> (u32, u32) {
    match (theme, size) {
        (LeagueTheme::Classic, OverlaySize::Full) => (300, 80),
        (LeagueTheme::Classic, OverlaySize::Compact) => (240, 58),
        (LeagueTheme::Versus, OverlaySize::Full) => (580, 104),
        (LeagueTheme::Versus, OverlaySize::Compact) => (380, 64),
    }
}

/// The other event strip's height under the card (event overlay spec 8.2):
/// 2 px padding around a 12 px line, a 16 px line on Versus Full.
pub fn strip_height(theme: LeagueTheme, size: OverlaySize) -> u32 {
    match (theme, size) {
        (LeagueTheme::Classic, OverlaySize::Full) => 16,
        (LeagueTheme::Classic, OverlaySize::Compact) => 16,
        (LeagueTheme::Versus, OverlaySize::Full) => 20,
        (LeagueTheme::Versus, OverlaySize::Compact) => 16,
    }
}

/// The card's size with the strip below it while one shows: the same width,
/// the strip's height added at the bottom (spec 8.1: the card itself keeps
/// its size and place).
pub fn card_and_strip_size(theme: LeagueTheme, size: OverlaySize, strip: bool) -> (u32, u32) {
    let (width, height) = card_size(theme, size);
    if strip {
        (width, height + strip_height(theme, size))
    } else {
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_full_grows_by_a_16_px_strip() {
        assert_eq!(strip_height(LeagueTheme::Classic, OverlaySize::Full), 16);
        assert_eq!(
            card_and_strip_size(LeagueTheme::Classic, OverlaySize::Full, true),
            (300, 96)
        );
    }

    #[test]
    fn classic_compact_grows_by_a_16_px_strip() {
        assert_eq!(strip_height(LeagueTheme::Classic, OverlaySize::Compact), 16);
        assert_eq!(
            card_and_strip_size(LeagueTheme::Classic, OverlaySize::Compact, true),
            (240, 74)
        );
    }

    #[test]
    fn versus_full_grows_by_a_20_px_strip() {
        assert_eq!(strip_height(LeagueTheme::Versus, OverlaySize::Full), 20);
        assert_eq!(
            card_and_strip_size(LeagueTheme::Versus, OverlaySize::Full, true),
            (580, 124)
        );
    }

    #[test]
    fn versus_compact_grows_by_a_16_px_strip() {
        assert_eq!(strip_height(LeagueTheme::Versus, OverlaySize::Compact), 16);
        assert_eq!(
            card_and_strip_size(LeagueTheme::Versus, OverlaySize::Compact, true),
            (380, 80)
        );
    }

    #[test]
    fn without_a_strip_the_size_is_the_card() {
        for theme in [LeagueTheme::Classic, LeagueTheme::Versus] {
            for size in [OverlaySize::Full, OverlaySize::Compact] {
                assert_eq!(
                    card_and_strip_size(theme, size, false),
                    card_size(theme, size)
                );
            }
        }
    }

    #[test]
    fn every_theme_has_its_full_and_compact_size() {
        assert_eq!(
            card_size(LeagueTheme::Classic, OverlaySize::Full),
            (300, 80)
        );
        assert_eq!(
            card_size(LeagueTheme::Classic, OverlaySize::Compact),
            (240, 58)
        );
        assert_eq!(
            card_size(LeagueTheme::Versus, OverlaySize::Full),
            (580, 104)
        );
        assert_eq!(
            card_size(LeagueTheme::Versus, OverlaySize::Compact),
            (380, 64)
        );
    }
}
