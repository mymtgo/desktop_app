//! The Opponents page command (0.x `Opponents\IndexController`): the current
//! account's opponents with their records, searched, sorted, filtered by
//! format and paged. The numbers come from `store::opponents`.

use serde::{Deserialize, Serialize};
use store::opponents::{
    FormatOption, OpponentRow, OpponentSort, OpponentsFilter, OpponentsPage, opponents_page,
};
use store::{Connection, StoreError};
use tauri::State;

use crate::bridge::lock;
use crate::commands::{CommandError, Startup, ready, storage};
use crate::decks::{RecordView, record_view};
use crate::logging::Timer;

/// The sort select (0.x `sort`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum OpponentSortKey {
    #[default]
    MostPlayed,
    WinrateDesc,
    WinrateAsc,
    MostRecent,
}

fn store_sort(sort: OpponentSortKey) -> OpponentSort {
    match sort {
        OpponentSortKey::MostPlayed => OpponentSort::MostPlayed,
        OpponentSortKey::WinrateDesc => OpponentSort::WinRateDesc,
        OpponentSortKey::WinrateAsc => OpponentSort::WinRateAsc,
        OpponentSortKey::MostRecent => OpponentSort::MostRecent,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OpponentView {
    pub opponent_id: u32,
    pub username: String,
    pub record: RecordView,
    /// Display names, sorted.
    pub formats: Vec<String>,
    /// UTC unix milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub last_played_at_ms: i64,
    /// The archetypes the opponent was seen with (0.x `archetypes`).
    pub archetypes: Vec<SeenArchetypeView>,
}

/// An archetype an opponent was seen with: its name and colours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SeenArchetypeView {
    pub name: String,
    /// Colour identity letters in WUBRG order.
    pub colors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OpponentFormatView {
    /// What the `format` argument takes.
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OpponentsView {
    pub rows: Vec<OpponentView>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    pub formats: Vec<OpponentFormatView>,
}

fn row_view(row: &OpponentRow) -> OpponentView {
    OpponentView {
        opponent_id: u32::try_from(row.opponent_id).unwrap_or(0),
        username: row.username.clone(),
        record: record_view(&row.record),
        formats: row.formats.clone(),
        last_played_at_ms: row.last_played_at_ms,
        archetypes: row
            .archetypes
            .iter()
            .map(|archetype| SeenArchetypeView {
                name: archetype.name.clone(),
                colors: crate::summary::archetype_colors(archetype.color_identity.as_deref()),
            })
            .collect(),
    }
}

pub fn page_view(page: &OpponentsPage) -> OpponentsView {
    OpponentsView {
        rows: page.rows.iter().map(row_view).collect(),
        total: page.total,
        page: page.page,
        per_page: page.per_page,
        formats: page.formats.iter().map(format_view).collect(),
    }
}

fn format_view(option: &FormatOption) -> OpponentFormatView {
    OpponentFormatView {
        code: option.code.clone(),
        name: option.name.clone(),
    }
}

pub fn list(
    conn: &Connection,
    search: Option<String>,
    sort: OpponentSortKey,
    format: Option<String>,
    page: u32,
) -> Result<OpponentsView, StoreError> {
    let scope = crate::platform::page_scope(conn)?;
    let filter = OpponentsFilter {
        search,
        sort: store_sort(sort),
        format,
        page,
    };
    Ok(page_view(&opponents_page(
        conn,
        scope.account,
        scope.platform,
        &filter,
    )?))
}

/// `format` is a code from the page's `formats` options.
#[tauri::command]
#[specta::specta]
pub fn opponents(
    search: Option<String>,
    sort: OpponentSortKey,
    format: Option<String>,
    page: u32,
    startup: State<'_, Startup>,
) -> Result<OpponentsView, CommandError> {
    let _timer = Timer::start("opponents");
    let state = ready(&startup)?;
    list(&lock(&state.reader), search, sort, format, page).map_err(storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use decks::stats::Record;

    #[test]
    fn sort_keys_map_and_default_to_most_played() {
        assert_eq!(OpponentSortKey::default(), OpponentSortKey::MostPlayed);
        assert_eq!(
            store_sort(OpponentSortKey::WinrateAsc),
            OpponentSort::WinRateAsc
        );
    }

    #[test]
    fn rows_carry_the_record_label() {
        let page = OpponentsPage {
            rows: vec![OpponentRow {
                opponent_id: 3,
                username: "Qann".into(),
                record: Record::from_counts(2, 1, 1),
                formats: vec!["Modern".into()],
                last_played_at_ms: 5,
                archetypes: vec![store::opponents::OpponentArchetype {
                    name: "Qburn".into(),
                    color_identity: Some("R,G".into()),
                }],
            }],
            total: 1,
            page: 1,
            per_page: 25,
            formats: Vec::new(),
        };
        let view = page_view(&page);
        assert_eq!(view.rows[0].record.label, "2 - 1 - 1");
        assert_eq!(view.rows[0].record.win_rate, 50);
        assert_eq!(view.rows[0].opponent_id, 3);
        // 0.x `archetypes`: name and colour identity.
        assert_eq!(
            view.rows[0].archetypes,
            vec![SeenArchetypeView {
                name: "Qburn".to_string(),
                colors: vec!["R".to_string(), "G".to_string()],
            }]
        );
    }
}
