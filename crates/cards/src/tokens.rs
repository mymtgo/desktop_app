//! Tokens from MTGO's own XML (0.x `PopulateTokensFromXml`, cards survey
//! section 3.2).
//!
//! MTGO ships a `CardDataSource` folder: `client_TOK.xml` lists every token
//! (`DigitalObject` elements) and points at string tables by id
//! (`CARDNAME_STRING.xml`, `COLOR.xml`, `CREATURE_TYPE_STRING0.xml`). The
//! app reads the files; this module only parses their text and matches the
//! tokens to nameless stubs. Like the deck file reader, it is a small tag
//! scanner, not an XML library: unreadable text gives nothing.

use std::collections::{BTreeMap, BTreeSet};

/// One token printing from `client_TOK.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenDef {
    /// The catalog id (`DOC_1001` gives `"1001"`).
    pub catalog_id: String,
    /// The `id` attribute of the `CARDNAME_STRING` child.
    pub cardname_id: String,
    /// The `id` attribute of the `COLOR` child (empty when absent).
    pub color_id: String,
    /// The `id` attribute of the `CREATURE_TYPE_STRING0` child (empty when
    /// absent).
    pub creature_type_id: String,
    pub is_creature: bool,
    pub is_artifact: bool,
    pub is_enchantment: bool,
    pub is_land: bool,
}

/// What a nameless stub learns from the token XML. The rarity is always
/// `token` (0.x marked XML tokens that way).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCard {
    pub catalog_id: u32,
    pub name: String,
    /// The type line (`Token Artifact Creature`).
    pub card_type: String,
    /// The creature type, when the token has one.
    pub sub_type: Option<String>,
    /// `W,U`, or `C` for colourless and unknown colours.
    pub color_identity: String,
}

/// The rarity 0.x stored on XML tokens; `rarity = 'token'` makes a row a
/// token for the lookups.
pub const TOKEN_RARITY: &str = "token";

/// A string table (`<{table}_ITEM id="...">value</...>`): id to value. Items
/// without an id or with an empty value are skipped.
pub fn parse_string_table(xml: &str, table: &str) -> BTreeMap<String, String> {
    let item = format!("{table}_ITEM");
    let mut map = BTreeMap::new();
    let mut rest = xml;
    while let Some(tag) = next_tag(rest) {
        rest = &rest[tag.end..];
        if tag.closing || tag.self_closing || tag.name != item {
            continue;
        }
        let Some(id) = tag.attribute("id").filter(|id| !id.is_empty()) else {
            continue;
        };
        // The text up to the next tag is the value.
        let text_end = rest.find('<').unwrap_or(rest.len());
        let value = decode_entities(&rest[..text_end]);
        if !value.is_empty() {
            map.insert(id, value);
        }
    }
    map
}

/// The tokens of `client_TOK.xml`: each `DigitalObject` whose
/// `DigitalObjectCatalogID` starts `DOC_`, foil clones (a `CLONE_ID` child)
/// skipped.
pub fn parse_token_file(xml: &str) -> Vec<TokenDef> {
    let mut tokens = Vec::new();
    // The object being read: its catalog id attribute, children so far and
    // whether a `CLONE_ID` was among them.
    let mut current: Option<(String, TokenDef, bool)> = None;
    let mut rest = xml;
    while let Some(tag) = next_tag(rest) {
        rest = &rest[tag.end..];
        if tag.name == "DigitalObject" {
            if tag.closing {
                if let Some((catalog, token, clone)) = current.take() {
                    push_token(&mut tokens, &catalog, token, clone);
                }
                continue;
            }
            // An object that never closed ends where the next one starts.
            if let Some((catalog, token, clone)) = current.take() {
                push_token(&mut tokens, &catalog, token, clone);
            }
            let catalog = tag.attribute("DigitalObjectCatalogID").unwrap_or_default();
            if tag.self_closing {
                push_token(&mut tokens, &catalog, TokenDef::default(), false);
            } else {
                current = Some((catalog, TokenDef::default(), false));
            }
            continue;
        }
        let Some((_, token, clone)) = current.as_mut() else {
            continue;
        };
        if tag.closing {
            continue;
        }
        let id = || tag.attribute("id").unwrap_or_default();
        match tag.name.as_str() {
            "CLONE_ID" => *clone = true,
            "CARDNAME_STRING" => token.cardname_id = id(),
            "COLOR" => token.color_id = id(),
            "CREATURE_TYPE_STRING0" => token.creature_type_id = id(),
            "IS_CREATURE" => token.is_creature = true,
            "IS_ARTIFACT" => token.is_artifact = true,
            "IS_ENCHANTMENT" => token.is_enchantment = true,
            "IS_LAND" => token.is_land = true,
            _ => {}
        }
    }
    if let Some((catalog, token, clone)) = current.take() {
        push_token(&mut tokens, &catalog, token, clone);
    }
    tokens
}

