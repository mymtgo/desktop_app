//! A limited run's set code (0.x `ResolveLeagueSetCode`). Setting the run's
//! kind from the format code belongs to the leagues fold.

use std::sync::LazyLock;

use regex::Regex;

use crate::cards::CardFactsMap;
use crate::draft::Draft;

/// `HOBx3` (league panel).
static PANEL_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?<code>[A-Z0-9]{2,5})x\d+$").expect("valid regex"));
/// `S6FRA` (sealed: booster count, then the code; `S4STX4SOS` mixes sets).
static SEALED_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^S\d+(?<code>[A-Z0-9]{2,12})$").expect("valid regex"));
/// A draft format that is not one set repeated (`INSCUBE`, `INPSAP`).
static WHOLE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z0-9]{2,12}$").expect("valid regex"));
/// The format on a `league_joined` line.
static PANEL_FORMAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"PlayFormatCd=\s*(?<format>\S+)").expect("valid regex"));

/// The set code a format code names (0.x `fromPlayFormat`): `HOBx3`, `S6FRA`,
/// or `D` and one code repeated at least twice (`DHOBHOBHOB`). Codes are 2 to
/// 5 characters, so the shortest repeating unit is found rather than
/// splitting in threes. Vintage Cube is `DC03C03C03`, so its code is `C03`.
///
/// Unlike 0.x, a `D` format that is not one code repeated (QuickCube
/// `DINSCUBE`, `DSUPREME`, the mixed `DINPSAP`) is named by its whole body,
/// and a mixed sealed format (`S4STX4SOS`) by everything after the count:
/// 0.x fell back to the picks' most common set, a random one for a cube.
pub fn set_code_from_play_format(play_format: &str) -> Option<String> {
    if play_format.is_empty() {
        return None;
    }
    if let Some(found) = PANEL_CODE.captures(play_format) {
        return Some(found["code"].to_string());
    }
    if let Some(found) = SEALED_CODE.captures(play_format) {
        return Some(found["code"].to_string());
    }

    let body = play_format.strip_prefix('D')?;
    let length = body.len();
    for unit in 2..=5 {
        if length % unit != 0 || length / unit < 2 {
            continue;
        }
        let Some(code) = body.get(..unit) else {
            continue;
        };
        if code.repeat(length / unit) == body {
            return Some(code.to_string());
        }
    }
    if WHOLE_CODE.is_match(body) {
        return Some(body.to_string());
    }
    None
}

/// `Vintage Cube x3`: a game structure's name and its pack count.
static STRUCTURE_PACKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?<name>.+?)\s*x\d+$").expect("valid regex"));
/// A pack count anywhere in a structure (`STXx4 SOSx4`).
static PACK_COUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"x\d+(\s|$)").expect("valid regex"));

/// The name a limited run shows instead of its set code, from a match's
/// `GameStructureCd`: `Vintage Cube x3` is "Vintage Cube", `QuickCube`
/// stays itself. `None` when the structure is only the set code and its
/// pack count (`HOBx3`): the set's own name reads better. A mixed format
/// (`STXx4 SOSx4`) keeps every pack count.
pub fn format_name(set_code: Option<&str>, game_structure: Option<&str>) -> Option<String> {
    let structure = game_structure?.trim();
    if structure.is_empty() {
        return None;
    }
    let name = match STRUCTURE_PACKS.captures(structure) {
        Some(found) if !PACK_COUNT.is_match(&found["name"]) => found["name"].to_string(),
        _ => structure.to_string(),
    };
    if set_code == Some(name.as_str()) {
        return None;
    }
    Some(name)
}

/// The `PlayFormatCd` of a `league_joined` line's raw text.
pub fn panel_play_format(raw: &str) -> Option<String> {
    PANEL_FORMAT
        .captures(raw)
        .map(|found| found["format"].to_string())
}

/// The most common set code among the draft's picked cards (0.x
/// `fromPicks`). Each distinct card counts once, as 0.x counted card rows;
/// a tie goes to the set picked first. Unknown cards are skipped.
pub fn set_code_from_picks(draft: &Draft, cards: &CardFactsMap) -> Option<String> {
    let mut picked: Vec<u32> = Vec::new();
    for pick in &draft.picks {
        if let Some(catalog_id) = pick.picked_catalog_id
            && !picked.contains(&catalog_id)
        {
            picked.push(catalog_id);
        }
    }

    // (set code, cards), in first-seen order.
    let mut counts: Vec<(String, u32)> = Vec::new();
    for catalog_id in picked {
        let Some(set_code) = cards
            .get(&catalog_id)
            .and_then(|card| card.set_code.clone())
        else {
            continue;
        };
        match counts.iter_mut().find(|(code, _)| *code == set_code) {
            Some((_, count)) => *count += 1,
            None => counts.push((set_code, 1)),
        }
    }

    let mut best: Option<(String, u32)> = None;
    for (code, count) in counts {
        if best.as_ref().is_none_or(|(_, top)| count > *top) {
            best = Some((code, count));
        }
    }
    best.map(|(code, _)| code)
}

/// The run's set code (0.x `ResolveLeagueSetCode::run`): an existing one is
/// never replaced; otherwise the first that answers of the match format code,
/// the latest `league_joined` line for the run's league id (its raw text),
/// and the draft's picked cards.
pub fn resolve_set_code(
    existing: Option<&str>,
    match_play_format: Option<&str>,
    latest_panel_raw: Option<&str>,
    draft: Option<&Draft>,
    cards: &CardFactsMap,
) -> Option<String> {
    if let Some(existing) = existing.filter(|code| !code.is_empty()) {
        return Some(existing.to_string());
    }

    match_play_format
        .and_then(set_code_from_play_format)
        .or_else(|| {
            latest_panel_raw
                .and_then(panel_play_format)
                .and_then(|format| set_code_from_play_format(&format))
        })
        .or_else(|| draft.and_then(|draft| set_code_from_picks(draft, cards)))
        .filter(|code| !code.is_empty())
}
