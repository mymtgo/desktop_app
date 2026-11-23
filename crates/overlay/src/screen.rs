//! Where an overlay window reopens (overlay spec 5.2, ruling 8).
//!
//! Every window reopens where the player left it. It is centred only when it
//! was never opened (nothing saved) or its saved bounds lie on no monitor
//! that is still connected (the monitor was unplugged or disabled). Rects are
//! physical pixels, as the window state plugin saves them and the monitors
//! report them.

/// A rectangle: top-left corner and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Where a window opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// At its saved bounds.
    Restore(Rect),
    /// Centred on this monitor (the primary one), or wherever the system
    /// centres a new window when no primary monitor is known.
    Centre(Option<Rect>),
}

/// The least width of a window that must lie on a monitor for it to count as
/// on screen: a one-pixel sliver cannot be grabbed to drag it back (#303).
pub const MIN_VISIBLE_WIDTH: u32 = 40;
/// The least height that must lie on a monitor, as `MIN_VISIBLE_WIDTH`.
pub const MIN_VISIBLE_HEIGHT: u32 = 20;

/// Whether at least `MIN_VISIBLE_WIDTH` by `MIN_VISIBLE_HEIGHT` pixels of a
/// window's bounds lie on one monitor (the whole window when it is smaller).
pub fn on_any_monitor(rect: Rect, monitors: &[Rect]) -> bool {
    monitors.iter().any(|monitor| overlaps(rect, *monitor))
}

/// Where a window opens: its saved bounds when they are on a monitor, else
/// centred on the primary monitor.
pub fn placement(saved: Option<Rect>, monitors: &[Rect], primary: Option<Rect>) -> Placement {
    match saved {
        Some(rect) if on_any_monitor(rect, monitors) => Placement::Restore(rect),
        _ => Placement::Centre(primary),
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    let a_right = i64::from(a.x) + i64::from(a.width);
    let a_bottom = i64::from(a.y) + i64::from(a.height);
    let b_right = i64::from(b.x) + i64::from(b.width);
    let b_bottom = i64::from(b.y) + i64::from(b.height);
    let visible_width = a_right.min(b_right) - i64::from(a.x).max(i64::from(b.x));
    let visible_height = a_bottom.min(b_bottom) - i64::from(a.y).max(i64::from(b.y));
    let needed_width = i64::from(a.width.min(MIN_VISIBLE_WIDTH));
    let needed_height = i64::from(a.height.min(MIN_VISIBLE_HEIGHT));
    visible_width > 0
        && visible_height > 0
        && visible_width >= needed_width
        && visible_height >= needed_height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn primary() -> Rect {
        rect(0, 0, 1920, 1080)
    }

    fn left_monitor() -> Rect {
        rect(-2560, 0, 2560, 1440)
    }

    #[test]
    fn a_window_on_a_missing_monitor_is_not_on_screen() {
        let on_left = rect(-1800, 200, 320, 640);

        assert!(on_any_monitor(on_left, &[primary(), left_monitor()]));
        assert!(!on_any_monitor(on_left, &[primary()]));
        assert!(!on_any_monitor(on_left, &[]));
    }

    #[test]
    fn a_window_touching_a_monitor_edge_only_is_not_on_it() {
        // Ends exactly where the primary monitor starts.
        assert!(!on_any_monitor(rect(-320, 0, 320, 400), &[primary()]));
        // One pixel over the edge is too thin a strip to grab.
        assert!(!on_any_monitor(rect(-319, 0, 320, 400), &[primary()]));
    }

    #[test]
    fn a_window_needs_a_visible_strip_of_40_by_20() {
        let monitors = [primary()];
        // 39 pixels wide shows too little, 40 is enough.
        assert!(!on_any_monitor(rect(-281, 100, 320, 400), &monitors));
        assert!(on_any_monitor(rect(-280, 100, 320, 400), &monitors));
        // 19 pixels tall shows too little, 20 is enough.
        assert!(!on_any_monitor(rect(100, -381, 320, 400), &monitors));
        assert!(on_any_monitor(rect(100, -380, 320, 400), &monitors));
        // Past the right and bottom edges the same way.
        assert!(!on_any_monitor(rect(1881, 100, 320, 400), &monitors));
        assert!(on_any_monitor(rect(1880, 100, 320, 400), &monitors));
        assert!(!on_any_monitor(rect(100, 1061, 320, 400), &monitors));
        assert!(on_any_monitor(rect(100, 1060, 320, 400), &monitors));
    }

    #[test]
    fn a_window_smaller_than_the_strip_must_be_wholly_visible() {
        let monitors = [primary()];
        assert!(on_any_monitor(rect(10, 10, 30, 10), &monitors));
        assert!(!on_any_monitor(rect(-1, 10, 30, 10), &monitors));
    }

    #[test]
    fn a_saved_position_on_a_present_monitor_is_restored() {
        let saved = rect(1500, 700, 320, 640);

        assert_eq!(
            placement(Some(saved), &[primary(), left_monitor()], Some(primary())),
            Placement::Restore(saved)
        );
        // Partly off screen is still on a monitor: restored as left.
        let partly_off = rect(1800, 900, 320, 640);
        assert_eq!(
            placement(Some(partly_off), &[primary()], Some(primary())),
            Placement::Restore(partly_off)
        );
    }

    #[test]
    fn a_window_is_centred_only_without_a_saved_position_or_monitor() {
        let on_left = rect(-1800, 200, 320, 640);

        // Never opened.
        assert_eq!(
            placement(None, &[primary()], Some(primary())),
            Placement::Centre(Some(primary()))
        );
        // Its monitor is gone.
        assert_eq!(
            placement(Some(on_left), &[primary()], Some(primary())),
            Placement::Centre(Some(primary()))
        );
        // Its monitor is back.
        assert_eq!(
            placement(Some(on_left), &[primary(), left_monitor()], Some(primary())),
            Placement::Restore(on_left)
        );
        // No primary monitor known.
        assert_eq!(placement(None, &[], None), Placement::Centre(None));
    }
}
