//! The deck Card Stats tab's reads (cards spec section 9, 0.x
//! `GetCardGameStats` and `GetFilteredDeckWinrate`) and the "Regenerate"
//! write (cards spec section 8).
//!
//! The matches read are a deck's listed (not hidden) complete matches of any
//! source, started in the timeframe, through the version they link to; their
//! games' `game_cards` rows are grouped by oracle id, or by catalog id for a
//! card without card data (shown as "Card {id}", spec ruling 5).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use archetypes::guides::{Postboard, SidedStat};

use cards::stats::{Board, CardStat, stat_key};
use decks::stats::Range;
use game_detail::Side;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::deck_pages::existing_version;
use crate::platform::BestOf;

/// What a card stats read counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardStatsFilter {
    /// One version of the deck, or every version. A version id with no row
    /// means every version (as the deck pages read it).
    pub version_id: Option<i64>,
    /// Matches started in this window (spec ruling 4).
    pub timeframe: Range,
    /// Game 1 or postboard games; `None` for both.
    pub board: Option<Board>,
    /// Games the local player was on the play (`true`) or the draw.
    pub on_play: Option<bool>,
    /// Games against this opponent archetype: the match's opponent seat
    /// (`match_archetypes`, `is_opponent = 1`) holds it (0.x
    /// `ApplyOpponentArchetypeFilter`).
    pub opponent_archetype_id: Option<i64>,
    /// The local player's cards or the opponent's.
    pub side: Side,
    /// Only the matches of this Arena kind; `None` reads every match (an
    /// MTGO read passes none).
    pub best_of: Option<BestOf>,
}

impl CardStatsFilter {
    /// Every game of every version, all time, the local player's cards.
    pub fn all() -> CardStatsFilter {
        CardStatsFilter {
            version_id: None,
            timeframe: Range::all_time(),
            board: None,
            on_play: None,
            opponent_archetype_id: None,
            side: Side::Local,
            best_of: None,
        }
    }
}

/// The deck's game win rate under the same filters: the prior card win
/// rates are pulled toward (0.x `DeckWinrateData`). `rate` is 0.5 without
/// games.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckWinrate {
    pub wins: u32,
    pub games: u32,
    pub rate: f64,
}

/// One opponent archetype for the filter (0.x `availableArchetypes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeOption {
    pub id: i64,
    pub name: String,
    pub color_identity: Option<String>,
}

/// What "Regenerate" did for a deck.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Regenerated {
    /// The deck's complete log matches with games, to sync again.
    pub log_tokens: Vec<String>,
    /// Imported and history matches with games the imported rule was
    /// applied to again.
    pub rebuilt: u32,
}

impl Regenerated {
    /// The matches regenerated, as the toast counts them (0.x `queued`).
    pub fn count(&self) -> u32 {
        self.log_tokens.len() as u32 + self.rebuilt
    }
}

/// Whose matches a card stats read counts: one deck's (the Card Stats tab)
/// or the versions of an archetype's decks (the deck index's archetype Card
/// stats tab, 0.x `GetCardGameStats::forVersionIds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `?1` is the deck id, `?2` the version (or NULL for all).
    Deck,
    /// `?1` is the versions as a JSON array, `?2` NULL.
    Versions,
}

impl Target {
    /// The condition on the version `v`.
    fn condition(self) -> &'static str {
        match self {
            Target::Deck => "v.deck_id = ?1 AND (?2 IS NULL OR v.id = ?2)",
            Target::Versions => {
                "v.id IN (SELECT value FROM json_each(?1)) AND (?2 IS NULL OR v.id = ?2)"
            }
        }
    }
}

/// The rows a card stats read walks: one side's `game_cards` rows (as `gc`,
/// or `game_card_casts` under the same alias) of the target's listed
/// complete matches (`?1`, `?2`: `Target`), started from `?3` to `?4`, side
/// `?5`, postboard `?6` (or both), local on the play `?7` (or both),
/// against opponent archetype `?8` (or any) and of kind `?9` (or any).
fn rows_from(target: Target, rows_table: &str, deck_rows_only: bool) -> String {
    let deck_rows = if deck_rows_only {
        format!("\n  AND ({} OR {})", deck_row("gc"), other_row("gc"))
    } else {
        String::new()
    };
    let against = against_archetype("?8");
    format!(
        "FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
JOIN games g ON g.match_id = m.id
JOIN {rows_table} gc ON gc.game_id = g.id
WHERE {target}
  AND m.state = 'complete' AND m.started_at BETWEEN ?3 AND ?4
  AND gc.side = ?5
  AND (?6 IS NULL OR (g.game_number > 1) = ?6)
  AND (?7 IS NULL OR g.on_play = ?7)
  AND (?9 IS NULL OR m.game_structure = ?9)
  AND {against}{deck_rows}",
        target = target.condition()
    )
}

/// The match `m` was played against the opponent archetype in parameter
/// `param`, or the parameter is NULL (0.x `ApplyOpponentArchetypeFilter`:
/// the opponent seat's row, never the player's own).
fn against_archetype(param: &str) -> String {
    format!(
        "({param} IS NULL OR EXISTS (SELECT 1 FROM match_archetypes ma
     WHERE ma.match_id = m.id AND ma.is_opponent = 1 AND ma.archetype_id = {param}))"
    )
}

/// The local `game_cards` rows 0.x's `card_game_stats` would have had (the
/// S8 review): a maindeck card, or in a postboard game a sideboard card, a
/// card sided in or out, and (for matches not from the log) 0.x's own
/// zero-quantity row with no counters. Being inferred is not enough: the
/// imported rule marks every local row of a postboard game inferred, tokens
/// included (S8-7). v1 also stores game 1
/// sideboard rows and tokens or other cards the local player never had in
/// the deck, which 0.x never counted. `alias` is the `game_cards` alias,
/// `g` the game and `m` the match; the opponent's rows are all kept. The
/// card stats shipping read (`api_reads`) shares it (issue #295).
pub(crate) fn deck_row(alias: &str) -> String {
    format!(
        "({a}.side <> 'local' OR {a}.main_qty > 0
     OR (g.game_number > 1 AND ({a}.side_qty > 0 OR {a}.sided_in > 0 OR {a}.sided_out > 0
         OR (m.source <> 'log' AND {a}.seen = 0 AND {a}.cast = 0 AND COALESCE({a}.played, 0) = 0))))",
        a = alias
    )
}

/// A local row with no deck quantity that may fold into a deck card that
/// game (0.x widened the deck's catalog to oracle map with every printing
/// the player touched):
/// - a variant (issue #319): its oracle is a card with a deck quantity
///   that game (MTGO gives some variants their own catalog id, as warp
///   casts are logged), so its counters land on the deck card's oracle
///   row; a card not in the deck that game (sided out, say) got none;
/// - maybe a face (issue #320): a multi-face card logs its face's actions
///   under the face's own catalog id, whose `cards` row is a stub named by
///   the log with no oracle id. Whether it is a face, and of a card in the
///   deck that game, is decided in Rust (`face_oracle`, then the game's
///   deck quantity), as 0.x's `resolveFaceNamesToOracle` decided it.
///
/// Read with the card data of now, so a card resolved later reads as one
/// known when its game was synced (live equals rebuild).
pub(crate) fn other_row(alias: &str) -> String {
    format!(
        "({a}.side = 'local' AND {a}.main_qty = 0 AND EXISTS (
     SELECT 1 FROM cards va
     WHERE va.catalog_id = {a}.catalog_id
       AND ((va.oracle_id IS NULL AND va.name <> '')
         OR (va.oracle_id IS NOT NULL AND EXISTS (
             SELECT 1 FROM cards vb
             JOIN game_cards vd ON vd.game_id = {a}.game_id AND vd.side = 'local'
                               AND vd.catalog_id = vb.catalog_id
             WHERE vb.oracle_id = va.oracle_id AND vd.main_qty > 0)))))",
        a = alias
    )
}

/// 0.x's `resolveFaceNamesToOracle`: the oracle id of a catalog id with no
/// card data. Its `cards` row is a stub with no oracle id, named by the log
/// (the face's name), and a card with an oracle id has a face of that name
/// (its name split on ` // `, a single-face card's whole name). The first
/// such card in catalog id order wins, with its catalog id (the printing to
/// show). Case-sensitive, as 0.x compared the faces.
///
/// The face map is built once, at the first id to resolve, from one read of
/// `cards`; a read with no oracle-less id to resolve never builds it.
#[derive(Default)]
pub(crate) struct FaceNames {
    by_face: Option<HashMap<String, (String, u32)>>,
}

impl FaceNames {
    pub(crate) fn new() -> FaceNames {
        FaceNames::default()
    }

    /// `catalog_id`'s oracle id and shown printing by its face name. `None`
    /// for a row with an oracle id, no row, no name or no match.
    pub(crate) fn oracle(
        &mut self,
        conn: &Connection,
        catalog_id: u32,
    ) -> Result<Option<(String, u32)>, StoreError> {
        let name: Option<String> = conn
            .prepare_cached("SELECT name FROM cards WHERE catalog_id = ?1 AND oracle_id IS NULL")?
            .query_row([catalog_id], |row| row.get(0))
            .optional()?
            .flatten();
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            return Ok(None);
        };
        if self.by_face.is_none() {
            self.by_face = Some(face_map(conn)?);
        }
        Ok(self
            .by_face
            .as_ref()
            .and_then(|map| map.get(&name))
            .cloned())
    }
}

/// Every face name of the cards with an oracle id, to the first such card
/// in catalog id order.
fn face_map(conn: &Connection) -> Result<HashMap<String, (String, u32)>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT catalog_id, name, oracle_id FROM cards
         WHERE oracle_id IS NOT NULL AND oracle_id <> '' AND name IS NOT NULL
         ORDER BY catalog_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, u32>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (id, name, oracle) = row?;
        for face in name.split(" // ") {
            map.entry(face.trim().to_string())
                .or_insert_with(|| (oracle.clone(), id));
        }
    }
    Ok(map)
}

/// Every row read, with its game's result and board. The rows are folded
/// in Rust rather than grouped in SQL: SQLite's two grouping passes over
/// 100,000 rows took about 150 ms, over the tab's 100 ms budget (cards spec
/// section 12).
fn rows_sql(target: Target) -> String {
    format!(
        "SELECT g.id, g.result = 'win', g.game_number > 1, gc.catalog_id,
       gc.main_qty, gc.starting_hand, gc.seen, gc.cast,
       COALESCE(gc.played, 0), COALESCE(gc.activated, 0),
       gc.sided_in > 0, gc.sided_out > 0, gc.pregame_revealed > 0, gc.pregame_played > 0,
       gc.inferred > 0, NOT {}
{}",
        deck_row("gc"),
        rows_from(target, "game_cards", true)
    )
}

/// Casts by an alternative method per catalog id, of deck rows.
fn casts_sql(target: Target) -> String {
    format!(
        "SELECT gc.catalog_id, gc.method, SUM(gc.count) {}
  AND EXISTS (SELECT 1 FROM game_cards d
              WHERE d.game_id = gc.game_id AND d.side = gc.side AND d.catalog_id = gc.catalog_id
                AND {})
GROUP BY 1, 2",
        rows_from(target, "game_card_casts", false),
        deck_row("d")
    )
}

/// Casts by an alternative method of the rows that are not deck rows (a
/// variant or a face), per game and catalog id: each is folded only into a
/// card in the deck that game.
fn other_casts_sql(target: Target) -> String {
    format!(
        "SELECT gc.game_id, gc.catalog_id, gc.method, gc.count {}
  AND EXISTS (SELECT 1 FROM game_cards d
              WHERE d.game_id = gc.game_id AND d.side = gc.side AND d.catalog_id = gc.catalog_id
                AND NOT {} AND {})",
        rows_from(target, "game_card_casts", false),
        deck_row("d"),
        other_row("d")
    )
}