fn push_token(tokens: &mut Vec<TokenDef>, catalog: &str, mut token: TokenDef, clone: bool) {
    if clone {
        return;
    }
    let Some(id) = catalog.strip_prefix("DOC_") else {
        return;
    };
    token.catalog_id = id.to_string();
    tokens.push(token);
}

/// The type line from the flags: `Token`, then `Artifact`, `Enchantment`,
/// `Land` and `Creature` in that order when flagged.
pub fn type_line(token: &TokenDef) -> String {
    let mut parts = vec!["Token"];
    if token.is_artifact {
        parts.push("Artifact");
    }
    if token.is_enchantment {
        parts.push("Enchantment");
    }
    if token.is_land {
        parts.push("Land");
    }
    if token.is_creature {
        parts.push("Creature");
    }
    parts.join(" ")
}

/// MTGO's colour text as a colour identity: `COLOR_WHITE|COLOR_BLUE` is
/// `W,U`; colourless, unknown or nothing is `C`; `C` is dropped when real
/// colours are present.
pub fn map_color(mtgo_color: Option<&str>) -> String {
    let Some(text) = mtgo_color.filter(|text| !text.is_empty()) else {
        return "C".to_string();
    };
    let mut colors = Vec::new();
    for part in text.split('|') {
        let letter = match part.trim() {
            "COLOR_WHITE" => "W",
            "COLOR_BLUE" => "U",
            "COLOR_BLACK" => "B",
            "COLOR_RED" => "R",
            "COLOR_GREEN" => "G",
            "COLOR_COLORLESS" => "C",
            _ => continue,
        };
        colors.push(letter);
    }
    let real: Vec<&str> = colors.into_iter().filter(|color| *color != "C").collect();
    if real.is_empty() {
        return "C".to_string();
    }
    real.join(",")
}

/// Match the tokens to the nameless stubs (`nameless`, by catalog id): a
/// token whose name id has a name in `names` fills its stub. Tokens
/// without a stub, or without a name, give nothing. In catalog id order.
pub fn token_cards(
    tokens: &[TokenDef],
    names: &BTreeMap<String, String>,
    colors: &BTreeMap<String, String>,
    creature_types: &BTreeMap<String, String>,
    nameless: &[u32],
) -> Vec<TokenCard> {
    let wanted: BTreeSet<u32> = nameless.iter().copied().collect();
    let mut found: BTreeMap<u32, TokenCard> = BTreeMap::new();
    if names.is_empty() {
        return Vec::new();
    }
    for token in tokens {
        let Ok(catalog_id) = token.catalog_id.parse::<u32>() else {
            continue;
        };
        if !wanted.contains(&catalog_id) {
            continue;
        }
        let Some(name) = names.get(&token.cardname_id) else {
            continue;
        };
        found.insert(
            catalog_id,
            TokenCard {
                catalog_id,
                name: name.clone(),
                card_type: type_line(token),
                sub_type: creature_types.get(&token.creature_type_id).cloned(),
                color_identity: map_color(colors.get(&token.color_id).map(String::as_str)),
            },
        );
    }
    found.into_values().collect()
}

/// One tag found by `next_tag`.
struct Tag {
    name: String,
    /// The attribute text after the name.
    attributes: String,
    closing: bool,
    self_closing: bool,
    /// Where the tag ends in the text searched (just past `>`).
    end: usize,
}

impl Tag {
    /// An attribute's value (double or single quoted), entities decoded.
    fn attribute(&self, name: &str) -> Option<String> {
        let mut rest = self.attributes.as_str();
        loop {
            rest = rest.trim_start();
            let equals = rest.find('=')?;
            let key = rest[..equals].trim();
            let after = rest[equals + 1..].trim_start();
            let quote = after.chars().next()?;
            if quote != '"' && quote != '\'' {
                return None;
            }
            let value_end = after[1..].find(quote)?;
            let value = &after[1..1 + value_end];
            if key == name {
                return Some(decode_entities(value));
            }
            rest = &after[value_end + 2..];
        }
    }
}

