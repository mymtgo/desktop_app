//! The canonical writer.

use crate::header::result_text;
use crate::model::{
    AbilityKind, CardType, Cause, Choice, DeckCard, Entering, ExtArg, Game, LibraryEnd,
    LibraryPlace, Line, Mana, MoveAction, ObjRef, Position, PtValue, Record, SpecialAction,
    Statement, Step, Tag, TargetRef, Zone,
};

/// The required tags of spec section 4.2, in their fixed order.
const REQUIRED_TAGS: [&str; 12] = [
    "Mtgr",
    "Event",
    "Site",
    "Date",
    "Round",
    "Player1",
    "Player2",
    "Result",
    "Format",
    "Match",
    "Game",
    "Perspective",
];

/// Writes a record as canonical `.mtgr` text: LF endings, one blank line
/// between games, the file ending with a newline.
pub fn write(record: &Record) -> String {
    let mut out = String::new();
    for (index, game) in record.games.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        write_game(&mut out, game);
    }
    out
}

fn write_game(out: &mut String, game: &Game) {
    for tag in ordered_tags(&game.tags) {
        debug_assert!(is_tag_name(&tag.name), "bad tag name: {}", tag.name);
        out.push_str(&format!("[{} {}]\n", tag.name, quote(&tag.value)));
    }
    out.push('\n');
    for line in &game.lines {
        write_line(out, line);
    }
    out.push_str(result_text(game.result));
    out.push('\n');
}

/// The required tags first in their fixed order, then every other tag in
/// ASCII order of its name (spec section 4.5). A tag name is written once:
/// when a name repeats, the first tag wins and the rest are dropped.
fn ordered_tags(tags: &[Tag]) -> Vec<&Tag> {
    let mut ordered: Vec<&Tag> = Vec::new();
    for name in REQUIRED_TAGS {
        if let Some(tag) = tags.iter().find(|tag| tag.name == name) {
            ordered.push(tag);
        }
    }
    let mut others: Vec<&Tag> = Vec::new();
    for tag in tags {
        let required = REQUIRED_TAGS.contains(&tag.name.as_str());
        let seen = others.iter().any(|other| other.name == tag.name);
        if !required && !seen {
            others.push(tag);
        }
    }
    others.sort_by(|a, b| a.name.cmp(&b.name));
    ordered.extend(others);
    ordered
}

fn write_line(out: &mut String, line: &Line) {
    if let Statement::Escape(text) = &line.statement {
        out.push_str(&format!("% {}\n", without_controls(text)));
        return;
    }
    out.push_str(&format_time(line.time_ms));
    out.push(' ');
    for _ in 0..line.level {
        out.push_str("  ");
    }
    out.push_str(&statement_text(&line.statement));
    if let Some(cause) = &line.cause {
        out.push_str(" by ");
        out.push_str(&cause_text(cause));
    }
    out.push('\n');
}

