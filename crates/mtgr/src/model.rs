//! The typed model of a `.mtgr` file.

/// A whole file: one or more games.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record {
    pub games: Vec<Game>,
}

/// One game: tags, body lines and the result marker.
#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    /// Tags in any order; the writer puts them in canonical order.
    pub tags: Vec<Tag>,
    pub lines: Vec<Line>,
    pub result: GameResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    pub value: String,
}

/// The result marker, written `1-0`, `0-1`, `1/2-1/2` or `*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameResult {
    Player1,
    Player2,
    Draw,
    Unknown,
}

/// One body line: elapsed time, nesting level (two spaces each), the
/// statement and an optional cause.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Milliseconds since the game started. Written as `M:SS` (whole seconds).
    pub time_ms: i64,
    pub level: u8,
    pub statement: Statement,
    pub cause: Option<Cause>,
}

/// Why a statement happened, written as ` by ...` after the statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Cause {
    /// `by #N`
    Object(u32),
    /// `by cost:#N`
    Cost(u32),
    /// `by "Player"`
    Player(String),
    /// `by rule:name`
    Rule(String),
    /// `by ?`
    Unknown,
}

/// The card of a deck entry: a name, or `?` with the card's one source id
/// (`main 4 ? src arena:106504`, decision 0048).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeckCard {
    Named(String),
    Unknown(String),
}

/// An object reference: an instance id and, when it is known, its name.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjRef {
    pub id: u32,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Library,
    Hand,
    Graveyard,
    Battlefield,
    Stack,
    Exile,
    Command,
    Ante,
    /// `outside "sideboard"`: a card in its owner's sideboard, outside the
    /// game (CR 100.4, 400.11), such as a companion.
    OutsideSideboard,
}

/// Card types in the order the standard writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardType {
    Artifact,
    Battle,
    Creature,
    Enchantment,
    Kindred,
    Land,
    Planeswalker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Untap,
    Upkeep,
    Draw,
    PrecombatMain,
    BeginningOfCombat,
    DeclareAttackers,
    DeclareBlockers,
    CombatDamage,
    EndOfCombat,
    PostcombatMain,
    End,
    Cleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mana {
    W,
    U,
    B,
    R,
    G,
    C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityKind {
    Activated,
    Triggered,
    Loyalty,
    Mana,
}

/// A special action taken with a card (`special ACTION`, decision 0025).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialAction {
    Companion,
    Suspend,
    Plot,
}

/// The reason a card moved, written after `as`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveAction {
    Draw,
    Discard,
    Sacrifice,
    Destroy,
    Mill,
}

/// A power and toughness value in a `pt` change (`PTVALUE`, spec section
/// 7.5): a creature's `P/T`, `none` for a permanent that is not a
/// creature, or `?` when the source does not report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtValue {
    Pt(i32, i32),
    None,
    Unknown,
}

/// What a `target` points at: an object or a player.
#[derive(Debug, Clone, PartialEq)]
pub enum TargetRef {
    Object(ObjRef),
    Player(String),
}

/// A recorded choice as a stack snapshot repeats it (spec 9.9): what was
/// written after `choose`, or a `target`.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// `target #5 "Name"` or `target "P"`.
    Target(TargetRef),
    /// `x 3`: the value of X (CR 107.3).
    X(u32),
    /// `object #8 "Name"`: an object chosen without targeting.
    Object(ObjRef),
}

/// How a permanent enters the battlefield (`ENTERING`, spec section 7.1):
/// `[face NAME] [tapped] [types TYPES] [pt PT]`. Empty `types` are written
/// `types ?`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entering {
    /// The face it enters with, when it is not the front face (decision
    /// 0022).
    pub face: Option<String>,
    pub tapped: bool,
    pub types: Vec<CardType>,
    /// Power and toughness, for a creature.
    pub pt: Option<(i32, i32)>,
}

/// The end of a library a card is put on (`top` or `bottom`, spec 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryEnd {
    Top,
    Bottom,
}