/// The distinct local games under the filters: the opponent's cards'
/// denominator (their rows exist only when a signal fired).
fn local_games_sql(target: Target) -> String {
    format!(
        "SELECT COUNT(DISTINCT g.id) {}",
        rows_from(target, "game_cards", false)
    )
}

/// The parameters of `rows_from`, `?1` to `?9` in order (`?1` is the
/// target's subject: a deck id or a JSON array of versions).
type RowsParams = (
    Value,
    Option<i64>,
    i64,
    i64,
    &'static str,
    Option<bool>,
    Option<bool>,
    Option<i64>,
    Option<BestOf>,
);

/// The parameters of `rows_from` for `filter` on `side`.
fn bound(
    subject: &Value,
    version: Option<i64>,
    filter: &CardStatsFilter,
    side: Side,
) -> RowsParams {
    let postboard = filter.board.map(|board| board == Board::Postboard);
    (
        subject.clone(),
        version,
        filter.timeframe.from_ms,
        filter.timeframe.to_ms,
        side.as_str(),
        postboard,
        filter.on_play,
        filter.opponent_archetype_id,
        filter.best_of,
    )
}

/// A count read from SQL as a `u32` (never negative, sums of small counts).
fn count(row: &rusqlite::Row, index: usize) -> rusqlite::Result<u32> {
    let value: Option<i64> = row.get(index)?;
    Ok(u32::try_from(value.unwrap_or(0).max(0)).unwrap_or(u32::MAX))
}

/// One card's rows in one game, printings of the card added together (0.x
/// has one row per oracle id per game).
#[derive(Debug, Clone, Copy, Default)]
struct GameCard {
    won: bool,
    postboard: bool,
    quantity: u32,
    kept: u32,
    seen: u32,
    cast: u32,
    played: u32,
    activated: u32,
    sided_in: bool,
    sided_out: bool,
    revealed: bool,
    pregame_played: bool,
    inferred: bool,
    /// The highest catalog id among the printings.
    top: u32,
    /// The most copies a variant row (`other_row`) saw: the same
    /// instances as the deck card's under another catalog id, so they are
    /// not added to `seen`.
    variant_seen: u32,
    /// A variant row was folded in.
    folded: bool,
}

impl GameCard {
    fn add(&mut self, row: &GameCard) {
        self.won |= row.won;
        self.postboard |= row.postboard;
        self.quantity += row.quantity;
        self.kept += row.kept;
        self.seen += row.seen;
        self.cast += row.cast;
        self.played += row.played;
        self.activated += row.activated;
        self.sided_in |= row.sided_in;
        self.sided_out |= row.sided_out;
        self.revealed |= row.revealed;
        self.pregame_played |= row.pregame_played;
        self.inferred |= row.inferred;
        self.top = self.top.max(row.top);
    }

    /// Fold in a variant row (issue #319): its log counters and kept copies
    /// add, its copies seen do not (see `variant_seen`), and it never sets
    /// the printing shown.
    fn add_variant(&mut self, row: &GameCard) {
        self.won |= row.won;
        self.postboard |= row.postboard;
        self.kept += row.kept;
        self.cast += row.cast;
        self.played += row.played;
        self.activated += row.activated;
        self.revealed |= row.revealed;
        self.pregame_played |= row.pregame_played;
        self.inferred |= row.inferred;
        self.variant_seen = self.variant_seen.max(row.seen);
        self.folded = true;
    }

    /// The game's card once every row is in: with a variant folded, seen is
    /// the larger of the two and seen and kept are capped at the copies in
    /// the deck, as 0.x capped them per oracle.
    fn finished(&self) -> GameCard {
        let mut card = *self;
        if card.folded {
            card.seen = card.seen.max(card.variant_seen);
            card.seen = card.seen.min(card.quantity);
            card.kept = card.kept.min(card.quantity);
        }
        card
    }
}

/// Count one game of a card (0.x's per-row sums; a game is won only when
/// its result is a win, so a draw counts as lost).
fn add_game(stat: &mut CardStat, game: &GameCard) {
    let won = u32::from(game.won);
    let lost = u32::from(!game.won);
    stat.total_games += 1;
    stat.total_possible += game.quantity;
    stat.total_kept += game.kept;
    if game.kept > 0 {
        stat.kept_games += 1;
        stat.kept_won += won;
        stat.kept_lost += lost;
    }
    stat.total_seen += game.seen;
    if game.seen > 0 {
        stat.seen_games += 1;
        stat.seen_won += won;
        stat.seen_lost += lost;
    }
    stat.total_cast += game.cast;
    if game.cast > 0 {
        stat.cast_games += 1;
        stat.cast_won += won;
        stat.cast_lost += lost;
    }
    stat.postboard_games += u32::from(game.postboard);
    stat.sided_out_games += u32::from(game.sided_out);
    stat.sided_in_games += u32::from(game.sided_in);
    stat.total_played += game.played;
    stat.played_games += u32::from(game.played > 0);
    stat.total_activated += game.activated;
    stat.pregame_revealed_games += u32::from(game.revealed);
    stat.pregame_played_games += u32::from(game.pregame_played);
    if game.revealed || game.pregame_played {
        stat.pregame_games += 1;
        stat.pregame_won += won;
        stat.pregame_lost += lost;
    }
    stat.inferred_games += u32::from(game.inferred);
    stat.catalog_id = stat.catalog_id.max(game.top);
}

/// The card keys (`cards::stats::stat_key`) of catalog ids, each looked up
/// once. A face logged under its own catalog id (`FaceNames`, issue #320)
/// takes its card's oracle key where 0.x resolved faces: the local side's
/// rows that are not deck rows (0.x's deck quantities came only from deck
/// cards with card data) and every opponent row.
struct Keys {
    by_catalog: HashMap<(u32, bool), usize>,
    keys: Vec<String>,
    by_key: HashMap<String, usize>,
    /// A face's catalog id to its card's (the printing shown).
    faces: HashMap<u32, u32>,
    names: FaceNames,
}

impl Keys {
    fn new() -> Keys {
        Keys {
            by_catalog: HashMap::new(),
            keys: Vec::new(),
            by_key: HashMap::new(),
            faces: HashMap::new(),
            names: FaceNames::new(),
        }
    }

    /// The printing to show for `catalog_id`: its card's for a face, else
    /// itself. Read after `index`.
    fn shown(&self, catalog_id: u32) -> u32 {
        self.faces.get(&catalog_id).copied().unwrap_or(catalog_id)
    }

    /// The index of `catalog_id`'s key; `faces` resolves an oracle-less id
    /// by its face name.
    fn index(
        &mut self,
        conn: &Connection,
        catalog_id: u32,
        faces: bool,
    ) -> Result<usize, StoreError> {
        if let Some(index) = self.by_catalog.get(&(catalog_id, faces)) {
            return Ok(*index);
        }
        let mut oracle: Option<String> = conn
            .prepare_cached("SELECT oracle_id FROM cards WHERE catalog_id = ?1")?
            .query_row([catalog_id], |row| row.get(0))
            .optional()?
            .flatten();
        if oracle.is_none()
            && faces
            && let Some((face_oracle, card)) = self.names.oracle(conn, catalog_id)?
        {
            self.faces.insert(catalog_id, card);
            oracle = Some(face_oracle);
        }
        let key = stat_key(oracle.as_deref(), catalog_id);
        let index = match self.by_key.get(&key) {
            Some(index) => *index,
            None => {
                self.keys.push(key.clone());
                self.by_key.insert(key, self.keys.len() - 1);
                self.keys.len() - 1
            }
        };
        self.by_catalog.insert((catalog_id, faces), index);
        Ok(index)
    }
}

/// A deck's card stats under `filter`, ordered by type, then name (0.x).
pub fn card_stats(
    conn: &Connection,
    deck_id: i64,
    filter: &CardStatsFilter,
) -> Result<Vec<CardStat>, StoreError> {
    let version = existing_version(conn, filter.version_id)?;
    let sideboard = match filter.side {
        Side::Opponent => BTreeSet::new(),
        Side::Local => match sideboard_version(conn, deck_id, version)? {
            Some(id) => sideboard_keys(conn, id)?,
            None => BTreeSet::new(),
        },
    };
    stats_of(
        conn,
        Target::Deck,
        &Value::Integer(deck_id),
        version,
        filter,
        &sideboard,
    )
}

/// The card stats of every game of these deck versions under `filter` (its
/// version is ignored): the deck index's archetype Card stats tab (0.x
/// `GetCardGameStats::forVersionIds`). A card is a sideboard card when any
/// of the versions has it in the sideboard (0.x `GetSideboardOracles`).
pub fn card_stats_of_versions(
    conn: &Connection,
    version_ids: &[i64],
    filter: &CardStatsFilter,
) -> Result<Vec<CardStat>, StoreError> {
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut sideboard = BTreeSet::new();
    if filter.side == Side::Local {
        for id in version_ids {
            sideboard.extend(sideboard_keys(conn, *id)?);
        }
    }
    stats_of(
        conn,
        Target::Versions,
        &Value::Text(crate::matchups::ids_json(version_ids)),
        None,
        filter,
        &sideboard,
    )
}

/// The card stats of `target`'s games: `subject` is its `?1` (a deck id or
/// a JSON array of versions), `version` its `?2`, `sideboard` the keys of
/// the cards shown as sideboard cards.
fn stats_of(
    conn: &Connection,
    target: Target,
    subject: &Value,
    version: Option<i64>,
    filter: &CardStatsFilter,
    sideboard: &BTreeSet<String>,
) -> Result<Vec<CardStat>, StoreError> {
    let params = bound(subject, version, filter, filter.side);

    // Each card's rows per game, keyed by (card key index, game id).
    let mut keys = Keys::new();
    let mut games: HashMap<(usize, i64), GameCard> = HashMap::new();
    // Rows that are not deck rows (a variant or a face): folded once every
    // deck row is in, and only into a card in the deck that game.
    let mut others: Vec<((usize, i64), GameCard)> = Vec::new();
    let opponent = filter.side == Side::Opponent;
    let mut stmt = conn.prepare_cached(&rows_sql(target))?;
    let mut rows = stmt.query(params.clone())?;
    while let Some(row) = rows.next()? {
        let game_id: i64 = row.get(0)?;
        let catalog_id: u32 = row.get(3)?;
        let card = GameCard {
            won: row.get(1)?,
            postboard: row.get(2)?,
            quantity: count(row, 4)?,
            kept: count(row, 5)?,
            seen: count(row, 6)?,
            cast: count(row, 7)?,
            played: count(row, 8)?,
            activated: count(row, 9)?,
            sided_in: row.get(10)?,
            sided_out: row.get(11)?,
            revealed: row.get(12)?,
            pregame_played: row.get(13)?,
            inferred: row.get(14)?,
            top: catalog_id,
            ..GameCard::default()
        };
        let variant: bool = row.get(15)?;
        let index = keys.index(conn, catalog_id, variant || opponent)?;
        if variant {
            others.push(((index, game_id), card));
        } else {
            let mut card = card;
            if !keys.faces.is_empty() {
                card.top = keys.shown(catalog_id);
            }
            games.entry((index, game_id)).or_default().add(&card);
        }
    }
    for (at, card) in &others {
        if let Some(game) = games.get_mut(at).filter(|game| game.quantity > 0) {
            game.add_variant(card);
        }
    }
    if games.is_empty() {
        return Ok(Vec::new());
    }

    let mut stats: Vec<CardStat> = keys
        .keys
        .iter()
        .map(|key| CardStat {
            key: key.clone(),
            ..CardStat::default()
        })
        .collect();
    for ((index, _game_id), game) in &games {
        add_game(&mut stats[*index], &game.finished());
    }

    let mut stmt = conn.prepare_cached(&casts_sql(target))?;
    let mut rows = stmt.query(params.clone())?;
    while let Some(row) = rows.next()? {
        let catalog_id: u32 = row.get(0)?;
        let method: String = row.get(1)?;
        let total = count(row, 2)?;
        let index = keys.index(conn, catalog_id, opponent)?;
        if let Some(stat) = stats.get_mut(index) {
            stat.add_casts(&method, total);
        }
    }
    if !others.is_empty() {
        let mut stmt = conn.prepare_cached(&other_casts_sql(target))?;
        let mut rows = stmt.query(params)?;
        while let Some(row) = rows.next()? {
            let game_id: i64 = row.get(0)?;
            let catalog_id: u32 = row.get(1)?;
            let method: String = row.get(2)?;
            let total = count(row, 3)?;
            let index = keys.index(conn, catalog_id, true)?;
            let in_deck = games
                .get(&(index, game_id))
                .is_some_and(|game| game.quantity > 0);
            if in_deck && let Some(stat) = stats.get_mut(index) {
                stat.add_casts(&method, total);
            }
        }
    }

    let opponent_games = if filter.side == Side::Opponent {
        let local = bound(subject, version, filter, Side::Local);
        let games: i64 = conn
            .prepare_cached(&local_games_sql(target))?
            .query_row(local, |row| row.get(0))?;
        Some(u32::try_from(games).unwrap_or(u32::MAX))
    } else {
        None
    };

    // A cast row always has its card's row, so no key is without games.
    stats.retain(|stat| stat.total_games > 0);
    for stat in &mut stats {
        describe(conn, stat)?;
        if let Some(games) = opponent_games {
            stat.total_games = games;
        }
        stat.is_sideboard = sideboard.contains(&stat.key);
    }
    // 0.x `ORDER BY type, name`: no type first, then byte order.
    stats.sort_by(|a, b| (&a.card_type, &a.name, &a.key).cmp(&(&b.card_type, &b.name, &b.key)));
    Ok(stats)
}