/// `M:SS` from milliseconds, dropping the fraction. Negative times are 0:00.
fn format_time(time_ms: i64) -> String {
    let total_seconds = time_ms.max(0) / 1000;
    format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// Replaces every control character (tab, CR, LF and the rest) with a space,
/// because spec section 2 allows none inside a line.
fn without_controls(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

/// A tag name: an uppercase ASCII letter, then ASCII letters, digits or `_`.
fn is_tag_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_uppercase());
    first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A keyword: one or more lowercase ASCII letters or `-`.
fn is_keyword(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
}

/// An extension name: uppercase prefix, `_`, then letters, digits or `_`.
fn is_extension_name(name: &str) -> bool {
    match name.split_once('_') {
        Some((prefix, rest)) => {
            !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_uppercase())
                && !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// A string in double quotes with `\"`, `\\` and `\n` escapes (spec section 2).
fn quote(text: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            other if other.is_control() => quoted.push(' '),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

fn cause_text(cause: &Cause) -> String {
    match cause {
        Cause::Object(id) => format!("#{id}"),
        Cause::Cost(id) => format!("cost:#{id}"),
        Cause::Player(name) => quote(name),
        Cause::Rule(rule) => {
            debug_assert!(is_keyword(rule), "bad rule name: {rule}");
            format!("rule:{rule}")
        }
        Cause::Unknown => "?".to_string(),
    }
}

/// A card's name on a line that names it (`new`, `identify`, a deck
/// entry): quoted, or `?` when it cannot be determined (decision 0048).
fn card_name_text(name: &Option<String>) -> String {
    match name {
        Some(name) => quote(name),
        None => "?".to_string(),
    }
}

/// `#1 "Swamp"`, or `#8` when the name is not known.
fn obj_text(object: &ObjRef) -> String {
    match &object.name {
        Some(name) => format!("#{} {}", object.id, quote(name)),
        None => format!("#{}", object.id),
    }
}

fn target_text(target: &TargetRef) -> String {
    match target {
        TargetRef::Object(object) => obj_text(object),
        TargetRef::Player(name) => quote(name),
    }
}

fn zone_text(zone: Zone) -> &'static str {
    match zone {
        Zone::Library => "library",
        Zone::Hand => "hand",
        Zone::Graveyard => "graveyard",
        Zone::Battlefield => "battlefield",
        Zone::Stack => "stack",
        Zone::Exile => "exile",
        Zone::Command => "command",
        Zone::Ante => "ante",
        Zone::OutsideSideboard => "outside \"sideboard\"",
    }
}

fn card_type_text(card_type: CardType) -> &'static str {
    match card_type {
        CardType::Artifact => "artifact",
        CardType::Battle => "battle",
        CardType::Creature => "creature",
        CardType::Enchantment => "enchantment",
        CardType::Kindred => "kindred",
        CardType::Land => "land",
        CardType::Planeswalker => "planeswalker",
    }
}

/// ` types land creature`, or nothing for an empty list.
fn types_suffix(types: &[CardType]) -> String {
    if types.is_empty() {
        return String::new();
    }
    let words: Vec<&str> = types.iter().map(|t| card_type_text(*t)).collect();
    format!(" types {}", words.join(" "))
}

/// ` types land creature`, or ` types ?` for an empty list.
fn permanent_types_suffix(types: &[CardType]) -> String {
    if types.is_empty() {
        " types ?".to_string()
    } else {
        types_suffix(types)
    }
}

/// ` pt 2/1`, or nothing.
fn pt_suffix(pt: Option<(i32, i32)>) -> String {
    match pt {
        Some((power, toughness)) => format!(" pt {power}/{toughness}"),
        None => String::new(),
    }
}

/// `TYPES`, or `?` for an empty list.
fn types_text(types: &[CardType]) -> String {
    if types.is_empty() {
        return "?".to_string();
    }
    let words: Vec<&str> = types.iter().map(|t| card_type_text(*t)).collect();
    words.join(" ")
}

/// `PTVALUE`: `2/1`, `none` or `?`.
fn pt_value_text(value: PtValue) -> String {
    match value {
        PtValue::Pt(power, toughness) => format!("{power}/{toughness}"),
        PtValue::None => "none".to_string(),
        PtValue::Unknown => "?".to_string(),
    }
}

/// ` face "NAME"`, or nothing.
fn face_suffix(face: &Option<String>) -> String {
    match face {
        Some(face) => format!(" face {}", quote(face)),
        None => String::new(),
    }
}

/// `top` or `bottom`.
fn library_end_text(end: LibraryEnd) -> &'static str {
    match end {
        LibraryEnd::Top => "top",
        LibraryEnd::Bottom => "bottom",
    }
}

/// `top`, `top N`, `bottom` or `bottom ?` on a move (spec 7.1). `top 1`
/// is never written: the top is `top` (decision 0045).
fn library_place_text(place: LibraryPlace) -> String {
    match place {
        LibraryPlace::Top => "top".to_string(),
        LibraryPlace::FromTop(n) if n >= 2 => format!("top {n}"),
        LibraryPlace::FromTop(_) => "top".to_string(),
        LibraryPlace::Bottom => "bottom".to_string(),
        LibraryPlace::RandomBottom => "bottom ?".to_string(),
    }
}

/// `[ top N | bottom N]`: a library position (spec 6.5, 10.3).
fn position_suffix(position: &Option<Position>) -> String {
    match position {
        Some(position) => format!(" {} {}", library_end_text(position.end), position.n),
        None => String::new(),
    }
}

/// ` src PREFIX:ID` for each source id (spec 6.4, `src-id` in section 16).
fn sources_suffix(sources: &[String]) -> String {
    let mut text = String::new();
    for source in sources {
        debug_assert!(is_source_id(source), "bad source id {source}");
        text.push_str(" src ");
        text.push_str(source);
    }
    text
}

/// `1*(a-z) ":" 1*(ALPHA / DIGIT / "-" / "_" / ".")` (spec section 16).
fn is_source_id(source: &str) -> bool {
    let Some((prefix, id)) = source.split_once(':') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_lowercase())
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// A text as a quoted string, or `?` when the source does not report it
/// (spec 6.5, decision 0044).
fn text_or_unknown(text: &Option<String>) -> String {
    match text {
        Some(text) => quote(text),
        None => "?".to_string(),
    }
}

/// `[ face NAME][ tapped] types TYPES[ pt PT]` (spec section 7.1).
fn entering_suffix(entering: &Entering) -> String {
    let mut text = face_suffix(&entering.face);
    if entering.tapped {
        text.push_str(" tapped");
    }
    text.push_str(&permanent_types_suffix(&entering.types));
    text.push_str(&pt_suffix(entering.pt));
    text
}

fn step_text(step: Step) -> &'static str {
    match step {
        Step::Untap => "untap",
        Step::Upkeep => "upkeep",
        Step::Draw => "draw",
        Step::PrecombatMain => "precombat-main",
        Step::BeginningOfCombat => "beginning-of-combat",
        Step::DeclareAttackers => "declare-attackers",
        Step::DeclareBlockers => "declare-blockers",
        Step::CombatDamage => "combat-damage",
        Step::EndOfCombat => "end-of-combat",
        Step::PostcombatMain => "postcombat-main",
        Step::End => "end",
        Step::Cleanup => "cleanup",
    }
}