/// Where a move puts an object in a library (spec 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryPlace {
    /// `top`: on top.
    Top,
    /// `top N`, N of 2 or more: Nth from the top (decision 0045). The
    /// library holds at least N other cards; with fewer the card goes on
    /// the bottom (CR 401.7), written `Bottom`.
    FromTop(u32),
    /// `bottom`: on the bottom.
    Bottom,
    /// `bottom ?`: on the bottom, one of two or more cards put there
    /// together in a random order, so its place is unknown (decision 0047).
    RandomBottom,
}

/// A card's position in a library, counted from one end (`top 1` is the
/// top card; spec 6.5, 10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub end: LibraryEnd,
    pub n: u32,
}

/// One argument of an extension statement.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtArg {
    /// Written as a quoted string.
    Text(String),
    Int(u32),
    /// A lowercase keyword such as `hand`.
    Word(String),
}

/// A body statement. Later tasks add variants here and an arm in the writer.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// `% text`: an escape line, written without time or indent.
    Escape(String),
    /// `deck "P" [name "N"] [unknown]`
    Deck {
        player: String,
        name: Option<String>,
        unknown: bool,
    },
    /// `main N "Name"` or `side N ? src arena:1` under a `deck`. The caller
    /// gives the entries in order: named ones first by name, then unknown
    /// ones by source id (decision 0048).
    DeckEntry {
        side: bool,
        count: u32,
        card: DeckCard,
    },
    Pregame,
    Keyframe,
    /// `player "P" life L poison N library N`
    KeyframePlayer {
        player: String,
        life: i32,
        poison: u32,
        library: u32,
    },
    /// `#1 "Swamp" battlefield "P" types land tapped`. `player` is `None` where
    /// the standard omits it (stack, exile, command, ante). `face` is the
    /// face up when it is not the front (decision 0022). On the
    /// battlefield, empty `types` are written `types ?`; `pt` is written for
    /// a creature (`?` when the source does not show it).
    KeyframeObject {
        object: ObjRef,
        zone: Zone,
        player: Option<String>,
        face: Option<String>,
        types: Vec<CardType>,
        pt: Option<PtValue>,
        tapped: bool,
        /// `attached ID`: the object it is attached to.
        attached: Option<u32>,
        /// `with ID`: the object a card in exile is exiled with, while the
        /// link holds (decision 0024).
        with: Option<u32>,
        /// `adventure PLAYER`: a card in exile on an adventure, with the
        /// player who may cast it from there (CR 715.3d).
        adventure: Option<String>,
        /// `companion`: a revealed companion still outside the game.
        companion: bool,
        /// `counters TEXT INTEGER` entries, each name once with a count
        /// above zero, in code point order of the name.
        counters: Vec<(String, u32)>,
        /// `top N` or `bottom N`: a library position, when it is known.
        position: Option<Position>,
    },
    /// `#18 effect of #6 "Name" ? controller "P"`: a lasting effect still
    /// in force (spec 10.3). A `None` text is written `?` (decision 0044).
    KeyframeEffect {
        id: u32,
        of: ObjRef,
        text: Option<String>,
        controller: String,
    },
    Turn {
        number: u32,
        player: String,
    },
    Step(Step),
    /// `"P" roll d6 5`, or `"P" roll 5` when the sides are not known.
    Roll {
        player: String,
        sides: Option<u32>,
        value: u32,
    },
    /// `"P" choose first "Q"`
    ChooseFirst {
        player: String,
        first: String,
    },
    Keep {
        player: String,
    },
    Mulligan {
        player: String,
    },
    Pass {
        player: String,
    },
    Concede {
        player: String,
    },
    /// `"P" lose`: the player loses the game (cause required).
    Lose {
        player: String,
    },
    /// `"P" shuffle`: the player's library is shuffled (cause required).
    Shuffle {
        player: String,
    },
    Priority {
        player: String,
    },
    /// `stack`: heads the stack snapshot under a `priority` line.
    Stack,
    /// `#2 "Name" [face "Half"] controller "P" [x 3] [target ...]...` under
    /// `stack`: a spell, the face or half it was cast as, and its recorded
    /// choices, in the order they were written.
    StackEntry {
        object: ObjRef,
        face: Option<String>,
        controller: String,
        choices: Vec<Choice>,
    },
    /// `#23 triggered of #5 "Name" ? controller "P" [target ...]...` under
    /// `stack`: an ability and its recorded choices. A `None` text is
    /// written `?`.
    StackAbility {
        id: u32,
        kind: AbilityKind,
        of: ObjRef,
        text: Option<String>,
        controller: String,
        choices: Vec<Choice>,
    },
    /// `new card #1 "Swamp" owner "P" in library [top 1]`; `position` only
    /// in a library, `entering` only in the battlefield (a card introduced
    /// there after `incomplete`, spec 5.3). An object with no name is
    /// written `?`: a card whose name the writer cannot determine (decision
    /// 0048), which then carries its source id.
    NewCard {
        object: ObjRef,
        owner: String,
        zone: Zone,
        position: Option<Position>,
        entering: Option<Entering>,
        /// The source's own ids for the card (`src arena:75000`, spec 6.4),
        /// written after everything else; optional metadata.
        sources: Vec<String>,
    },
    /// `new copy #65 of #63 "Name" "Name" owner "P" in stack`: a copy of a
    /// card or spell (cause required, decision 0023); `entering` only in
    /// the battlefield. A `None` name (the copy's own) is written `?`
    /// (decision 0048).
    NewCopy {
        id: u32,
        of: ObjRef,
        name: Option<String>,
        owner: String,
        zone: Zone,
        entering: Option<Entering>,
    },
    /// `new hidden #8 owner "P" in library [top 1]`
    NewHidden {
        id: u32,
        owner: String,
        zone: Zone,
        position: Option<Position>,
    },
    /// `new token #22 "Treasure" owner "P" in battlefield types artifact`
    /// (cause required). An object with no name is written `?` (decision
    /// 0048).
    NewToken {
        object: ObjRef,
        owner: String,
        zone: Zone,
        entering: Entering,
        /// The source's own ids for the token (spec 6.4), written after
        /// `entering`.
        sources: Vec<String>,
    },
    /// `new effect #18 of #6 "Name" ? controller "P"`: a lasting effect
    /// (cause required, spec 9.10). A `None` text is written `?` (decision
    /// 0044).
    NewEffect {
        id: u32,
        of: ObjRef,
        text: Option<String>,
        controller: String,
    },
    /// `new ability #15 mana of #1 "Swamp" "text" controller "P" [in stack]`.
    /// A `None` text is written `?` (the source does not report it).
    NewAbility {
        id: u32,
        kind: AbilityKind,
        of: ObjRef,
        text: Option<String>,
        controller: String,
        in_stack: bool,
    },
    /// `#1 "Swamp" play from hand types land`
    Play {
        object: ObjRef,
        from: Zone,
        entering: Entering,
    },
    /// `#80 "Fire // Ice" cast from hand [face "Ice"]`: `face` for a card
    /// cast as one face or half (decision 0022).
    Cast {
        object: ObjRef,
        from: Zone,
        face: Option<String>,
    },
    Target {
        object: ObjRef,
        target: TargetRef,
    },
    /// `#2 "Name" choose x 3`: the value of X (CR 107.3).
    ChooseX {
        object: ObjRef,
        value: u32,
    },
    /// `#2 "Name" choose object #8 "Other"`
    ChooseObject {
        object: ObjRef,
        chosen: ObjRef,
    },
    /// `#1 "X" move hand -> graveyard as discard`; `position` (`top`,
    /// `top N`, `bottom` or `bottom ?`) only on a move to a library,
    /// `entering` only on a move to the battlefield, `with` (`with #6`)
    /// only on a move to exile: the object the card is exiled with (CR
    /// 607.2a).
    Move {
        object: ObjRef,
        from: Zone,
        to: Zone,
        action: Option<MoveAction>,
        position: Option<LibraryPlace>,
        entering: Option<Entering>,
        with: Option<u32>,
    },
    Tap {
        object: ObjRef,
    },
    Untap {
        object: ObjRef,
    },
    /// `#5 attack "P"` or `#5 attack #9 "Planeswalker"`.
    Attack {
        object: ObjRef,
        target: TargetRef,
    },
    /// `#6 block #5 "Attacker"`.
    Block {
        object: ObjRef,
        attacker: ObjRef,
    },
    /// `#5 damage "P" 3` (cause required).
    Damage {
        object: ObjRef,
        target: TargetRef,
        amount: u32,
    },
    /// `#5 "Name" control "P"`: the object's controller changes (cause
    /// required).
    Control {
        object: ObjRef,
        player: String,
    },
    /// `#5 counters "+1/+1" 0 -> 1` (cause required).
    Counters {
        object: ObjRef,
        kind: String,
        from: u32,
        to: u32,
    },
    /// `#5 pt 2/2 -> 3/2` (cause required).
    PtChange {
        object: ObjRef,
        from: PtValue,
        to: PtValue,
    },
    /// `#5 types land -> creature land` (cause required); empty lists are
    /// written `?`.
    TypesChange {
        object: ObjRef,
        from: Vec<CardType>,
        to: Vec<CardType>,
    },
    Resolve {
        object: ObjRef,
    },
    /// `#66 "Front // Back" transform`: a double-faced permanent turns over
    /// (cause required, decision 0022).
    Transform {
        object: ObjRef,
    },
    /// `day` or `night`: the game becomes day or night (cause required, CR
    /// 731); with no cause, a keyframe entry.
    DayNight {
        night: bool,
    },
    /// `#1 "Lurrus" special companion` (no cause).
    Special {
        object: ObjRef,
        action: SpecialAction,
    },
    /// `incomplete "reason"`: events from here to the next keyframe may be
    /// missing (spec section 9.8).
    Incomplete {
        reason: String,
    },
    /// `#9 fizzle`: the spell or ability does not resolve because every
    /// target is illegal (CR 608.2b; cause optional).
    Fizzle {
        object: ObjRef,
    },
    /// `#9 countered`: the spell or ability is countered (cause required).
    Countered {
        object: ObjRef,
    },
    /// `#12 attach #5 "Bear"`: the object becomes attached to another
    /// (cause required).
    Attach {
        object: ObjRef,
        to: ObjRef,
    },
    /// `#12 unattach`: the object becomes unattached (cause required).
    Unattach {
        object: ObjRef,
    },
    /// `#15 cease`: the object ceases to exist and its id ends (cause
    /// required).
    Cease {
        object: ObjRef,
    },
    /// `#8 identify "Name" [src arena:75000]`: names a hidden object. A
    /// `None` name is written `?` (decision 0048).
    Identify {
        id: u32,
        name: Option<String>,
        /// The source's own ids for the card (spec 6.4, 6.6).
        sources: Vec<String>,
    },
    Reveal {
        object: ObjRef,
    },
    /// `#17 "Name" look "P"`: the player looks at the object (cause
    /// required).
    Look {
        object: ObjRef,
        player: String,
    },
    /// `#9 untracked`: the object can no longer be told apart from the
    /// other cards in its zone; its id ends (no cause).
    Untracked {
        object: ObjRef,
    },
    /// `"P" add {B} [restriction ?]` (cause required). With
    /// `unknown_restriction` the mana may carry a spending restriction the
    /// source does not report (decision 0042).
    Add {
        player: String,
        mana: Vec<Mana>,
        unknown_restriction: bool,
    },
    /// `"P" pay {B}`
    Pay {
        player: String,
        mana: Vec<Mana>,
    },
    /// `"P" lose-mana {G}`: unspent mana is lost as the pool empties (cause
    /// required).
    LoseMana {
        player: String,
        mana: Vec<Mana>,
    },
    /// `"P" life 20 -> 18`
    Life {
        player: String,
        from: i32,
        to: i32,
    },
    /// `"P" poison 0 -> 3` (cause required)
    Poison {
        player: String,
        from: u32,
        to: u32,
    },
    /// An extension statement such as `MYMTGO_seen "P" 3 hand`.
    Extension {
        name: String,
        args: Vec<ExtArg>,
    },
}