/// Fill a stat's card data: the printing shown is the highest catalog id
/// among the printings played in the games read (S8-1), with its card data.
fn describe(conn: &Connection, stat: &mut CardStat) -> Result<(), StoreError> {
    let shown = stat.catalog_id;
    let card = conn
        .prepare_cached(
            "SELECT oracle_id, name, color_identity, type, image, local_image
             FROM cards WHERE catalog_id = ?1",
        )?
        .query_row([shown], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .optional()?;
    let (oracle_id, name, identity, card_type, image, local_image) =
        card.unwrap_or((None, None, None, None, None, None));
    stat.oracle_id = oracle_id;
    stat.name = name.unwrap_or_else(|| format!("Card {shown}"));
    stat.color_identity = cards::normalize_colors(identity.as_deref());
    stat.card_type = card_type;
    stat.image = image;
    stat.local_image = local_image;
    Ok(())
}

/// The version whose sideboard marks cards: the chosen one, else the deck's
/// latest (0.x `$deckVersion ?? $deck->latestVersion`).
fn sideboard_version(
    conn: &Connection,
    deck_id: i64,
    chosen: Option<i64>,
) -> Result<Option<i64>, StoreError> {
    if chosen.is_some() {
        return Ok(chosen);
    }
    let versions = crate::decks::versions_of(conn, deck_id)?;
    Ok(versions.last().map(|version| version.id))
}

/// The keys (`cards::stats::stat_key`) of the sideboard cards of version
/// `version_id`.
pub fn sideboard_keys(conn: &Connection, version_id: i64) -> Result<BTreeSet<String>, StoreError> {
    let signature: Option<String> = conn
        .prepare_cached("SELECT signature FROM deck_versions WHERE id = ?1")?
        .query_row([version_id], |row| row.get(0))
        .optional()?;
    let mut keys = BTreeSet::new();
    let Some(signature) = signature else {
        return Ok(keys);
    };
    let mut stmt = conn.prepare_cached("SELECT oracle_id FROM cards WHERE catalog_id = ?1")?;
    for card in decks::decode_signature(&signature) {
        if !card.sideboard {
            continue;
        }
        let oracle: Option<String> = stmt
            .query_row([card.catalog_id], |row| row.get(0))
            .optional()?
            .flatten();
        keys.insert(stat_key(oracle.as_deref(), card.catalog_id));
    }
    Ok(keys)
}

/// The games of the target's listed complete matches (`?1`, `?2`: `Target`)
/// under the filters of `rows_from` (no side; the archetype is `?7`): how
/// many, and how many were won. Under a board filter a game counts only with
/// a card row 0.x would have had (0.x's `EXISTS` on `card_game_stats`, S8-2).
fn prior_sql(target: Target) -> String {
    format!(
        "SELECT COUNT(*), COALESCE(SUM(g.result = 'win'), 0)
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
JOIN games g ON g.match_id = m.id
WHERE {}
  AND m.state = 'complete' AND m.started_at BETWEEN ?3 AND ?4
  AND (?5 IS NULL OR ((g.game_number > 1) = ?5 AND EXISTS (SELECT 1 FROM game_cards gc
       WHERE gc.game_id = g.id AND {})))
  AND (?6 IS NULL OR g.on_play = ?6)
  AND (?8 IS NULL OR m.game_structure = ?8)
  AND {}",
        target.condition(),
        deck_row("gc"),
        against_archetype("?7")
    )
}

/// No games: the prior is a coin flip.
const NO_PRIOR: DeckWinrate = DeckWinrate {
    wins: 0,
    games: 0,
    rate: 0.5,
};

/// The deck's game win rate under `filter` (0.x `GetFilteredDeckWinrate`).
pub fn deck_prior(
    conn: &Connection,
    deck_id: i64,
    filter: &CardStatsFilter,
) -> Result<DeckWinrate, StoreError> {
    let version = existing_version(conn, filter.version_id)?;
    prior_of(conn, Target::Deck, Value::Integer(deck_id), version, filter)
}

/// The game win rate of these deck versions under `filter` (its version is
/// ignored; 0.x `GetFilteredDeckWinrate::forVersionIds`).
pub fn deck_prior_of_versions(
    conn: &Connection,
    version_ids: &[i64],
    filter: &CardStatsFilter,
) -> Result<DeckWinrate, StoreError> {
    if version_ids.is_empty() {
        return Ok(NO_PRIOR);
    }
    let subject = Value::Text(crate::matchups::ids_json(version_ids));
    prior_of(conn, Target::Versions, subject, None, filter)
}

fn prior_of(
    conn: &Connection,
    target: Target,
    subject: Value,
    version: Option<i64>,
    filter: &CardStatsFilter,
) -> Result<DeckWinrate, StoreError> {
    let postboard = filter.board.map(|board| board == Board::Postboard);
    let (games, wins) = conn.prepare_cached(&prior_sql(target))?.query_row(
        params![
            subject,
            version,
            filter.timeframe.from_ms,
            filter.timeframe.to_ms,
            postboard,
            filter.on_play,
            filter.opponent_archetype_id,
            filter.best_of
        ],
        |row| Ok((count(row, 0)?, count(row, 1)?)),
    )?;
    if games == 0 {
        return Ok(NO_PRIOR);
    }
    Ok(DeckWinrate {
        wins,
        games,
        rate: f64::from(wins) / f64::from(games),
    })
}

/// A sideboard guide's numbers (archetypes spec section 21.3, 0.x
/// `BuildSideboardGuide::aggregate` and `postboardTotals`): over every
/// version of the deck, all time, games against the opponent archetype,
/// the local rows per oracle id (printings added together; a card without
/// an oracle id is left out): games it was sided in, won and lost (a draw
/// counts as lost), and games it was sided out; then the postboard games
/// and their wins.
pub fn sided_stats(
    conn: &Connection,
    deck_id: i64,
    archetype_id: i64,
) -> Result<(BTreeMap<String, SidedStat>, Postboard), StoreError> {
    let filter = CardStatsFilter {
        opponent_archetype_id: Some(archetype_id),
        ..CardStatsFilter::all()
    };
    let subject = Value::Integer(deck_id);
    let params = bound(&subject, None, &filter, Side::Local);
    let mut keys = Keys::new();
    let mut games: HashMap<(usize, i64), GameCard> = HashMap::new();
    let mut stmt = conn.prepare_cached(&rows_sql(Target::Deck))?;
    let mut rows = stmt.query(params)?;
    while let Some(row) = rows.next()? {
        let game_id: i64 = row.get(0)?;
        let catalog_id: u32 = row.get(3)?;
        let card = GameCard {
            won: row.get(1)?,
            sided_in: row.get(10)?,
            sided_out: row.get(11)?,
            ..GameCard::default()
        };
        // Deck rows only, as before faces were resolved: a sided row is a
        // deck row.
        let index = keys.index(conn, catalog_id, false)?;
        games.entry((index, game_id)).or_default().add(&card);
    }
    let mut stats: BTreeMap<String, SidedStat> = BTreeMap::new();
    for ((index, _game_id), game) in &games {
        let key = &keys.keys[*index];
        if key.starts_with('#') {
            continue;
        }
        let stat = stats.entry(key.clone()).or_default();
        if game.sided_in {
            stat.sided_in_games += 1;
            if game.won {
                stat.sided_in_won += 1;
            } else {
                stat.sided_in_lost += 1;
            }
        }
        if game.sided_out {
            stat.sided_out_games += 1;
        }
    }

    let postboard_filter = CardStatsFilter {
        board: Some(Board::Postboard),
        ..filter
    };
    let params = bound(&subject, None, &postboard_filter, Side::Local);
    let sql = format!(
        "SELECT COUNT(DISTINCT g.id), COUNT(DISTINCT CASE WHEN g.result = 'win' THEN g.id END) {}",
        rows_from(Target::Deck, "game_cards", true)
    );
    let postboard = conn.prepare_cached(&sql)?.query_row(params, |row| {
        Ok(Postboard {
            games: count(row, 0)?,
            wins: count(row, 1)?,
        })
    })?;
    Ok((stats, postboard))
}

/// The archetypes on the opponent seat of the target's listed matches with
/// at least one game (0.x `availableArchetypesForVersionIds`: any match
/// state, a game row for the non-local player), by name then id.
fn options_sql(target: Target) -> String {
    format!(
        "SELECT a.id, a.name, a.color_identity
FROM archetypes a
WHERE EXISTS (
    SELECT 1 FROM match_archetypes ma
    JOIN listed_matches m ON m.id = ma.match_id
    JOIN deck_versions v ON v.id = m.deck_version_id
    WHERE ma.archetype_id = a.id AND ma.is_opponent = 1
      AND {}
      AND (?3 IS NULL OR m.game_structure = ?3)
      AND EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id))
ORDER BY a.name, a.id",
        target.condition()
    )
}

fn options_of(
    conn: &Connection,
    target: Target,
    subject: &Value,
    version: Option<i64>,
    best_of: Option<BestOf>,
) -> Result<Vec<ArchetypeOption>, StoreError> {
    let mut stmt = conn.prepare_cached(&options_sql(target))?;
    let rows = stmt.query_map(params![subject, version, best_of], |row| {
        let colors: Option<String> = row.get(2)?;
        Ok(ArchetypeOption {
            id: row.get(0)?,
            name: row.get(1)?,
            color_identity: archetypes::normalize_colors(colors.as_deref()),
        })
    })?;
    let mut options = Vec::new();
    for row in rows {
        options.push(row?);
    }
    Ok(options)
}

