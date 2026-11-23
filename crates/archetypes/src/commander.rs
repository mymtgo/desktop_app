//! Commander archetypes (Arena Brawl spec section 6): one archetype row per
//! format and commander set, with a deterministic uuid, a name and colours
//! built from the commanders' card rows.

use sha2::{Digest, Sha256};

/// The refusal for deleting a commander archetype: the next detection would
/// bring it back under its original name.
pub const COMMANDER_NO_DELETE: &str = "A commander archetype cannot be deleted.";

/// The refusal for editing a commander archetype's colours.
pub const COMMANDER_NO_COLOURS: &str = "A commander archetype's colours come from its commander.";

/// The archetype uuid for a format and its commanders' oracle ids: a
/// name-based (version 5 shape) uuid of `commander|<format>|<ids sorted,
/// joined with |>`, so every device and every rebuild agrees.
pub fn commander_uuid(format_key: &str, oracle_ids: &[String]) -> String {
    let mut sorted: Vec<&str> = oracle_ids.iter().map(String::as_str).collect();
    sorted.sort();
    let text = format!("commander|{format_key}|{}", sorted.join("|"));
    let digest = Sha256::digest(text.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

/// The archetype name: the commanders' names sorted, joined with ` & `.
pub fn commander_name(names: &[String]) -> String {
    let mut sorted: Vec<&str> = names.iter().map(String::as_str).collect();
    sorted.sort();
    sorted.join(" & ")
}

/// The combined colour identity of the commanders in the canonical stored
/// form (`"B,G"`, see `cards::colors::normalize_colors`); `None` for a
/// colourless set, as every colourless archetype stores NULL. Each input is
/// one card's identity letters, in any form.
pub fn commander_colors(identities: &[String]) -> Option<String> {
    let letters: String = cards::colors::WUBRG
        .iter()
        .filter(|colour| {
            identities
                .iter()
                .any(|identity| identity.contains(**colour))
        })
        .collect();
    cards::colors::normalize_colors(Some(&letters))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_uuid_ignores_the_order_of_the_oracle_ids() {
        assert_eq!(
            commander_uuid("historic_brawl", &ids(&["b", "a"])),
            commander_uuid("historic_brawl", &ids(&["a", "b"]))
        );
    }

    #[test]
    fn the_uuid_differs_by_format() {
        assert_ne!(
            commander_uuid("historic_brawl", &ids(&["a"])),
            commander_uuid("brawl", &ids(&["a"]))
        );
    }

    #[test]
    fn the_uuid_has_the_version_5_shape() {
        let uuid = commander_uuid("brawl", &ids(&["a", "b"]));
        let ok = uuid.len() == 36
            && uuid.split('-').map(str::len).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
            && uuid
                .chars()
                .all(|c| c == '-' || matches!(c, '0'..='9' | 'a'..='f'))
            && uuid.as_bytes()[14] == b'5'
            && matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b');
        assert!(ok, "{uuid}");
    }

    #[test]
    fn the_uuid_is_pinned() {
        assert_eq!(
            commander_uuid("historic_brawl", &ids(&["a", "b"])),
            "33719735-6790-58bf-afde-3151a4a4024c"
        );
    }

    #[test]
    fn the_name_sorts_partners() {
        assert_eq!(commander_name(&ids(&["Zur", "Akiri"])), "Akiri & Zur");
        assert_eq!(commander_name(&ids(&["Zur"])), "Zur");
    }

    #[test]
    fn the_colours_union_in_the_canonical_comma_form() {
        assert_eq!(commander_colors(&ids(&["BG", "W"])), Some("W,B,G".into()));
        assert_eq!(commander_colors(&ids(&["G,B"])), Some("B,G".into()));
    }

    #[test]
    fn a_colourless_commander_has_no_colours() {
        assert_eq!(commander_colors(&ids(&["", ""])), None);
        assert_eq!(commander_colors(&ids(&["C"])), None);
    }
}
