//! The league card's artwork (overlay spec section 9.4, 0.x
//! `ResolveOverlayArt`): which mode applies and the image it shows.

use crate::options::OverlayArtwork;

/// The artwork mode in force. Unset means a user from before the setting
/// existed: an existing background keeps showing (`Custom`), everyone else
/// gets the deck cover (0.x `ResolveOverlayArt::mode`, read time, so no
/// migration). `has_background` is whether the background file exists.
pub fn art_mode(setting: Option<OverlayArtwork>, has_background: bool) -> OverlayArtwork {
    match setting {
        Some(mode) => mode,
        None if has_background => OverlayArtwork::Custom,
        None => OverlayArtwork::Deck,
    }
}

/// The image for a mode (0.x `ResolveOverlayArt::run`): `Deck` the deck's
/// art, `Thumbnail` the deck's art too (shown beside the text), `NoArt` none, `Custom` the background, else the deck's art when the
/// background is gone. The same rule serves the window (local URLs) and the
/// hosted page (public URLs): the caller passes the matching pair.
pub fn window_art(
    mode: OverlayArtwork,
    deck_art: Option<String>,
    background_url: Option<String>,
) -> Option<String> {
    match mode {
        OverlayArtwork::Deck | OverlayArtwork::Thumbnail => deck_art,
        OverlayArtwork::NoArt => None,
        OverlayArtwork::Custom => background_url.or(deck_art),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::OverlaySize;

    const KARN: &str = "https://cards.scryfall.io/art_crop/karn.jpg";
    const BACKGROUND: &str = "overlay://localhost/background";

    fn karn() -> Option<String> {
        Some(KARN.to_string())
    }

    fn background() -> Option<String> {
        Some(BACKGROUND.to_string())
    }

    // ResolveOverlayArtTest

    #[test]
    fn defaults_the_size_to_full_and_stores_compact() {
        // The stored value round trip is U5's settings test; here the type's
        // default and its two values.
        assert_eq!(OverlaySize::default(), OverlaySize::Full);
        assert_ne!(OverlaySize::Compact, OverlaySize::Full);
    }

    #[test]
    fn defaults_artwork_to_deck_when_nothing_is_set() {
        assert_eq!(art_mode(None, false), OverlayArtwork::Deck);
    }

    #[test]
    fn defaults_artwork_to_custom_for_a_user_with_a_legacy_uploaded_background() {
        let mode = art_mode(None, true);

        assert_eq!(mode, OverlayArtwork::Custom);
        assert_eq!(window_art(mode, karn(), background()), background());
    }

    #[test]
    fn uses_the_deck_cover_in_deck_mode() {
        let mode = art_mode(Some(OverlayArtwork::Deck), true);

        assert_eq!(window_art(mode, karn(), background()), karn());
    }

    #[test]
    fn returns_no_art_in_none_mode_even_with_a_deck_and_a_custom_image() {
        let mode = art_mode(Some(OverlayArtwork::NoArt), true);

        assert_eq!(window_art(mode, karn(), background()), None);
    }

    #[test]
    fn falls_back_to_the_deck_cover_when_the_custom_file_is_missing() {
        let mode = art_mode(Some(OverlayArtwork::Custom), false);

        assert_eq!(window_art(mode, karn(), None), karn());
    }

    #[test]
    fn returns_null_with_no_deck_in_deck_mode() {
        assert_eq!(window_art(OverlayArtwork::Deck, None, None), None);
    }

    // "finds the deck of the most recently started match" is U4's
    // `store::overlay::last_played_deck` test; the idle card's use of it is
    // `league::tests::is_idle_with_no_league_showing_the_last_played_deck_art`.

    #[test]
    fn a_custom_mode_with_neither_image_shows_none() {
        assert_eq!(window_art(OverlayArtwork::Custom, None, None), None);
    }

    #[test]
    fn the_side_thumbnail_shows_the_deck_cover() {
        let mode = art_mode(Some(OverlayArtwork::Thumbnail), true);
        assert_eq!(window_art(mode, karn(), background()), karn());
        assert_eq!(window_art(mode, None, background()), None);
    }
}