/// The opponent archetype filter's options for the deck Card Stats tab
/// (0.x `GetCardGameStats::availableArchetypes`): the archetypes the deck's
/// matches (of `version_id` when that version exists, else every version)
/// were played against, by name.
pub fn opponent_archetypes(
    conn: &Connection,
    deck_id: i64,
    version_id: Option<i64>,
    best_of: Option<BestOf>,
) -> Result<Vec<ArchetypeOption>, StoreError> {
    let version = existing_version(conn, version_id)?;
    options_of(
        conn,
        Target::Deck,
        &Value::Integer(deck_id),
        version,
        best_of,
    )
}

/// The same options over these deck versions: the deck index's archetype
/// Card stats tab (0.x `availableArchetypesForVersionIds`).
pub fn opponent_archetypes_of_versions(
    conn: &Connection,
    version_ids: &[i64],
    best_of: Option<BestOf>,
) -> Result<Vec<ArchetypeOption>, StoreError> {
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }
    options_of(
        conn,
        Target::Versions,
        &Value::Text(crate::matchups::ids_json(version_ids)),
        None,
        best_of,
    )
}

/// "Regenerate" (cards spec section 8, 0.x `RegenerateCardGameStats`): of
/// the deck's complete matches with games, the log matches are returned to
/// be synced again and the imported postboard rule is applied again to the
/// imported and history matches. Manual matches are the player's input and
/// are left alone, as are Arena and Endstep log matches (their sync rows
/// never read MTGO card data). Hidden matches are regenerated too (0.x did).
pub fn regenerate(tx: &Transaction, deck_id: i64) -> Result<Regenerated, StoreError> {
    let mut found: Vec<(i64, String, String)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT m.id, m.token, m.source
             FROM deck_versions v JOIN matches m ON m.deck_version_id = v.id
             JOIN accounts a ON a.id = m.account_id
             WHERE v.deck_id = ?1 AND m.state = 'complete' AND m.source <> 'manual'
               AND NOT (m.source = 'log' AND a.platform <> 'mtgo')
               AND EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id)
             ORDER BY m.id",
        )?;
        let rows = stmt.query_map([deck_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        for row in rows {
            found.push(row?);
        }
    }
    let mut done = Regenerated::default();
    for (id, token, source) in found {
        if source == "log" {
            done.log_tokens.push(token);
        } else {
            crate::imported_detail::reapply_imported_rule(tx, id)?;
            done.rebuilt += 1;
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use crate::test_support::hide;
    use decks::{DeckCard, signature};
    use fixture_tools::TestDir;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0)",
            [],
        )
        .expect("account");
        (dir, conn)
    }

    fn deck(conn: &Connection) -> i64 {
        conn.query_row(
            "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES ('deck-' || (SELECT COUNT(*) FROM decks), 1, 'Qdeck', 'CMODERN', 0, 0, 'file')
             RETURNING id",
            [],
            |row| row.get(0),
        )
        .expect("deck")
    }

    fn version_with(conn: &Connection, deck_id: i64, signature: &str) -> i64 {
        conn.query_row(
            "INSERT INTO deck_versions (deck_id, signature, modified_at)
             VALUES (?1, ?2, (SELECT COUNT(*) FROM deck_versions)) RETURNING id",
            params![deck_id, signature],
            |row| row.get(0),
        )
        .expect("version")
    }

    fn version(conn: &Connection, deck_id: i64) -> i64 {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM deck_versions", [], |row| row.get(0))
            .expect("count");
        version_with(conn, deck_id, &format!("SIG-{n}"))
    }

    fn card(conn: &Connection, catalog_id: u32, oracle: &str, name: &str, card_type: &str) {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
             VALUES (?1, ?2, ?3, ?4, 'R')",
            params![catalog_id, oracle, name, card_type],
        )
        .expect("card");
    }

    /// A match of `version` with `source` and `state`, started at `started_at`.
    fn match_with(
        conn: &Connection,
        version_id: i64,
        source: &str,
        state: &str,
        started_at: i64,
    ) -> i64 {
        conn.query_row(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version, deck_version_id)
             VALUES ('m-' || (SELECT COUNT(*) FROM matches), 1, ?1, ?2, 'win', ?3, ?3, 1, 1, ?4)
             RETURNING id",
            params![source, state, started_at, version_id],
            |row| row.get(0),
        )
        .expect("match")
    }

    fn complete(conn: &Connection, version_id: i64) -> i64 {
        match_with(conn, version_id, "log", "complete", 1_000)
    }

    fn game(conn: &Connection, match_id: i64, result: &str, on_play: Option<bool>) -> i64 {
        conn.query_row(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play)
             VALUES (?1, (SELECT COUNT(*) + 1 FROM games WHERE match_id = ?1), ?2, 'win', ?3)
             RETURNING id",
            params![match_id, result, on_play],
            |row| row.get(0),
        )
        .expect("game")
    }

    /// A `game_cards` row: 0.x's `insertCardGameStat` defaults on the local
    /// side (4 copies, 2 kept, 3 seen), nothing on the opponent's, then
    /// `values` (`kept` sets the opening and starting hand).
    fn stat(conn: &Connection, game_id: i64, side: Side, catalog_id: u32, values: &[(&str, i64)]) {
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id) VALUES (?1, ?2, ?3)",
            params![game_id, side.as_str(), catalog_id],
        )
        .expect("row");
        let mut all: Vec<(&str, i64)> = Vec::new();
        if side == Side::Local {
            all.extend([("main_qty", 4), ("kept", 2), ("seen", 3)]);
        }
        all.extend_from_slice(values);
        for (column, value) in all {
            let sql = if column == "kept" {
                "UPDATE game_cards SET opening_hand = ?1, starting_hand = ?1
                 WHERE game_id = ?2 AND side = ?3 AND catalog_id = ?4"
                    .to_string()
            } else {
                format!(
                    "UPDATE game_cards SET {column} = ?1
                     WHERE game_id = ?2 AND side = ?3 AND catalog_id = ?4"
                )
            };
            conn.execute(&sql, params![value, game_id, side.as_str(), catalog_id])
                .expect(column);
        }
    }

    fn local(conn: &Connection, game_id: i64, catalog_id: u32, values: &[(&str, i64)]) {
        stat(conn, game_id, Side::Local, catalog_id, values);
    }

    fn read(conn: &Connection, deck_id: i64, filter: &CardStatsFilter) -> Vec<CardStat> {
        card_stats(conn, deck_id, filter).expect("card stats")
    }

    fn only(conn: &Connection, deck_id: i64, filter: &CardStatsFilter) -> CardStat {
        let stats = read(conn, deck_id, filter);
        assert_eq!(stats.len(), 1, "one card: {stats:?}");
        stats.into_iter().next().expect("one")
    }

    #[test]
    fn card_stats_read_one_kind() {
        let (_dir, conn) = db("card-stats-kinds");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "kind-oracle", "Qbolt", "Instant");
        let bo1 = complete(&conn, v);
        let bo3 = complete(&conn, v);
        crate::test_support::set_game_structure(&conn, &token(&conn, bo1), "bo1");
        crate::test_support::set_game_structure(&conn, &token(&conn, bo3), "bo3");
        let won = game(&conn, bo1, "win", Some(true));
        let lost_one = game(&conn, bo3, "loss", Some(true));
        let lost_two = game(&conn, bo3, "loss", Some(false));
        for id in [won, lost_one, lost_two] {
            local(&conn, id, 101, &[]);
        }
        let kind = |best_of| CardStatsFilter {
            best_of,
            ..CardStatsFilter::all()
        };
        assert_eq!(
            only(&conn, deck_id, &kind(Some(BestOf::One))).total_games,
            1
        );
        assert_eq!(
            only(&conn, deck_id, &kind(Some(BestOf::Three))).total_games,
            2
        );
        assert_eq!(only(&conn, deck_id, &kind(None)).total_games, 3);
        let prior = deck_prior(&conn, deck_id, &kind(Some(BestOf::One))).expect("prior");
        assert_eq!((prior.wins, prior.games), (1, 1));
        let prior = deck_prior(&conn, deck_id, &kind(Some(BestOf::Three))).expect("prior");
        assert_eq!((prior.wins, prior.games), (0, 2));
        let faced = archetype(&conn, "Qkind");
        seat(&conn, bo3, true, faced);
        let options =
            |best_of| opponent_archetypes(&conn, deck_id, None, best_of).expect("options");
        assert!(options(Some(BestOf::One)).is_empty());
        assert_eq!(options(Some(BestOf::Three)).len(), 1);
    }

    // ---- GetCardGameStatsTest ----------------------------------------------

    #[test]
    fn aggregates_card_stats_across_all_deck_versions_when_no_version_specified() {
        let (_dir, conn) = db("card-stats-all-versions");
        let deck_id = deck(&conn);
        let v1 = version(&conn, deck_id);
        let v2 = version(&conn, deck_id);
        card(&conn, 101, "test-oracle-1", "Qbolt", "Instant");
        let game1 = game(&conn, complete(&conn, v1), "win", None);
        let game2 = game(&conn, complete(&conn, v2), "loss", None);
        local(&conn, game1, 101, &[("kept", 3)]);
        local(&conn, game2, 101, &[("kept", 1)]);

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.total_games, 2);
        assert_eq!(row.total_kept, 4);
        assert_eq!(row.oracle_id.as_deref(), Some("test-oracle-1"));
        assert_eq!(row.key, "test-oracle-1");
        assert_eq!(row.name, "Qbolt");
        assert_eq!(row.color_identity.as_deref(), Some("R"));
    }

    #[test]
    fn filters_card_stats_to_a_single_version_when_version_specified() {
        let (_dir, conn) = db("card-stats-one-version");
        let deck_id = deck(&conn);
        let v1 = version(&conn, deck_id);
        let v2 = version(&conn, deck_id);
        card(&conn, 102, "test-oracle-2", "Qcounter", "Instant");
        let game1 = game(&conn, complete(&conn, v1), "win", None);
        let game2 = game(&conn, complete(&conn, v2), "loss", None);
        local(&conn, game1, 102, &[("kept", 2)]);
        local(&conn, game2, 102, &[("kept", 4)]);

        let filter = CardStatsFilter {
            version_id: Some(v1),
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &filter);
        assert_eq!(row.total_games, 1);
        assert_eq!(row.total_kept, 2);
    }

    #[test]
    fn counts_games_where_an_event_occurred_separately_from_total_copies() {
        let (_dir, conn) = db("card-stats-event-games");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 103, "test-oracle-games", "Qswamp", "Basic Land");
        let m = complete(&conn, v);
        let game1 = game(&conn, m, "win", None);
        let game2 = game(&conn, m, "loss", None);
        let game3 = game(&conn, m, "win", None);
        let row = |played, seen, kept| {
            [
                ("main_qty", 9),
                ("played", played),
                ("seen", seen),
                ("kept", kept),
                ("cast", 0),
            ]
        };
        local(&conn, game1, 103, &row(4, 3, 2));
        local(&conn, game2, 103, &row(0, 0, 0));
        local(&conn, game3, 103, &row(2, 1, 0));

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.total_games, 3);
        assert_eq!(row.total_possible, 27);
        assert_eq!(row.total_played, 6);
        assert_eq!(row.total_seen, 4);
        assert_eq!(row.total_kept, 2);
        assert_eq!(row.played_games, 2);
        assert_eq!(row.seen_games, 2);
        assert_eq!(row.kept_games, 1);
        assert_eq!(row.cast_games, 0);
    }

    #[test]
    fn aggregates_pregame_revealed_and_played_counts_per_card() {
        let (_dir, conn) = db("card-stats-pregame");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 104, "test-oracle-pregame", "Qdevourer", "Creature");
        let m = complete(&conn, v);
        let game1 = game(&conn, m, "win", None);
        let game2 = game(&conn, m, "loss", None);
        let game3 = game(&conn, m, "win", None);
        local(&conn, game1, 104, &[("pregame_revealed", 1)]);
        local(
            &conn,
            game2,
            104,
            &[("pregame_revealed", 1), ("pregame_played", 1)],
        );
        local(&conn, game3, 104, &[]);

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.total_games, 3);
        assert_eq!(row.pregame_revealed_games, 2);
        assert_eq!(row.pregame_played_games, 1);
        assert_eq!(row.pregame_games, 2);
        assert_eq!(row.pregame_won, 1);
        assert_eq!(row.pregame_lost, 1);
    }

    #[test]
    fn does_not_blow_up_when_a_sideboard_card_has_no_oracle_id_mapping() {
        let (_dir, conn) = db("card-stats-sideboard-no-oracle");
        let deck_id = deck(&conn);
        let list = [
            DeckCard {
                catalog_id: 1234,
                quantity: 4,
                sideboard: false,
            },
            DeckCard {
                catalog_id: 5678,
                quantity: 1,
                sideboard: true,
            },
        ];
        let v = version_with(&conn, deck_id, &signature(&list));
        card(&conn, 1234, "mainboard-oracle", "Qmain", "Instant");
        let g = game(&conn, complete(&conn, v), "win", None);
        local(&conn, g, 1234, &[]);

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.oracle_id.as_deref(), Some("mainboard-oracle"));
        assert!(!row.is_sideboard);
        // v1 keys the unresolved sideboard card by its catalog id.
        let keys = sideboard_keys(&conn, v).expect("keys");
        assert_eq!(keys, BTreeSet::from(["#5678".to_string()]));
    }

    #[test]
    fn filters_by_on_play() {
        let (_dir, conn) = db("card-stats-on-play");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 105, "test-oracle-3", "Qritual", "Instant");
        let m = complete(&conn, v);
        let on_play = game(&conn, m, "win", Some(true));
        let on_draw = game(&conn, m, "loss", Some(false));
        local(&conn, on_play, 105, &[("kept", 2)]);
        local(&conn, on_draw, 105, &[("kept", 1)]);

        let play = CardStatsFilter {
            version_id: Some(v),
            on_play: Some(true),
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &play);
        assert_eq!((row.total_games, row.total_kept), (1, 2));
        let draw = CardStatsFilter {
            on_play: Some(false),
            ..play
        };
        let row = only(&conn, deck_id, &draw);
        assert_eq!((row.total_games, row.total_kept), (1, 1));
    }

    fn archetype(conn: &Connection, uuid: &str) -> i64 {
        conn.query_row(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, ?1, 'modern', 'manual', 1, 1) RETURNING id",
            [uuid],
            |row| row.get(0),
        )
        .expect("archetype")
    }

    /// A seat's archetype row for a match.
    fn seat(conn: &Connection, match_id: i64, is_opponent: bool, archetype_id: i64) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, ?2, ?3, 1.0, 0)",
            params![match_id, is_opponent, archetype_id],
        )
        .expect("seat");
    }

    /// 0.x "filters by opponent archetype and on_play together with a single
    /// games join" (decision C6): only games whose opponent seat is the
    /// archetype count; the player's own seat never does.
    #[test]
    fn filters_by_opponent_archetype_and_on_play_together() {
        let (_dir, conn) = db("card-stats-archetype-filter");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 106, "test-oracle-combo", "Qpush", "Instant");
        let wanted = archetype(&conn, "q-wanted");
        let other = archetype(&conn, "q-other");

        let match_wanted = complete(&conn, v);
        seat(&conn, match_wanted, true, wanted);
        let on_play = game(&conn, match_wanted, "win", Some(true));
        local(&conn, on_play, 106, &[("kept", 2)]);
        let on_draw = game(&conn, match_wanted, "loss", Some(false));
        local(&conn, on_draw, 106, &[("kept", 5)]);

        let match_other = complete(&conn, v);
        seat(&conn, match_other, true, other);
        // The player's own seat holding the wanted archetype is not a game
        // against it.
        seat(&conn, match_other, false, wanted);
        let other_game = game(&conn, match_other, "win", Some(true));
        local(&conn, other_game, 106, &[("kept", 9)]);

        let play = CardStatsFilter {
            version_id: Some(v),
            on_play: Some(true),
            opponent_archetype_id: Some(wanted),
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &play);
        assert_eq!((row.total_games, row.total_kept), (1, 2));
        let draw = CardStatsFilter {
            on_play: Some(false),
            ..play
        };
        let row = only(&conn, deck_id, &draw);
        assert_eq!((row.total_games, row.total_kept), (1, 5));

        let prior = deck_prior(&conn, deck_id, &play).expect("prior");
        assert_eq!((prior.wins, prior.games), (1, 1));
        let all = CardStatsFilter {
            opponent_archetype_id: Some(wanted),
            ..CardStatsFilter::all()
        };
        let prior = deck_prior(&conn, deck_id, &all).expect("prior");
        assert_eq!((prior.wins, prior.games), (1, 2));
    }

    /// The deck index's archetype Card stats tab (the `Versions` target)
    /// honours the opponent archetype filter like the deck tab: across the
    /// versions of several decks, only games whose opponent seat is the
    /// archetype count, in the stats and in the prior.
    #[test]
    fn the_versions_target_honours_the_opponent_archetype_filter() {
        let (_dir, conn) = db("card-stats-versions-archetype-filter");
        let first = version(&conn, deck(&conn));
        let second = version(&conn, deck(&conn));
        card(&conn, 106, "test-oracle-combo", "Qpush", "Instant");
        let wanted = archetype(&conn, "q-wanted");
        let other = archetype(&conn, "q-other");

        let match_first = complete(&conn, first);
        seat(&conn, match_first, true, wanted);
        let won = game(&conn, match_first, "win", Some(true));
        local(&conn, won, 106, &[("kept", 2)]);
        let match_second = complete(&conn, second);
        seat(&conn, match_second, true, wanted);
        let lost = game(&conn, match_second, "loss", Some(false));
        local(&conn, lost, 106, &[("kept", 3)]);

        let match_other = complete(&conn, second);
        seat(&conn, match_other, true, other);
        seat(&conn, match_other, false, wanted);
        let other_game = game(&conn, match_other, "win", Some(true));
        local(&conn, other_game, 106, &[("kept", 9)]);

        let ids = [first, second];
        let filter = CardStatsFilter {
            opponent_archetype_id: Some(wanted),
            ..CardStatsFilter::all()
        };
        let stats = card_stats_of_versions(&conn, &ids, &filter).expect("stats");
        assert_eq!(stats.len(), 1, "one card: {stats:?}");
        assert_eq!((stats[0].total_games, stats[0].total_kept), (2, 5));
        let prior = deck_prior_of_versions(&conn, &ids, &filter).expect("prior");
        assert_eq!((prior.wins, prior.games), (1, 2));

        let unfiltered =
            card_stats_of_versions(&conn, &ids, &CardStatsFilter::all()).expect("stats");
        assert_eq!(unfiltered[0].total_games, 3);
        let unknown = CardStatsFilter {
            opponent_archetype_id: Some(other + 100),
            ..CardStatsFilter::all()
        };
        assert!(
            card_stats_of_versions(&conn, &ids, &unknown)
                .expect("stats")
                .is_empty()
        );
    }

    /// 0.x ApplyOpponentArchetypeFilterTest "keeps only rows from games
    /// against the given archetype": of two games against two archetypes,
    /// only the wanted one's rows are read.
    #[test]
    fn keeps_only_rows_from_games_against_the_given_archetype() {
        let (_dir, conn) = db("card-stats-apply-archetype-filter");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 110, "o-keep", "Qkeep", "Instant");
        card(&conn, 111, "o-drop", "Qdrop", "Instant");
        let wanted = archetype(&conn, "q-esper-blink");
        let other = archetype(&conn, "q-burn");

        let kept = complete(&conn, v);
        seat(&conn, kept, true, wanted);
        let kept_game = game(&conn, kept, "win", None);
        local(&conn, kept_game, 110, &[]);
        let dropped = complete(&conn, v);
        seat(&conn, dropped, true, other);
        let dropped_game = game(&conn, dropped, "win", None);
        local(&conn, dropped_game, 111, &[]);

        let filter = CardStatsFilter {
            opponent_archetype_id: Some(wanted),
            ..CardStatsFilter::all()
        };
        let keys: Vec<String> = read(&conn, deck_id, &filter)
            .into_iter()
            .map(|stat| stat.key)
            .collect();
        assert_eq!(keys, vec!["o-keep"]);
    }

    /// The filter's options (0.x `GetCardGameStats::availableArchetypes`):
    /// the archetypes on the opponent seat of the deck's matches that have
    /// a game, by name, of one version when it exists, else every version.
    /// The player's own seat, other decks, hidden matches and matches
    /// without games are left out.
    #[test]
    fn the_options_are_the_opponent_archetypes_the_deck_faced() {
        let (_dir, conn) = db("card-stats-archetype-options");
        let deck_id = deck(&conn);
        let first = version(&conn, deck_id);
        let second = version(&conn, deck_id);
        let other_deck = version(&conn, deck(&conn));
        let burn = archetype(&conn, "Qburn");
        let affinity = archetype(&conn, "Qaffinity");
        let own = archetype(&conn, "Qown");
        let elsewhere = archetype(&conn, "Qelsewhere");
        let hidden = archetype(&conn, "Qhidden");
        let gameless = archetype(&conn, "Qgameless");
        conn.execute(
            "UPDATE archetypes SET color_identity = 'r,w' WHERE id = ?1",
            [burn],
        )
        .expect("colours");

        let vs_burn = complete(&conn, first);
        seat(&conn, vs_burn, true, burn);
        seat(&conn, vs_burn, false, own);
        game(&conn, vs_burn, "win", None);
        // Twice against one archetype is one option.
        let again = complete(&conn, first);
        seat(&conn, again, true, burn);
        game(&conn, again, "loss", None);
        let vs_affinity = complete(&conn, second);
        seat(&conn, vs_affinity, true, affinity);
        game(&conn, vs_affinity, "win", None);
        let theirs = complete(&conn, other_deck);
        seat(&conn, theirs, true, elsewhere);
        game(&conn, theirs, "win", None);
        let hidden_match = complete(&conn, second);
        seat(&conn, hidden_match, true, hidden);
        game(&conn, hidden_match, "win", None);
        let token: String = conn
            .query_row(
                "SELECT token FROM matches WHERE id = ?1",
                [hidden_match],
                |row| row.get(0),
            )
            .expect("token");
        hide(&conn, &token);
        let no_games = complete(&conn, second);
        seat(&conn, no_games, true, gameless);

        let names = |options: Vec<ArchetypeOption>| -> Vec<String> {
            options.into_iter().map(|option| option.name).collect()
        };
        let every = opponent_archetypes(&conn, deck_id, None, None).expect("options");
        assert_eq!(
            every[1],
            ArchetypeOption {
                id: burn,
                name: "Qburn".to_string(),
                color_identity: Some("W,R".to_string()),
            }
        );
        assert_eq!(names(every), vec!["Qaffinity", "Qburn"]);
        assert_eq!(
            names(opponent_archetypes(&conn, deck_id, Some(first), None).expect("first")),
            vec!["Qburn"]
        );
        // A version with no row reads every version (as the stats do).
        assert_eq!(
            names(opponent_archetypes(&conn, deck_id, Some(999), None).expect("missing")),
            vec!["Qaffinity", "Qburn"]
        );
        assert_eq!(
            names(
                opponent_archetypes_of_versions(&conn, &[second, other_deck], None).expect("tab")
            ),
            vec!["Qaffinity", "Qelsewhere"]
        );
        assert!(
            opponent_archetypes_of_versions(&conn, &[], None)
                .expect("none")
                .is_empty()
        );
    }

    /// An archetype no game was played against gives nothing.
    #[test]
    fn an_unknown_archetype_id_gives_nothing() {
        let (_dir, conn) = db("card-stats-archetype");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 106, "test-oracle-combo", "Qpush", "Instant");
        let g = game(&conn, complete(&conn, v), "win", Some(true));
        local(&conn, g, 106, &[]);

        let filter = CardStatsFilter {
            version_id: Some(v),
            on_play: Some(true),
            opponent_archetype_id: Some(7),
            ..CardStatsFilter::all()
        };
        assert!(read(&conn, deck_id, &filter).is_empty());
        let prior = deck_prior(&conn, deck_id, &filter).expect("prior");
        assert_eq!((prior.wins, prior.games, prior.rate), (0, 0, 0.5));
        assert!(
            opponent_archetypes(&conn, deck_id, None, None)
                .expect("archetypes")
                .is_empty()
        );
    }

    #[test]
    fn uses_total_games_played_as_denominator_for_opponent_percentages() {
        let (_dir, conn) = db("card-stats-opponent-denominator");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 107, "oracle-mine", "Qforest", "Land");
        card(&conn, 108, "oracle-opp", "Qbolt", "Instant");
        let m = complete(&conn, v);
        for i in 1..=5 {
            let g = game(&conn, m, if i <= 3 { "win" } else { "loss" }, None);
            local(&conn, g, 107, &[]);
            if i <= 2 {
                stat(&conn, g, Side::Opponent, 108, &[("seen", 1), ("cast", 1)]);
            }
        }

        let theirs = CardStatsFilter {
            side: Side::Opponent,
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &theirs);
        assert_eq!(row.total_games, 5);
        assert_eq!(row.cast_games, 2);
        assert_eq!(row.seen_games, 2);
    }

    #[test]
    fn returns_only_local_rows_by_default_and_only_opponent_rows_for_the_opponent_side() {
        let (_dir, conn) = db("card-stats-sides");
        let deck_id = deck(&conn);
        let list = [DeckCard {
            catalog_id: 110,
            quantity: 1,
            sideboard: true,
        }];
        let v = version_with(&conn, deck_id, &signature(&list));
        card(&conn, 109, "oracle-local", "Qforest", "Land");
        card(&conn, 110, "oracle-opp", "Qbolt", "Instant");
        let g = game(&conn, complete(&conn, v), "loss", None);
        local(&conn, g, 109, &[]);
        stat(&conn, g, Side::Opponent, 110, &[("seen", 1), ("cast", 1)]);

        let mine = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(mine.oracle_id.as_deref(), Some("oracle-local"));
        let theirs = CardStatsFilter {
            side: Side::Opponent,
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &theirs);
        assert_eq!(row.oracle_id.as_deref(), Some("oracle-opp"));
        assert!(!row.is_sideboard, "never for the opponent's cards");
    }

    /// 0.x "aggregates card stats across multiple deck versions via
    /// forVersionIds": v1 reads one deck's versions (the archetype tab that
    /// reads several decks is sub-project 09's), never another deck's.
    #[test]
    fn aggregates_across_a_decks_versions_and_never_another_decks() {
        let (_dir, conn) = db("card-stats-other-deck");
        let deck_a = deck(&conn);
        let deck_b = deck(&conn);
        let a1 = version(&conn, deck_a);
        let a2 = version(&conn, deck_a);
        let b = version(&conn, deck_b);
        card(&conn, 111, "oracle-multi-version", "Qstorm", "Instant");
        for version_id in [a1, a2, b] {
            let g = game(&conn, complete(&conn, version_id), "win", None);
            local(&conn, g, 111, &[]);
        }
        assert_eq!(only(&conn, deck_a, &CardStatsFilter::all()).total_games, 2);
        assert_eq!(only(&conn, deck_b, &CardStatsFilter::all()).total_games, 1);
    }

    // ---- v1 ---------------------------------------------------------------

    #[test]
    fn the_timeframe_filters_card_stats() {
        let (_dir, conn) = db("card-stats-timeframe");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 112, "q-oracle-time", "Qtime", "Instant");
        let old = game(
            &conn,
            match_with(&conn, v, "log", "complete", 1_000),
            "win",
            None,
        );
        let new = game(
            &conn,
            match_with(&conn, v, "log", "complete", 5_000),
            "win",
            None,
        );
        local(&conn, old, 112, &[("kept", 1)]);
        local(&conn, new, 112, &[("kept", 3)]);

        let window = CardStatsFilter {
            timeframe: Range {
                from_ms: 4_000,
                to_ms: 6_000,
            },
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &window);
        assert_eq!((row.total_games, row.total_kept), (1, 3));
        assert_eq!(deck_prior(&conn, deck_id, &window).expect("prior").games, 1);
        let empty = CardStatsFilter {
            timeframe: Range {
                from_ms: 10_000,
                to_ms: 20_000,
            },
            ..CardStatsFilter::all()
        };
        assert!(read(&conn, deck_id, &empty).is_empty());
    }

    #[test]
    fn unresolved_cards_show_by_catalog_id() {
        let (_dir, conn) = db("card-stats-unresolved");
        let deck_id = deck(&conn);
        let list = [DeckCard {
            catalog_id: 999,
            quantity: 1,
            sideboard: true,
        }];
        let v = version_with(&conn, deck_id, &signature(&list));
        // 998 has no row at all, 999 is a stub.
        conn.execute("INSERT INTO cards (catalog_id) VALUES (999)", [])
            .expect("stub");
        card(&conn, 113, "q-oracle-known", "Qknown", "Creature");
        let m = complete(&conn, v);
        let g1 = game(&conn, m, "win", None);
        let g2 = game(&conn, m, "loss", None);
        for g in [g1, g2] {
            local(&conn, g, 998, &[]);
            local(&conn, g, 999, &[]);
            local(&conn, g, 113, &[]);
        }

        let stats = read(&conn, deck_id, &CardStatsFilter::all());
        let names: Vec<&str> = stats.iter().map(|stat| stat.name.as_str()).collect();
        // Type then name: no type sorts first (0.x `ORDER BY type, name`).
        assert_eq!(names, ["Card 998", "Card 999", "Qknown"]);
        let stub = &stats[1];
        assert_eq!(stub.key, "#999");
        assert_eq!(stub.oracle_id, None);
        assert_eq!(stub.catalog_id, 999);
        assert_eq!(stub.total_games, 2);
        assert!(stub.is_sideboard);
        assert!(!stats[0].is_sideboard);
    }

    #[test]
    fn printings_of_one_card_are_one_row_counted_once_per_game() {
        let (_dir, conn) = db("card-stats-printings");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 120, "q-oracle-bolt", "Qbolt old", "Instant");
        card(&conn, 121, "q-oracle-bolt", "Qbolt", "Instant");
        let g = game(&conn, complete(&conn, v), "win", None);
        local(&conn, g, 120, &[("main_qty", 2), ("cast", 1)]);
        local(&conn, g, 121, &[("main_qty", 2), ("cast", 0)]);
        conn.execute(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES (?1, 'local', 120, 'kicked', 1), (?1, 'local', 121, 'kicked', 2)",
            [g],
        )
        .expect("casts");

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.total_games, 1);
        assert_eq!(row.total_possible, 4);
        assert_eq!((row.total_cast, row.cast_games, row.cast_won), (1, 1, 1));
        assert_eq!(row.total_kicked, 3);
        assert_eq!(row.catalog_id, 121, "the highest printing played is shown");
        assert_eq!(row.name, "Qbolt");
    }

    #[test]
    fn hidden_matches_are_left_out() {
        let (_dir, conn) = db("card-stats-hidden");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 114, "q-oracle-hidden", "Qhidden", "Instant");
        let shown = complete(&conn, v);
        let hidden = complete(&conn, v);
        for m in [shown, hidden] {
            let g = game(&conn, m, "win", None);
            local(&conn, g, 114, &[]);
        }
        let token: String = conn
            .query_row("SELECT token FROM matches WHERE id = ?1", [hidden], |row| {
                row.get(0)
            })
            .expect("token");
        hide(&conn, &token);
        // Not complete: left out too.
        let live = match_with(&conn, v, "log", "in_progress", 1_000);
        local(&conn, game(&conn, live, "win", None), 114, &[]);

        assert_eq!(only(&conn, deck_id, &CardStatsFilter::all()).total_games, 1);
        assert_eq!(
            deck_prior(&conn, deck_id, &CardStatsFilter::all())
                .expect("prior")
                .games,
            1
        );
    }

    #[test]
    fn a_draw_counts_as_not_won() {
        let (_dir, conn) = db("card-stats-draw");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 115, "q-oracle-draw", "Qdraw", "Instant");
        let m = complete(&conn, v);
        let won = game(&conn, m, "win", None);
        let drawn = game(&conn, m, "draw", None);
        local(&conn, won, 115, &[("cast", 1)]);
        local(&conn, drawn, 115, &[("cast", 1)]);

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!((row.cast_games, row.cast_won, row.cast_lost), (2, 1, 1));
        assert_eq!((row.kept_won, row.kept_lost), (1, 1));
        let prior = deck_prior(&conn, deck_id, &CardStatsFilter::all()).expect("prior");
        assert_eq!((prior.wins, prior.games, prior.rate), (1, 2, 0.5));
    }

    #[test]
    fn inferred_games_are_counted_per_card() {
        let (_dir, conn) = db("card-stats-inferred");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 116, "q-oracle-main", "Qmain", "Creature");
        card(&conn, 117, "q-oracle-side", "Qside", "Instant");
        let m = match_with(&conn, v, "imported", "complete", 1_000);
        let g1 = game(&conn, m, "win", None);
        let g2 = game(&conn, m, "loss", None);
        let g3 = game(&conn, m, "win", None);
        local(&conn, g1, 116, &[]);
        local(&conn, g2, 116, &[("inferred", 1)]);
        local(&conn, g3, 116, &[("inferred", 1)]);
        local(&conn, g2, 117, &[("inferred", 1), ("sided_in", 1)]);

        let stats = read(&conn, deck_id, &CardStatsFilter::all());
        let main = stats.iter().find(|s| s.name == "Qmain").expect("main");
        let side = stats.iter().find(|s| s.name == "Qside").expect("side");
        assert_eq!(main.inferred_games, 2);
        assert_eq!((side.inferred_games, side.sided_in_games), (1, 1));
        assert_eq!(main.postboard_games, 2);
    }

    #[test]
    fn the_board_filter_reads_game_one_or_the_games_after_it() {
        let (_dir, conn) = db("card-stats-board");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 118, "q-oracle-board", "Qboard", "Instant");
        let m = complete(&conn, v);
        let g1 = game(&conn, m, "win", None);
        let g2 = game(&conn, m, "loss", None);
        let g3 = game(&conn, m, "loss", None);
        local(&conn, g1, 118, &[("kept", 1)]);
        local(&conn, g2, 118, &[("kept", 2)]);
        local(&conn, g3, 118, &[("kept", 3), ("sided_out", 1)]);

        let game1 = CardStatsFilter {
            board: Some(Board::Game1),
            ..CardStatsFilter::all()
        };
        assert_eq!(only(&conn, deck_id, &game1).total_kept, 1);
        let prior = deck_prior(&conn, deck_id, &game1).expect("prior");
        assert_eq!((prior.wins, prior.games, prior.rate), (1, 1, 1.0));
        let post = CardStatsFilter {
            board: Some(Board::Postboard),
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &post);
        assert_eq!((row.total_kept, row.postboard_games), (5, 2));
        assert_eq!(row.sided_out_games, 1);
        let prior = deck_prior(&conn, deck_id, &post).expect("prior");
        assert_eq!((prior.wins, prior.games, prior.rate), (0, 2, 0.0));
    }

    #[test]
    fn the_sideboard_is_the_chosen_versions_else_the_latest() {
        let (_dir, conn) = db("card-stats-sideboard-version");
        let deck_id = deck(&conn);
        let side_in_first = [DeckCard {
            catalog_id: 119,
            quantity: 1,
            sideboard: true,
        }];
        let main_in_latest = [DeckCard {
            catalog_id: 119,
            quantity: 1,
            sideboard: false,
        }];
        let first = version_with(&conn, deck_id, &signature(&side_in_first));
        let latest = version_with(&conn, deck_id, &signature(&main_in_latest));
        card(&conn, 119, "q-oracle-flex", "Qflex", "Instant");
        let g = game(&conn, complete(&conn, first), "win", None);
        local(&conn, g, 119, &[]);

        assert!(!only(&conn, deck_id, &CardStatsFilter::all()).is_sideboard);
        let chosen = CardStatsFilter {
            version_id: Some(first),
            ..CardStatsFilter::all()
        };
        assert!(only(&conn, deck_id, &chosen).is_sideboard);
        assert_eq!(
            sideboard_keys(&conn, first).expect("keys"),
            BTreeSet::from(["q-oracle-flex".to_string()])
        );
        assert!(sideboard_keys(&conn, latest).expect("keys").is_empty());
    }

    #[test]
    fn a_deck_without_games_has_no_stats_and_an_even_prior() {
        let (_dir, conn) = db("card-stats-empty");
        let deck_id = deck(&conn);
        assert!(read(&conn, deck_id, &CardStatsFilter::all()).is_empty());
        let prior = deck_prior(&conn, deck_id, &CardStatsFilter::all()).expect("prior");
        assert_eq!((prior.wins, prior.games, prior.rate), (0, 0, 0.5));
    }

    // ---- RegenerateCardStatsTest -------------------------------------------

    fn regenerate_deck(conn: &mut Connection, deck_id: i64) -> Regenerated {
        let tx = conn.transaction().expect("tx");
        let done = regenerate(&tx, deck_id).expect("regenerate");
        tx.commit().expect("commit");
        done
    }

    fn token(conn: &Connection, match_id: i64) -> String {
        conn.query_row(
            "SELECT token FROM matches WHERE id = ?1",
            [match_id],
            |row| row.get(0),
        )
        .expect("token")
    }

    #[test]
    fn queues_a_resync_for_each_log_complete_match_in_the_deck() {
        let (_dir, mut conn) = db("regenerate-each");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        let a = complete(&conn, v);
        let b = complete(&conn, v);
        game(&conn, a, "win", None);
        game(&conn, b, "win", None);

        let done = regenerate_deck(&mut conn, deck_id);
        assert_eq!(done.log_tokens, vec![token(&conn, a), token(&conn, b)]);
        assert_eq!(done.count(), 2);
    }

    #[test]
    fn regenerate_skips_matches_from_other_decks() {
        let (_dir, mut conn) = db("regenerate-other-decks");
        let deck_id = deck(&conn);
        let other = deck(&conn);
        let mine = complete(&conn, version(&conn, deck_id));
        let foreign = complete(&conn, version(&conn, other));
        game(&conn, mine, "win", None);
        game(&conn, foreign, "win", None);

        let done = regenerate_deck(&mut conn, deck_id);
        assert_eq!(done.log_tokens, vec![token(&conn, mine)]);
        assert_eq!(done.count(), 1);
    }

    #[test]
    fn regenerate_skips_live_matches_that_are_not_yet_complete() {
        let (_dir, mut conn) = db("regenerate-not-complete");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        let done_match = complete(&conn, v);
        let live = match_with(&conn, v, "log", "in_progress", 1_000);
        game(&conn, done_match, "win", None);
        game(&conn, live, "win", None);

        let done = regenerate_deck(&mut conn, deck_id);
        assert_eq!(done.log_tokens, vec![token(&conn, done_match)]);
    }

    #[test]
    fn regenerate_gives_0_when_the_deck_has_no_matches() {
        let (_dir, mut conn) = db("regenerate-none");
        let deck_id = deck(&conn);
        version(&conn, deck_id);
        assert_eq!(regenerate_deck(&mut conn, deck_id), Regenerated::default());
        assert_eq!(regenerate_deck(&mut conn, deck_id).count(), 0);
    }

    #[test]
    fn regenerate_rebuilds_imported_matches_and_leaves_manual_ones() {
        let (_dir, mut conn) = db("regenerate-sources");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        let imported = match_with(&conn, v, "imported", "complete", 1_000);
        let history = match_with(&conn, v, "history", "complete", 1_000);
        let manual = match_with(&conn, v, "manual", "complete", 1_000);
        // A complete match without games has nothing to regenerate (0.x
        // `whereHas('games')`).
        complete(&conn, v);
        for m in [imported, history, manual] {
            game(&conn, m, "win", None);
        }

        let done = regenerate_deck(&mut conn, deck_id);
        assert!(done.log_tokens.is_empty());
        assert_eq!(done.rebuilt, 2);
        assert_eq!(done.count(), 2);
    }
    // ---- the rows 0.x would have had (S8 review) ---------------------------

    #[test]
    fn a_game_one_sideboard_row_changes_no_card_stat() {
        let (_dir, conn) = db("card-stats-game-one-sideboard");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "o-main", "Qmain", "Creature");
        card(&conn, 102, "o-side", "Qside", "Sorcery");
        let m = complete(&conn, v);
        let game1 = game(&conn, m, "win", None);
        let game2 = game(&conn, m, "loss", None);
        local(&conn, game1, 101, &[]);
        local(
            &conn,
            game1,
            102,
            &[("main_qty", 0), ("side_qty", 2), ("seen", 0)],
        );
        local(&conn, game2, 101, &[]);
        local(
            &conn,
            game2,
            102,
            &[("main_qty", 0), ("side_qty", 2), ("seen", 0)],
        );

        let stats = read(&conn, deck_id, &CardStatsFilter::all());
        let side = stats
            .iter()
            .find(|s| s.name == "Qside")
            .expect("sideboard card");
        assert_eq!(side.total_games, 1, "only its postboard game");
        assert_eq!(side.postboard_games, 1);
        let main = stats.iter().find(|s| s.name == "Qmain").expect("main card");
        assert_eq!(main.total_games, 2);
    }

    #[test]
    fn a_token_row_with_no_deck_quantity_is_not_a_card_stat() {
        let (_dir, conn) = db("card-stats-token-row");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "o-main", "Qmain", "Creature");
        card(&conn, 900, "o-token", "Qtreasure", "Token Artifact");
        let m = complete(&conn, v);
        let game1 = game(&conn, m, "win", None);
        let game2 = game(&conn, m, "win", None);
        for game_id in [game1, game2] {
            local(&conn, game_id, 101, &[]);
            local(
                &conn,
                game_id,
                900,
                &[("main_qty", 0), ("seen", 1), ("cast", 1)],
            );
        }
        conn.execute(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES (?1, 'local', 900, 'free_cast', 1)",
            params![game2],
        )
        .expect("cast");

        let stats = read(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(stats.len(), 1, "only the deck card: {stats:?}");
        assert_eq!(stats[0].name, "Qmain");
    }

    #[test]
    fn postboard_zero_quantity_rows_stay_for_sideboard_and_sided_out_cards() {
        let (_dir, conn) = db("card-stats-postboard-zero-rows");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "o-main", "Qmain", "Creature");
        card(&conn, 102, "o-side", "Qside", "Sorcery");
        card(&conn, 103, "o-out", "Qout", "Creature");
        let m = complete(&conn, v);
        let _game1 = game(&conn, m, "win", None);
        let game2 = game(&conn, m, "win", None);
        local(&conn, game2, 101, &[]);
        local(
            &conn,
            game2,
            102,
            &[("main_qty", 0), ("side_qty", 3), ("seen", 0)],
        );
        local(
            &conn,
            game2,
            103,
            &[
                ("main_qty", 0),
                ("side_qty", 2),
                ("sided_out", 1),
                ("seen", 0),
            ],
        );

        let stats = read(&conn, deck_id, &CardStatsFilter::all());
        for name in ["Qside", "Qout"] {
            let row = stats
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name} kept"));
            assert_eq!(row.postboard_games, 1, "{name}");
        }
    }

    #[test]
    fn an_inferred_token_row_of_an_imported_rule_game_is_not_a_card_stat() {
        // S8-7: the imported rule marks every local row of a postboard game
        // inferred, tokens and log-only cards included; being inferred does
        // not make a row one 0.x would have had.
        let (_dir, conn) = db("card-stats-inferred-token");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "o-main", "Qmain", "Creature");
        card(&conn, 102, "o-side", "Qside", "Sorcery");
        card(&conn, 900, "o-token", "Qtreasure", "Token Artifact");
        let m = match_with(&conn, v, "imported", "complete", 1_000);
        let _game1 = game(&conn, m, "win", None);
        let game2 = game(&conn, m, "win", None);
        local(&conn, game2, 101, &[("inferred", 1)]);
        local(
            &conn,
            game2,
            102,
            &[
                ("main_qty", 0),
                ("side_qty", 2),
                ("seen", 0),
                ("inferred", 1),
            ],
        );
        local(
            &conn,
            game2,
            900,
            &[("main_qty", 0), ("seen", 1), ("cast", 1), ("inferred", 1)],
        );

        let names: Vec<String> = read(&conn, deck_id, &CardStatsFilter::all())
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["Qmain".to_string(), "Qside".to_string()]);
    }

    #[test]
    fn an_imported_games_variant_casts_count_for_the_deck_card() {
        // Review I1: MTGO logs some casts under a variant's catalog id
        // (warp). The imported rule folds the variant row into the deck
        // card, so its casts count as 0.x counted them. The rows are written
        // directly, as a history import has them (no 0.x card stats).
        let (_dir, mut conn) = db("card-stats-imported-variant");
        let deck_id = deck(&conn);
        let list = [DeckCard {
            catalog_id: 101,
            quantity: 4,
            sideboard: false,
        }];
        let v = version_with(&conn, deck_id, &signature(&list));
        card(&conn, 101, "o-main", "Qmain", "Creature");
        card(&conn, 191, "o-main", "Qmain", "Creature");
        let m = match_with(&conn, v, "imported", "complete", 1_000);
        let g1 = game(&conn, m, "win", None);
        let g2 = game(&conn, m, "win", None);
        local(&conn, g1, 101, &[]);
        local(&conn, g2, 101, &[("cast", 1)]);
        local(&conn, g2, 191, &[("main_qty", 0), ("seen", 1), ("cast", 2)]);
        conn.execute(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES (?1, 'local', 191, 'free_cast', 2)",
            [g2],
        )
        .expect("casts");
        let entry = |at: i64, message: &str| game_detail::GameLogEntry {
            timestamp_ms: at,
            message: message.to_string(),
        };
        let log = vec![
            entry(1_000, "@P@PQlocal joined the game."),
            entry(1_001, "@P@PQopponent joined the game."),
            entry(1_002, "@PQlocal wins the game."),
            entry(2_000, "@P@PQlocal joined the game."),
            entry(2_001, "@P@PQopponent joined the game."),
            entry(2_002, "@PTurn 1: Qlocal"),
            entry(2_003, "@PQlocal casts @[Qmain@:382,201:@]."),
            entry(2_004, "@PQlocal wins the game."),
        ];
        let tx = conn.transaction().expect("tx");
        crate::game_sync::store_game_log(&tx, m, &log).expect("log");
        tx.commit().expect("commit");
        regenerate_deck(&mut conn, deck_id);

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!((row.total_cast, row.cast_games), (3, 1));
        assert_eq!(row.total_free_cast, 2);
    }

    #[test]
    fn the_printing_shown_is_the_highest_one_played() {
        // S8-1: a newer printing of the card that was never played is not
        // the one shown.
        let (_dir, conn) = db("card-stats-played-printing");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 120, "q-oracle-bolt", "Qbolt old", "Instant");
        card(&conn, 121, "q-oracle-bolt", "Qbolt", "Instant");
        card(&conn, 130, "q-oracle-bolt", "Qbolt unplayed", "Instant");
        let g = game(&conn, complete(&conn, v), "win", None);
        local(&conn, g, 120, &[]);
        local(&conn, g, 121, &[]);

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.catalog_id, 121);
        assert_eq!(row.name, "Qbolt");
    }

    #[test]
    fn the_deck_prior_under_a_board_filter_counts_only_games_with_card_rows() {
        // S8-2 (0.x `GetFilteredDeckWinrate`: a board filter is an `EXISTS`
        // on the game's card rows).
        let (_dir, conn) = db("card-stats-prior-rows");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 118, "q-oracle-board", "Qboard", "Instant");
        let m = complete(&conn, v);
        let g1 = game(&conn, m, "win", None);
        let g2 = game(&conn, m, "win", None);
        let _g3_without_rows = game(&conn, m, "loss", None);
        local(&conn, g1, 118, &[]);
        local(&conn, g2, 118, &[]);

        let post = CardStatsFilter {
            board: Some(Board::Postboard),
            ..CardStatsFilter::all()
        };
        let prior = deck_prior(&conn, deck_id, &post).expect("prior");
        assert_eq!((prior.wins, prior.games), (1, 1));
        // No board filter: every game, as 0.x.
        let prior = deck_prior(&conn, deck_id, &CardStatsFilter::all()).expect("prior");
        assert_eq!((prior.wins, prior.games), (2, 3));
    }

    #[test]
    fn an_imported_matchs_postboard_zero_row_stays_but_its_token_does_not() {
        let (_dir, conn) = db("card-stats-imported-zero-rows");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "o-main", "Qmain", "Creature");
        card(&conn, 102, "o-side", "Qside", "Sorcery");
        card(&conn, 900, "o-token", "Qtreasure", "Token Artifact");
        let m = match_with(&conn, v, "imported", "complete", 1_000);
        let _game1 = game(&conn, m, "win", None);
        let game2 = game(&conn, m, "win", None);
        local(&conn, game2, 101, &[]);
        // 0.x's zero-quantity row for a sideboard card that stayed in the sideboard.
        local(&conn, game2, 102, &[("main_qty", 0), ("seen", 0)]);
        local(&conn, game2, 900, &[("main_qty", 0), ("seen", 1)]);

        let names: Vec<String> = read(&conn, deck_id, &CardStatsFilter::all())
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["Qmain".to_string(), "Qside".to_string()]);
    }

    /// A log match's game 1 with the deck card 101 (4 copies, 3 seen, 2
    /// kept, 1 cast) and a row MTGO logged under the variant 191 (no deck
    /// quantity, seen 1, cast 2, 2 warp casts); game 2 has the deck card
    /// sided out and a variant cast. `variant_oracle` is 191's oracle id.
    fn log_variant_games(conn: &Connection, variant_oracle: Option<&str>) -> i64 {
        let deck_id = deck(conn);
        let v = version(conn, deck_id);
        card(conn, 101, "o-main", "Qmain", "Creature");
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES (191, ?1, 'Qmain')",
            [variant_oracle],
        )
        .expect("variant");
        let m = complete(conn, v);
        let g1 = game(conn, m, "win", None);
        local(conn, g1, 101, &[("cast", 1)]);
        local(
            conn,
            g1,
            191,
            &[("main_qty", 0), ("kept", 0), ("seen", 1), ("cast", 2)],
        );
        let g2 = game(conn, m, "loss", None);
        local(
            conn,
            g2,
            101,
            &[("main_qty", 0), ("kept", 0), ("seen", 0), ("sided_out", 4)],
        );
        local(
            conn,
            g2,
            191,
            &[("main_qty", 0), ("kept", 0), ("seen", 1), ("cast", 1)],
        );
        conn.execute(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES (?1, 'local', 191, 'warp', 2), (?2, 'local', 191, 'warp', 1)",
            [g1, g2],
        )
        .expect("casts");
        deck_id
    }

    #[test]
    fn a_log_matchs_variant_casts_count_for_the_deck_card() {
        // Issue #319: MTGO logs some casts under another printing's catalog
        // id (warp). 0.x widened the deck's catalog to oracle map with every
        // printing the player touched, so those casts land on the deck
        // card's oracle row, and only where the card is in the deck that
        // game (a sided-out card's row has no counters). The variant's seen
        // copies are the deck card's (one instance, two catalog ids), so
        // seen takes the larger; the printing shown stays the deck's.
        let (_dir, conn) = db("card-stats-log-variant");
        let deck_id = log_variant_games(&conn, Some("o-main"));

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.total_games, 2);
        assert_eq!(row.total_possible, 4);
        assert_eq!((row.total_cast, row.cast_games, row.cast_won), (3, 1, 1));
        assert_eq!(row.total_warp, 2);
        assert_eq!((row.total_seen, row.seen_games), (3, 1));
        assert_eq!((row.total_kept, row.kept_games), (2, 1));
        assert_eq!(row.sided_out_games, 1);
        assert_eq!(row.catalog_id, 101);
    }

    #[test]
    fn a_variant_whose_oracle_arrives_later_counts_as_if_known_up_front() {
        // Live equals rebuild (#319): the fold is read from the stored rows
        // and the card data, so a variant resolved after its game was synced
        // reads the same as one known when the game was synced.
        let (_dir_known, known) = db("card-stats-log-variant-known");
        let known_deck = log_variant_games(&known, Some("o-main"));
        let (_dir_late, late) = db("card-stats-log-variant-late");
        let late_deck = log_variant_games(&late, None);

        // Nameless, so the face name rule (#320) cannot resolve it either.
        late.execute("UPDATE cards SET name = NULL WHERE catalog_id = 191", [])
            .expect("nameless");
        let before = read(&late, late_deck, &CardStatsFilter::all());
        let main = before.iter().find(|s| s.key == "o-main").expect("main");
        assert_eq!(main.total_cast, 1, "an unresolved variant is its own card");
        late.execute(
            "UPDATE cards SET oracle_id = 'o-main', name = 'Qmain' WHERE catalog_id = 191",
            [],
        )
        .expect("resolve");

        assert_eq!(
            read(&late, late_deck, &CardStatsFilter::all()),
            read(&known, known_deck, &CardStatsFilter::all())
        );
    }

    /// Issue #320: a multi-face card (adventure, omen, disturb) logs its
    /// face's actions under the face's own catalog id, whose `cards` row is
    /// a stub with the log's face name and no oracle id. 0.x
    /// (`resolveFaceNamesToOracle`) resolved it to the oracle of the card
    /// with a face of that name, so the casts count for the deck card. A
    /// face of a card not in the deck that game, or a name no card has,
    /// stays out of the deck's rows.
    #[test]
    fn a_face_logged_under_its_own_id_counts_for_the_deck_card() {
        let (_dir, conn) = db("card-stats-face-name");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "o-main", "Qmain // Qface", "Creature");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name) VALUES (192, 'Qface'), (193, 'Qnothing'),
                 (194, 'Qother');
             INSERT INTO cards (catalog_id, oracle_id, name) VALUES (301, 'o-other', 'Qother // Qx');",
        )
        .expect("cards");
        let m = complete(&conn, v);
        let g1 = game(&conn, m, "win", None);
        local(&conn, g1, 101, &[("cast", 1)]);
        let face = [("main_qty", 0), ("kept", 0), ("seen", 1), ("cast", 2)];
        local(&conn, g1, 192, &face);
        local(&conn, g1, 193, &face);
        local(&conn, g1, 194, &face);
        conn.execute(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES (?1, 'local', 192, 'warp', 2), (?1, 'local', 193, 'warp', 2),
                    (?1, 'local', 194, 'warp', 2)",
            [g1],
        )
        .expect("casts");

        let row = only(&conn, deck_id, &CardStatsFilter::all());
        assert_eq!(row.key, "o-main");
        assert_eq!((row.total_cast, row.total_warp), (3, 2));
        assert_eq!((row.total_seen, row.total_kept), (3, 2));
        assert_eq!(row.catalog_id, 101, "the printing shown stays the deck's");
    }

    #[test]
    fn face_names_resolve_like_0x() {
        let (_dir, conn) = db("card-stats-face-oracle");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES
                 (1, 'o-adventure', 'Qgiant // Qstomp'),
                 (2, 'o-three', 'Qa // Qb // Qc'),
                 (3, 'o-plain', 'Qbolt'),
                 (4, 'o-case', 'Qcase // QSTOMPY');
             INSERT INTO cards (catalog_id, name) VALUES
                 (11, 'Qstomp'), (12, 'Qgiant'), (13, 'Qb'), (14, 'Qbolt'), (15, 'Qstompy'),
                 (16, 'Qsto'), (17, '');
             INSERT INTO cards (catalog_id, oracle_id, name) VALUES (18, 'o-own', 'Qstomp');",
        )
        .expect("cards");
        let mut names = FaceNames::new();
        // v1 also finds a middle face, which 0.x's SQL prefilter could not
        // (the traps doc; no real three-face card is known).
        let resolved: Vec<Option<String>> = [11, 12, 13, 14, 15, 16, 17, 18, 99]
            .iter()
            .map(|id| {
                names
                    .oracle(&conn, *id)
                    .expect("read")
                    .map(|(oracle, _)| oracle)
            })
            .collect();
        let expected: Vec<Option<String>> = [
            Some("o-adventure"),
            Some("o-adventure"),
            Some("o-three"),
            Some("o-plain"),
            None,
            None,
            None,
            None,
            None,
        ]
        .iter()
        .map(|o| o.map(str::to_string))
        .collect();
        assert_eq!(resolved, expected);
    }

    /// Faces resolve where 0.x resolved them (#320 review): a local deck
    /// row that is an oracle-less stub keeps its own key (0.x's deck
    /// quantities came only from deck cards with card data), and an
    /// opponent's face counts for the card whose face it is.
    #[test]
    fn faces_resolve_for_local_non_deck_rows_and_opponent_rows_only() {
        let (_dir, conn) = db("card-stats-face-sides");
        let deck_id = deck(&conn);
        let v = version(&conn, deck_id);
        card(&conn, 101, "o-main", "Qmain // Qface", "Creature");
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (192, 'Qface')",
            [],
        )
        .expect("stub");
        let g = game(&conn, complete(&conn, v), "win", None);
        local(&conn, g, 101, &[]);
        local(&conn, g, 192, &[("main_qty", 1)]);
        stat(&conn, g, Side::Opponent, 192, &[("seen", 1), ("cast", 1)]);

        let mine = read(&conn, deck_id, &CardStatsFilter::all());
        let keys: Vec<&str> = mine.iter().map(|stat| stat.key.as_str()).collect();
        assert_eq!(mine.len(), 2, "the deck row stub is its own card: {keys:?}");
        let main = mine.iter().find(|stat| stat.key == "o-main").expect("main");
        assert_eq!(main.total_possible, 4);

        let theirs = CardStatsFilter {
            side: Side::Opponent,
            ..CardStatsFilter::all()
        };
        let row = only(&conn, deck_id, &theirs);
        assert_eq!(row.key, "o-main");
        assert_eq!(row.catalog_id, 101, "shown as the card, not the stub");
    }
}