/// Mana symbols in the standard's order, `{W}{U}{B}{R}{G}{C}` (spec 9.4).
fn mana_text(mana: &[Mana]) -> String {
    let mut sorted = mana.to_vec();
    sorted.sort_by_key(|symbol| mana_rank(*symbol));
    let mut text = String::new();
    for symbol in &sorted {
        let letter = match symbol {
            Mana::W => 'W',
            Mana::U => 'U',
            Mana::B => 'B',
            Mana::R => 'R',
            Mana::G => 'G',
            Mana::C => 'C',
        };
        text.push_str(&format!("{{{letter}}}"));
    }
    text
}

fn mana_rank(symbol: Mana) -> u8 {
    match symbol {
        Mana::W => 0,
        Mana::U => 1,
        Mana::B => 2,
        Mana::R => 3,
        Mana::G => 4,
        Mana::C => 5,
    }
}

/// Each recorded choice as a stack snapshot repeats it, in order (spec
/// 9.9): ` x 3`, ` object #8 "Name"`, ` target X`.
fn push_choices(text: &mut String, choices: &[Choice]) {
    for choice in choices {
        let written = match choice {
            Choice::Target(target) => format!("target {}", target_text(target)),
            Choice::X(value) => format!("x {value}"),
            Choice::Object(object) => format!("object {}", obj_text(object)),
        };
        text.push(' ');
        text.push_str(&written);
    }
}

fn ability_kind_text(kind: AbilityKind) -> &'static str {
    match kind {
        AbilityKind::Activated => "activated",
        AbilityKind::Triggered => "triggered",
        AbilityKind::Loyalty => "loyalty",
        AbilityKind::Mana => "mana",
    }
}

fn action_text(action: MoveAction) -> &'static str {
    match action {
        MoveAction::Draw => "draw",
        MoveAction::Discard => "discard",
        MoveAction::Sacrifice => "sacrifice",
        MoveAction::Destroy => "destroy",
        MoveAction::Mill => "mill",
    }
}

