//! The game overlay's window heights (overlay spec 7.2, 0.x
//! `ComputeGameOverlayHeight`).
//!
//! The window is a fixed region (the opponent header, or the "enable a
//! section" hint when nothing is on) that should always be fully visible, plus
//! a tab area (draw odds, reveals, sideboard) that scrolls inside and only
//! needs a sensible default. Heights are logical pixels.

/// Default height of the scrolling tab area.
pub const TAB_AREA: u32 = 520;

/// Least tab area a refit keeps before it grows the window to `TAB_AREA`.
pub const MIN_TAB_AREA: u32 = 120;

/// Smallest window that still shows a full opponent header.
pub const MIN_HEIGHT: u32 = 120;

/// Smallest collapsed strip: the tab bar alone, with the header off.
pub const COLLAPSED_MIN_HEIGHT: u32 = 40;

/// The opponent header's height before the page has measured it.
pub const OPPONENT_HEADER_ESTIMATE: u32 = 120;

/// The game overlay's width.
pub const DEFAULT_WIDTH: u32 = 320;

/// Which game overlay sections are switched on (settings
/// `overlay_show_opponent`, `overlay_show_draw_odds`, `overlay_show_reveals`,
/// `overlay_show_sideboard`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Sections {
    pub opponent: bool,
    pub draw_odds: bool,
    pub reveals: bool,
    pub sideboard: bool,
}

/// Whether any tab section is on.
pub fn has_tab_sections(sections: &Sections) -> bool {
    sections.draw_odds || sections.reveals || sections.sideboard
}

/// The expanded height: the fixed region, plus the tab area when a tab
/// section is on, never below `MIN_HEIGHT`.
pub fn height(has_tabs: bool, fixed: u32) -> u32 {
    let tab_area = if has_tabs { TAB_AREA } else { 0 };
    (fixed + tab_area).max(MIN_HEIGHT)
}

/// The collapsed height: the fixed region and the tab bar only, never below
/// `COLLAPSED_MIN_HEIGHT`.
pub fn collapsed(fixed: u32, bar: u32) -> u32 {
    (fixed + bar).max(COLLAPSED_MIN_HEIGHT)
}

/// The expanded height after the page measured its fixed region, given the
/// window's height now. Without a tab section the window follows the fixed
/// region. With one, the scrolling tab area absorbs the rest, so the height
/// the player chose (restored with the window) stays while the fixed region
/// and `MIN_TAB_AREA` fit in it.
pub fn refit(has_tabs: bool, fixed: u32, current: Option<u32>) -> u32 {
    if !has_tabs {
        return height(false, fixed);
    }
    let roomy = fixed + MIN_TAB_AREA;
    match current {
        Some(current) if current >= roomy && current >= MIN_HEIGHT => current,
        _ => height(true, fixed),
    }
}

/// The height of a window that has not rendered yet. Without a measured
/// fixed height it is estimated from the sections: the header estimate when
/// the opponent section is on, else 0.
pub fn from_settings(sections: &Sections, fixed: Option<u32>) -> u32 {
    let fixed = match fixed {
        Some(fixed) => fixed,
        None if sections.opponent => OPPONENT_HEADER_ESTIMATE,
        None => 0,
    };
    height(has_tab_sections(sections), fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(opponent: bool, draw_odds: bool, reveals: bool, sideboard: bool) -> Sections {
        Sections {
            opponent,
            draw_odds,
            reveals,
            sideboard,
        }
    }

    #[test]
    fn uses_the_fixed_content_height_alone_when_no_tab_section_is_enabled() {
        assert_eq!(height(false, 150), 150);
    }

    #[test]
    fn adds_the_tab_area_on_top_of_the_fixed_content_when_a_tab_section_is_enabled() {
        assert_eq!(height(true, 100), 100 + TAB_AREA);
    }

    #[test]
    fn never_goes_below_the_minimum_window_height() {
        assert_eq!(height(false, 10), MIN_HEIGHT);
    }

    #[test]
    fn estimates_the_fixed_content_from_settings_when_nothing_has_been_measured_yet() {
        assert_eq!(
            from_settings(&sections(true, false, false, false), None),
            OPPONENT_HEADER_ESTIMATE
        );
        assert_eq!(
            from_settings(&sections(true, true, false, false), None),
            OPPONENT_HEADER_ESTIMATE + TAB_AREA
        );
    }

    #[test]
    fn estimates_the_settings_hint_when_every_section_is_off() {
        assert_eq!(
            from_settings(&sections(false, false, false, false), None),
            MIN_HEIGHT
        );
    }

    #[test]
    fn a_measured_fixed_height_wins_over_the_estimate() {
        assert_eq!(
            from_settings(&sections(true, false, true, false), Some(90)),
            90 + TAB_AREA
        );
    }

    #[test]
    fn the_collapsed_strip_is_the_fixed_region_and_the_bar() {
        assert_eq!(collapsed(100, 32), 132);
        assert_eq!(collapsed(0, 20), COLLAPSED_MIN_HEIGHT);
    }

    #[test]
    fn a_refit_keeps_a_chosen_height_while_the_tab_area_has_room() {
        // The player's height stays, taller or shorter than the default.
        assert_eq!(refit(true, 100, Some(900)), 900);
        assert_eq!(
            refit(true, 100, Some(100 + MIN_TAB_AREA)),
            100 + MIN_TAB_AREA
        );
        // Too short for the header and some tabs (a tab section was just
        // switched on, or the header grew): the default tab area.
        assert_eq!(
            refit(true, 100, Some(100 + MIN_TAB_AREA - 1)),
            100 + TAB_AREA
        );
        assert_eq!(refit(true, 100, None), 100 + TAB_AREA);
    }

    #[test]
    fn a_refit_without_tabs_follows_the_fixed_region() {
        assert_eq!(refit(false, 142, Some(900)), 142);
        assert_eq!(refit(false, 20, Some(900)), MIN_HEIGHT);
    }

    #[test]
    fn any_of_the_three_tab_sections_counts() {
        assert!(!has_tab_sections(&sections(true, false, false, false)));
        assert!(has_tab_sections(&sections(false, true, false, false)));
        assert!(has_tab_sections(&sections(false, false, true, false)));
        assert!(has_tab_sections(&sections(false, false, false, true)));
    }
}
