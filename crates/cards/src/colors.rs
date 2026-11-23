//! Colour identity in its one canonical form (0.x `App\Support\ColorIdentity`).
//!
//! The canonical form is upper-case letters joined with commas ("U,B"), in
//! WUBRG order with colourless last, each letter once. Every page shows a
//! stored colour identity through `normalize_colors`, so one value reads the
//! same everywhere whatever order it was stored in.

/// The five colours, in WUBRG order.
pub const WUBRG: [char; 5] = ['W', 'U', 'B', 'R', 'G'];

/// The colour letters in canonical order: WUBRG, then colourless.
pub const COLOR_ORDER: [char; 6] = ['W', 'U', 'B', 'R', 'G', 'C'];

/// The canonical form of a stored or served colour identity: "UB", "U,B"
/// and "b,u" all give "U,B", "G,W" gives "W,G"; blank (or no colour letter
/// at all) is `None`.
pub fn normalize_colors(text: Option<&str>) -> Option<String> {
    let text = text?;
    let upper = text.to_uppercase();
    let mut letters = Vec::new();
    for letter in COLOR_ORDER {
        if upper.contains(letter) {
            letters.push(letter.to_string());
        }
    }
    if letters.is_empty() {
        None
    } else {
        Some(letters.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_come_out_in_wubrg_order_whatever_order_they_went_in() {
        assert_eq!(normalize_colors(Some("G,W")), Some("W,G".into()));
        assert_eq!(normalize_colors(Some("C,R")), Some("R,C".into()));
        assert_eq!(normalize_colors(Some("UB")), Some("U,B".into()));
        assert_eq!(normalize_colors(Some("u,b,u")), Some("U,B".into()));
        assert_eq!(normalize_colors(Some("")), None);
        assert_eq!(normalize_colors(Some("xyz")), None);
        assert_eq!(normalize_colors(None), None);
    }
}