fn ext_arg_text(arg: &ExtArg) -> String {
    match arg {
        ExtArg::Text(text) => quote(text),
        ExtArg::Int(number) => number.to_string(),
        ExtArg::Word(word) => {
            debug_assert!(is_keyword(word), "bad extension word: {word}");
            word.clone()
        }
    }
}

/// The statement without time, indent or cause.
fn statement_text(statement: &Statement) -> String {
    match statement {
        // Escape lines are written by `write_line` before this is reached.
        Statement::Escape(text) => format!("% {}", without_controls(text)),
        Statement::Deck {
            player,
            name,
            unknown,
        } => {
            let mut text = format!("deck {}", quote(player));
            if let Some(name) = name {
                text.push_str(&format!(" name {}", quote(name)));
            }
            if *unknown {
                text.push_str(" unknown");
            }
            text
        }
        Statement::DeckEntry { side, count, card } => {
            let section = if *side { "side" } else { "main" };
            match card {
                DeckCard::Named(name) => format!("{section} {count} {}", quote(name)),
                DeckCard::Unknown(source) => {
                    format!(
                        "{section} {count} ?{}",
                        sources_suffix(std::slice::from_ref(source))
                    )
                }
            }
        }
        Statement::Pregame => "pregame".to_string(),
        Statement::Keyframe => "keyframe".to_string(),
        Statement::KeyframePlayer {
            player,
            life,
            poison,
            library,
        } => {
            format!(
                "player {} life {life} poison {poison} library {library}",
                quote(player)
            )
        }
        Statement::KeyframeObject {
            object,
            zone,
            player,
            face,
            types,
            pt,
            tapped,
            attached,
            with,
            adventure,
            companion,
            counters,
            position,
        } => {
            let mut text = format!("{} {}", obj_text(object), zone_text(*zone));
            if let Some(player) = player {
                text.push(' ');
                text.push_str(&quote(player));
            }
            text.push_str(&face_suffix(face));
            if *zone == Zone::Battlefield {
                text.push_str(&permanent_types_suffix(types));
            } else {
                text.push_str(&types_suffix(types));
            }
            if let Some(pt) = pt {
                text.push_str(&format!(" pt {}", pt_value_text(*pt)));
            }
            if *tapped {
                text.push_str(" tapped");
            }
            if let Some(id) = attached {
                text.push_str(&format!(" attached #{id}"));
            }
            if let Some(id) = with {
                text.push_str(&format!(" with #{id}"));
            }
            if let Some(player) = adventure {
                text.push_str(&format!(" adventure {}", quote(player)));
            }
            if *companion {
                text.push_str(" companion");
            }
            for (name, count) in counters {
                text.push_str(&format!(" counters {} {count}", quote(name)));
            }
            text.push_str(&position_suffix(position));
            text
        }
        Statement::KeyframeEffect {
            id,
            of,
            text,
            controller,
        } => format!(
            "#{id} effect of {} {} controller {}",
            obj_text(of),
            text_or_unknown(text),
            quote(controller)
        ),
        Statement::Turn { number, player } => format!("turn {number} {}", quote(player)),
        Statement::Step(step) => step_text(*step).to_string(),
        Statement::Roll {
            player,
            sides,
            value,
        } => match sides {
            Some(sides) => format!("{} roll d{sides} {value}", quote(player)),
            None => format!("{} roll {value}", quote(player)),
        },
        Statement::ChooseFirst { player, first } => {
            format!("{} choose first {}", quote(player), quote(first))
        }
        Statement::Keep { player } => format!("{} keep", quote(player)),
        Statement::Mulligan { player } => format!("{} mulligan", quote(player)),
        Statement::Pass { player } => format!("{} pass", quote(player)),
        Statement::Concede { player } => format!("{} concede", quote(player)),
        Statement::Lose { player } => format!("{} lose", quote(player)),
        Statement::Shuffle { player } => format!("{} shuffle", quote(player)),
        Statement::Priority { player } => format!("{} priority", quote(player)),
        Statement::Stack => "stack".to_string(),
        Statement::StackEntry {
            object,
            face,
            controller,
            choices,
        } => {
            let mut text = format!(
                "{}{} controller {}",
                obj_text(object),
                face_suffix(face),
                quote(controller)
            );
            push_choices(&mut text, choices);
            text
        }
        Statement::StackAbility {
            id,
            kind,
            of,
            text,
            controller,
            choices,
        } => {
            let text = match text {
                Some(text) => quote(text),
                None => "?".to_string(),
            };
            let mut line = format!(
                "#{id} {} of {} {text} controller {}",
                ability_kind_text(*kind),
                obj_text(of),
                quote(controller)
            );
            push_choices(&mut line, choices);
            line
        }
        Statement::NewCard {
            object,
            owner,
            zone,
            position,
            entering,
            sources,
        } => {
            let mut text = format!(
                "new card #{} {} owner {} in {}{}",
                object.id,
                card_name_text(&object.name),
                quote(owner),
                zone_text(*zone),
                position_suffix(position)
            );
            if let Some(entering) = entering {
                text.push_str(&entering_suffix(entering));
            }
            text.push_str(&sources_suffix(sources));
            text
        }
        Statement::NewCopy {
            id,
            of,
            name,
            owner,
            zone,
            entering,
        } => {
            let mut text = format!(
                "new copy #{id} of {} {} owner {} in {}",
                obj_text(of),
                card_name_text(name),
                quote(owner),
                zone_text(*zone)
            );
            if let Some(entering) = entering {
                text.push_str(&entering_suffix(entering));
            }
            text
        }
        Statement::NewHidden {
            id,
            owner,
            zone,
            position,
        } => {
            format!(
                "new hidden #{id} owner {} in {}{}",
                quote(owner),
                zone_text(*zone),
                position_suffix(position)
            )
        }
        Statement::NewEffect {
            id,
            of,
            text,
            controller,
        } => format!(
            "new effect #{id} of {} {} controller {}",
            obj_text(of),
            text_or_unknown(text),
            quote(controller)
        ),
        Statement::NewToken {
            object,
            owner,
            zone,
            entering,
            sources,
        } => {
            let mut text = format!(
                "new token #{} {} owner {} in {}",
                object.id,
                card_name_text(&object.name),
                quote(owner),
                zone_text(*zone)
            );
            if *zone == Zone::Battlefield {
                text.push_str(&entering_suffix(entering));
            }
            text.push_str(&sources_suffix(sources));
            text
        }
        Statement::NewAbility {
            id,
            kind,
            of,
            text,
            controller,
            in_stack,
        } => {
            let mut line = format!(
                "new ability #{id} {} of {} {} controller {}",
                ability_kind_text(*kind),
                obj_text(of),
                text.as_deref()
                    .map(quote)
                    .unwrap_or_else(|| "?".to_string()),
                quote(controller)
            );
            if *in_stack {
                line.push_str(" in stack");
            }
            line
        }
        Statement::Play {
            object,
            from,
            entering,
        } => format!(
            "{} play from {}{}",
            obj_text(object),
            zone_text(*from),
            entering_suffix(entering)
        ),
        Statement::Cast { object, from, face } => {
            format!(
                "{} cast from {}{}",
                obj_text(object),
                zone_text(*from),
                face_suffix(face)
            )
        }
        Statement::Target { object, target } => {
            format!("{} target {}", obj_text(object), target_text(target))
        }
        Statement::ChooseX { object, value } => {
            format!("{} choose x {value}", obj_text(object))
        }
        Statement::ChooseObject { object, chosen } => {
            format!("{} choose object {}", obj_text(object), obj_text(chosen))
        }
        Statement::Move {
            object,
            from,
            to,
            action,
            position,
            entering,
            with,
        } => {
            let mut text = format!(
                "{} move {} -> {}",
                obj_text(object),
                zone_text(*from),
                zone_text(*to)
            );
            if let Some(action) = action {
                text.push_str(&format!(" as {}", action_text(*action)));
            }
            if let Some(place) = position {
                text.push(' ');
                text.push_str(&library_place_text(*place));
            }
            if let Some(entering) = entering {
                text.push_str(&entering_suffix(entering));
            }
            if let Some(id) = with {
                text.push_str(&format!(" with #{id}"));
            }
            text
        }
        Statement::Tap { object } => format!("{} tap", obj_text(object)),
        Statement::Untap { object } => format!("{} untap", obj_text(object)),
        Statement::Attack { object, target } => {
            format!("{} attack {}", obj_text(object), target_text(target))
        }
        Statement::Block { object, attacker } => {
            format!("{} block {}", obj_text(object), obj_text(attacker))
        }
        Statement::Damage {
            object,
            target,
            amount,
        } => format!(
            "{} damage {} {amount}",
            obj_text(object),
            target_text(target)
        ),
        Statement::Control { object, player } => {
            format!("{} control {}", obj_text(object), quote(player))
        }
        Statement::Counters {
            object,
            kind,
            from,
            to,
        } => format!(
            "{} counters {} {from} -> {to}",
            obj_text(object),
            quote(kind)
        ),
        Statement::PtChange { object, from, to } => format!(
            "{} pt {} -> {}",
            obj_text(object),
            pt_value_text(*from),
            pt_value_text(*to)
        ),
        Statement::TypesChange { object, from, to } => format!(
            "{} types {} -> {}",
            obj_text(object),
            types_text(from),
            types_text(to)
        ),
        Statement::Resolve { object } => format!("{} resolve", obj_text(object)),
        Statement::Transform { object } => format!("{} transform", obj_text(object)),
        Statement::DayNight { night } => {
            if *night {
                "night".to_string()
            } else {
                "day".to_string()
            }
        }
        Statement::Special { object, action } => {
            let action = match action {
                SpecialAction::Companion => "companion",
                SpecialAction::Suspend => "suspend",
                SpecialAction::Plot => "plot",
            };
            format!("{} special {action}", obj_text(object))
        }
        Statement::Incomplete { reason } => format!("incomplete {}", quote(reason)),
        Statement::Fizzle { object } => format!("{} fizzle", obj_text(object)),
        Statement::Countered { object } => format!("{} countered", obj_text(object)),
        Statement::Attach { object, to } => {
            format!("{} attach {}", obj_text(object), obj_text(to))
        }
        Statement::Unattach { object } => format!("{} unattach", obj_text(object)),
        Statement::Cease { object } => format!("{} cease", obj_text(object)),
        Statement::Identify { id, name, sources } => {
            format!(
                "#{id} identify {}{}",
                card_name_text(name),
                sources_suffix(sources)
            )
        }
        Statement::Reveal { object } => format!("{} reveal", obj_text(object)),
        Statement::Look { object, player } => {
            format!("{} look {}", obj_text(object), quote(player))
        }
        Statement::Untracked { object } => format!("{} untracked", obj_text(object)),
        Statement::Add {
            player,
            mana,
            unknown_restriction,
        } => {
            let mut text = format!("{} add {}", quote(player), mana_text(mana));
            if *unknown_restriction {
                text.push_str(" restriction ?");
            }
            text
        }
        Statement::Pay { player, mana } => format!("{} pay {}", quote(player), mana_text(mana)),
        Statement::LoseMana { player, mana } => {
            format!("{} lose-mana {}", quote(player), mana_text(mana))
        }
        Statement::Life { player, from, to } => {
            format!("{} life {from} -> {to}", quote(player))
        }
        Statement::Poison { player, from, to } => {
            format!("{} poison {from} -> {to}", quote(player))
        }
        Statement::Extension { name, args } => {
            debug_assert!(is_extension_name(name), "bad extension name: {name}");
            let mut text = name.clone();
            for arg in args {
                text.push(' ');
                text.push_str(&ext_arg_text(arg));
            }
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::tag;

    #[test]
    fn a_repeated_tag_name_is_written_once_with_the_first_value() {
        let tags = vec![
            tag("Zed", "one"),
            tag("Event", "first"),
            tag("Zed", "two"),
            tag("Event", "second"),
        ];
        let ordered = ordered_tags(&tags);
        let written: Vec<(&str, &str)> = ordered
            .iter()
            .map(|tag| (tag.name.as_str(), tag.value.as_str()))
            .collect();
        assert_eq!(written, vec![("Event", "first"), ("Zed", "one")]);
    }
}