/// The next element tag in `text` (declarations, comments and processing
/// instructions skipped), `None` at the end or at a tag that never closes.
fn next_tag(text: &str) -> Option<Tag> {
    let mut offset = 0;
    loop {
        let start = offset + text[offset..].find('<')?;
        let after = &text[start + 1..];
        if after.starts_with("!--") {
            let close = after.find("-->")?;
            offset = start + 1 + close + 3;
            continue;
        }
        if after.starts_with('?') || after.starts_with('!') {
            let close = after.find('>')?;
            offset = start + 1 + close + 1;
            continue;
        }
        let close = after.find('>')?;
        let mut inner = &after[..close];
        let closing = inner.starts_with('/');
        if closing {
            inner = &inner[1..];
        }
        let self_closing = inner.ends_with('/');
        if self_closing {
            inner = &inner[..inner.len() - 1];
        }
        let name_end = inner
            .find(|c: char| c.is_whitespace())
            .unwrap_or(inner.len());
        return Some(Tag {
            name: inner[..name_end].to_string(),
            attributes: inner[name_end..].to_string(),
            closing,
            self_closing,
            end: start + 1 + close + 1,
        });
    }
}

/// The longest entity body this decoder reads, between `&` and `;`
/// (`#x10FFFF` and `#1114111` are eight).
const MAX_ENTITY_BYTES: usize = 10;

