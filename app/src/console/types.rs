//! The console's wire types: the `store::console` types as the page sees
//! them. The store types carry no serde, so the shapes sent to the page are
//! decided here.

use serde::{Deserialize, Serialize};

/// A named parameter's value as the user typed it (`None`: SQL `NULL`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleParam {
    pub name: String,
    pub value: Option<String>,
}

impl From<ConsoleParam> for store::console::ConsoleParam {
    fn from(param: ConsoleParam) -> Self {
        store::console::ConsoleParam {
            name: param.name,
            value: param.value,
        }
    }
}

/// One cell: `{ "kind": "integer", "value": 3 }`; a null has no value and a
/// blob is its length in bytes. A real that is not finite (NaN, infinity)
/// is sent as a null, as the export writes it, since JSON cannot hold it.
/// An integer a JavaScript number cannot hold exactly (beyond 2^53 - 1) is
/// sent as its digits (`bigInteger`), so the grid shows the exact value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ConsoleValue {
    Null,
    Integer(#[specta(type = specta_typescript::Number)] i64),
    BigInteger(String),
    Real(f64),
    Text(String),
    Blob(#[specta(type = specta_typescript::Number)] u64),
}

/// JavaScript's `Number.MAX_SAFE_INTEGER`: the largest integer a JSON number
/// keeps exactly once the page parses it.
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

impl From<store::console::ConsoleValue> for ConsoleValue {
    fn from(value: store::console::ConsoleValue) -> Self {
        use store::console::ConsoleValue as Store;
        match value {
            Store::Null => ConsoleValue::Null,
            Store::Integer(number) if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&number) => {
                ConsoleValue::Integer(number)
            }
            Store::Integer(number) => ConsoleValue::BigInteger(number.to_string()),
            Store::Real(number) if number.is_finite() => ConsoleValue::Real(number),
            Store::Real(_) => ConsoleValue::Null,
            Store::Text(text) => ConsoleValue::Text(text),
            Store::Blob(length) => ConsoleValue::Blob(u64::try_from(length).unwrap_or(u64::MAX)),
        }
    }
}

/// A finished query: at most the row cap, `truncated` when more were left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<ConsoleValue>>,
    pub truncated: bool,
    #[specta(type = specta_typescript::Number)]
    pub elapsed_ms: u64,
}

impl From<store::console::ConsoleResult> for ConsoleResult {
    fn from(result: store::console::ConsoleResult) -> Self {
        ConsoleResult {
            columns: result.columns,
            rows: result
                .rows
                .into_iter()
                .map(|row| row.into_iter().map(ConsoleValue::from).collect())
                .collect(),
            truncated: result.truncated,
            elapsed_ms: result.elapsed_ms,
        }
    }
}

/// Whether a listed object is a table or a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    View,
    Table,
}

/// One column of a listed object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleColumn {
    pub name: String,
    /// The declared type; empty when SQLite has none (most view columns).
    pub decl_type: String,
    /// The one-line help of a `v_*` column; `None` for raw columns.
    pub description: Option<String>,
}

/// One table or view the console can read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleObject {
    pub name: String,
    pub kind: ObjectKind,
    /// A raw table or view of the database, not one of the `v_*` views.
    pub raw: bool,
    pub columns: Vec<ConsoleColumn>,
}

impl From<store::console::ConsoleObject> for ConsoleObject {
    fn from(object: store::console::ConsoleObject) -> Self {
        ConsoleObject {
            name: object.name,
            kind: match object.kind {
                store::console::ObjectKind::View => ObjectKind::View,
                store::console::ObjectKind::Table => ObjectKind::Table,
            },
            raw: object.raw,
            columns: object
                .columns
                .into_iter()
                .map(|column| ConsoleColumn {
                    name: column.name,
                    decl_type: column.decl_type,
                    description: column.description,
                })
                .collect(),
        }
    }
}

/// The export file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Csv,
    Json,
}

impl From<ExportFormat> for store::console::ExportFormat {
    fn from(format: ExportFormat) -> Self {
        match format {
            ExportFormat::Csv => store::console::ExportFormat::Csv,
            ExportFormat::Json => store::console::ExportFormat::Json,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_sent_tagged_by_kind() {
        let row = vec![
            ConsoleValue::from(store::console::ConsoleValue::Null),
            ConsoleValue::from(store::console::ConsoleValue::Integer(3)),
            ConsoleValue::from(store::console::ConsoleValue::Real(0.5)),
            ConsoleValue::from(store::console::ConsoleValue::Real(f64::NAN)),
            ConsoleValue::from(store::console::ConsoleValue::Text("Mono Red".to_string())),
            ConsoleValue::from(store::console::ConsoleValue::Blob(12)),
        ];
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            serde_json::json!([
                { "kind": "null" },
                { "kind": "integer", "value": 3 },
                { "kind": "real", "value": 0.5 },
                { "kind": "null" },
                { "kind": "text", "value": "Mono Red" },
                { "kind": "blob", "value": 12 }
            ])
        );
    }

    #[test]
    fn an_integer_past_the_safe_range_is_sent_as_digits() {
        let values = [9_007_199_254_740_991, 9_007_199_254_740_993, i64::MIN]
            .map(|number| ConsoleValue::from(store::console::ConsoleValue::Integer(number)));
        assert_eq!(
            serde_json::to_value(values).unwrap(),
            serde_json::json!([
                { "kind": "integer", "value": 9_007_199_254_740_991_i64 },
                { "kind": "bigInteger", "value": "9007199254740993" },
                { "kind": "bigInteger", "value": "-9223372036854775808" }
            ])
        );
    }
}