/// The five named entities and numeric references; anything else is kept
/// as written. A `;` is looked for only within [`MAX_ENTITY_BYTES`] of the
/// `&`, so a long run of `&` stays linear.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let window = &after.as_bytes()[1..after.len().min(MAX_ENTITY_BYTES + 2)];
        let Some(semi) = window.iter().position(|byte| *byte == b';') else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        // `semi` counts from the byte after the `&`.
        let semi = semi + 1;
        let entity = &after[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => numeric_entity(entity),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn numeric_entity(entity: &str) -> Option<char> {
    let number = entity.strip_prefix('#')?;
    let code = match number
        .strip_prefix('x')
        .or_else(|| number.strip_prefix('X'))
    {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => number.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOK: &str = include_str!("../../../fixtures/cards/CardDataSource/client_TOK.xml");
    const NAMES: &str = include_str!("../../../fixtures/cards/CardDataSource/CARDNAME_STRING.xml");
    const COLORS: &str = include_str!("../../../fixtures/cards/CardDataSource/COLOR.xml");
    const TYPES: &str =
        include_str!("../../../fixtures/cards/CardDataSource/CREATURE_TYPE_STRING0.xml");

    /// 0.x `PopulateTokensFromXml::run` over the fixture folder for these
    /// nameless stubs.
    fn populate(nameless: &[u32]) -> Vec<TokenCard> {
        token_cards(
            &parse_token_file(TOK),
            &parse_string_table(NAMES, "CARDNAME_STRING"),
            &parse_string_table(COLORS, "COLOR"),
            &parse_string_table(TYPES, "CREATURE_TYPE_STRING0"),
            nameless,
        )
    }

    fn token(
        catalog_id: u32,
        name: &str,
        card_type: &str,
        sub: Option<&str>,
        ci: &str,
    ) -> TokenCard {
        TokenCard {
            catalog_id,
            name: name.to_string(),
            card_type: card_type.to_string(),
            sub_type: sub.map(str::to_string),
            color_identity: ci.to_string(),
        }
    }

    // 0.x PopulateTokensFromXmlTest: "populates token cards from XML".
    #[test]
    fn populates_token_cards_from_xml() {
        assert_eq!(
            populate(&[1001, 1003]),
            vec![
                token(1001, "Goblin", "Token Creature", Some("Goblin"), "R"),
                token(1003, "Zombie", "Token Creature", Some("Zombie"), "B"),
            ]
        );
    }

    // "skips CLONE_ID entries (foils)".
    #[test]
    fn skips_clone_id_entries() {
        assert_eq!(populate(&[1002]), Vec::new());
        assert!(
            parse_token_file(TOK)
                .iter()
                .all(|token| token.catalog_id != "1002")
        );
    }

    // "handles artifact tokens".
    #[test]
    fn handles_artifact_tokens() {
        assert_eq!(
            populate(&[1004]),
            vec![token(1004, "Treasure", "Token Artifact", None, "C")]
        );
    }

    // "handles enchantment creature tokens with multi-color".
    #[test]
    fn handles_enchantment_creature_tokens_with_multi_color() {
        assert_eq!(
            populate(&[1006]),
            vec![token(
                1006,
                "Food",
                "Token Enchantment Creature",
                Some("Goblin"),
                "W,U"
            )]
        );
    }

    // "only updates stub cards that match token IDs": 9999 is no token.
    #[test]
    fn only_updates_stub_cards_that_match_token_ids() {
        assert_eq!(
            populate(&[9999, 1001]),
            vec![token(1001, "Goblin", "Token Creature", Some("Goblin"), "R")]
        );
    }

    // "accepts a pre-loaded card collection": only the stubs given.
    #[test]
    fn accepts_a_pre_loaded_card_collection() {
        let found = populate(&[1001]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Goblin");
    }

    // "returns 0 when data path is missing": no files read means empty
    // text, which gives nothing (the app's folder search is tested there).
    #[test]
    fn returns_nothing_when_the_files_are_missing() {
        let nothing = token_cards(
            &parse_token_file(""),
            &parse_string_table("", "CARDNAME_STRING"),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[1001],
        );
        assert!(nothing.is_empty());
        // Tokens without a name table give nothing either.
        let no_names = token_cards(
            &parse_token_file(TOK),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[1001],
        );
        assert!(no_names.is_empty());
    }

    // "returns 0 when no stub cards exist".
    #[test]
    fn returns_nothing_when_no_stub_cards_exist() {
        assert!(populate(&[]).is_empty());
    }

    // "is idempotent, running twice produces same result": the second run
    // has no nameless stubs left (the first filled them).
    #[test]
    fn is_idempotent() {
        assert_eq!(populate(&[1001]).len(), 1);
        assert_eq!(populate(&[1001]), populate(&[1001]));
        assert!(populate(&[]).is_empty());
    }

    #[test]
    fn string_tables_skip_empty_values_and_decode_entities() {
        let xml = r#"<?xml version="1.0"?><!-- c --><T_ITEMS>
            <T_ITEM id="a">Q &amp; Q</T_ITEM><T_ITEM id="b"></T_ITEM>
            <T_ITEM id='c'>Qc&#33;</T_ITEM><T_ITEM>no id</T_ITEM><OTHER_ITEM id="d">x</OTHER_ITEM>
        </T_ITEMS>"#;
        let table = parse_string_table(xml, "T");
        assert_eq!(table.len(), 2);
        assert_eq!(table["a"], "Q & Q");
        assert_eq!(table["c"], "Qc!");
    }

    #[test]
    fn entity_decoding_is_linear_on_a_long_run_of_ampersands() {
        let text = format!("{};", "&".repeat(1_000_000));
        let started = std::time::Instant::now();
        assert_eq!(decode_entities(&text), text);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn an_ampersand_without_a_near_semicolon_is_kept_as_written() {
        assert_eq!(decode_entities("Q & Q &amp; Q"), "Q & Q & Q");
        assert_eq!(
            decode_entities("&#x10FFFF;&#1114111;"),
            "\u{10FFFF}\u{10FFFF}"
        );
        assert_eq!(decode_entities("&#0000000033;"), "&#0000000033;");
        assert_eq!(decode_entities("tail &"), "tail &");
    }

    #[test]
    fn type_lines_follow_0x_order() {
        let all = TokenDef {
            is_creature: true,
            is_artifact: true,
            is_enchantment: true,
            is_land: true,
            ..TokenDef::default()
        };
        assert_eq!(type_line(&all), "Token Artifact Enchantment Land Creature");
        assert_eq!(type_line(&TokenDef::default()), "Token");
    }

    #[test]
    fn colours_map_as_0x() {
        assert_eq!(map_color(None), "C");
        assert_eq!(map_color(Some("")), "C");
        assert_eq!(map_color(Some("COLOR_COLORLESS")), "C");
        assert_eq!(map_color(Some("COLOR_GREEN")), "G");
        assert_eq!(map_color(Some("COLOR_WHITE|COLOR_BLUE")), "W,U");
        assert_eq!(map_color(Some("COLOR_COLORLESS|COLOR_RED")), "R");
        assert_eq!(map_color(Some("COLOR_PURPLE")), "C");
    }

    #[test]
    fn non_doc_and_unclosed_objects_are_handled() {
        let xml = r#"<CardSet><DigitalObject DigitalObjectCatalogID="X_5"><IS_LAND/></DigitalObject>
            <DigitalObject DigitalObjectCatalogID="DOC_7"><IS_LAND value="1"/>"#;
        let tokens = parse_token_file(xml);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].catalog_id, "7");
        assert!(tokens[0].is_land);
    }
}
