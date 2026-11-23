//! The capture file: a header line, then one JSON line per captured frame.

use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    Match,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Human,
    Bot,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CaptureHeader {
    #[serde(rename = "mymtgo_endstep_capture")]
    pub format: u32,
    pub unit_id: String,
    pub kind: UnitKind,
    pub started_at: i64,
    pub counter: i64,
    pub extension: String,
    pub browser: String,
    pub developer: bool,
    pub classification: Classification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCapture {
    pub header: CaptureHeader,
    pub frame_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    Empty,
    /// The first line is not a valid header: the reason.
    Header(String),
    UnsupportedFormat(u32),
    BadUnitId(String),
    /// A frame line (1-based line number) that is not valid: the reason.
    Line(usize, String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Empty => write!(f, "capture is empty"),
            CaptureError::Header(why) => write!(f, "line 1: bad header: {why}"),
            CaptureError::UnsupportedFormat(n) => {
                write!(f, "line 1: unsupported capture format {n}")
            }
            CaptureError::BadUnitId(id) => write!(f, "line 1: bad unit id {id:?}"),
            CaptureError::Line(n, why) => write!(f, "line {n}: {why}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Checks the header, then every frame line. Frame bodies are not inspected.
pub fn parse_capture(text: &str) -> Result<ParsedCapture, CaptureError> {
    let mut lines = text.lines().enumerate();
    let first = match lines.next() {
        Some((_, line)) if !line.trim().is_empty() => line,
        _ => return Err(CaptureError::Empty),
    };
    // The format number is read first: a newer format may have other fields,
    // so it must be refused as a format, not as a bad header.
    let value: serde_json::Value =
        serde_json::from_str(first).map_err(|e| CaptureError::Header(e.to_string()))?;
    let format = value
        .get("mymtgo_endstep_capture")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| CaptureError::Header("missing capture format number".to_string()))?;
    if format != u64::from(FORMAT_VERSION) {
        return Err(CaptureError::UnsupportedFormat(
            u32::try_from(format).unwrap_or(u32::MAX),
        ));
    }
    let header: CaptureHeader =
        serde_json::from_value(value).map_err(|e| CaptureError::Header(e.to_string()))?;
    if !crate::message::valid_unit_id(&header.unit_id) {
        return Err(CaptureError::BadUnitId(header.unit_id));
    }

    let mut frame_count = 0u32;
    for (index, line) in lines {
        if line.trim().is_empty() {
            continue;
        }
        check_frame(line).map_err(|why| CaptureError::Line(index + 1, why))?;
        frame_count += 1;
    }
    Ok(ParsedCapture {
        header,
        frame_count,
    })
}

fn check_frame(line: &str) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let object = value.as_object().ok_or("not a JSON object")?;
    if object.get("at").and_then(|v| v.as_i64()).is_none() {
        return Err("missing integer \"at\"".to_string());
    }
    match object.get("src").and_then(|v| v.as_str()) {
        Some("game_ws" | "lobby_ws" | "rest") => {}
        _ => return Err("\"src\" must be game_ws, lobby_ws or rest".to_string()),
    }
    match object.get("dir").and_then(|v| v.as_str()) {
        Some("in" | "out") => {}
        _ => return Err("\"dir\" must be in or out".to_string()),
    }
    if object.get("url").and_then(|v| v.as_str()).is_none() {
        return Err("missing string \"url\"".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(format: u32) -> String {
        format!(
            r#"{{"mymtgo_endstep_capture":{format},"unit_id":"0f8fad5b-d9cb-469f-a165-70867728950e","kind":"match","started_at":1700000000000,"counter":3,"extension":"0.1.0","browser":"firefox","developer":false,"classification":"human"}}"#
        )
    }

    const FRAME_A: &str = r#"{"at":1,"src":"game_ws","dir":"in","url":"wss://x","body":{"a":1}}"#;
    const FRAME_B: &str = r#"{"at":2,"src":"rest","dir":"out","url":"https://x","text":"hi"}"#;

    #[test]
    fn a_capture_parses() {
        let text = format!("{}\n{FRAME_A}\n{FRAME_B}\n", header(1));
        let parsed = parse_capture(&text).unwrap();
        assert_eq!(parsed.frame_count, 2);
        assert_eq!(parsed.header.kind, UnitKind::Match);
        assert_eq!(parsed.header.counter, 3);
        assert_eq!(parsed.header.started_at, 1_700_000_000_000);
        assert_eq!(parsed.header.browser, "firefox");
        assert!(!parsed.header.developer);
        assert_eq!(parsed.header.classification, Classification::Human);
    }

    #[test]
    fn a_capture_with_a_bad_line_names_it() {
        let text = format!(
            "{}\n{FRAME_A}\n{}\n",
            header(1),
            r#"{"src":"rest","dir":"in","url":"u"}"#
        );
        let err = parse_capture(&text).unwrap_err();
        assert!(err.to_string().contains("line 3"), "{err}");
    }

    #[test]
    fn a_newer_format_is_refused() {
        let text = format!("{}\n{FRAME_A}\n", header(2));
        assert_eq!(
            parse_capture(&text).unwrap_err(),
            CaptureError::UnsupportedFormat(2)
        );
    }

    #[test]
    fn a_newer_format_is_refused_even_with_other_fields() {
        let text = r#"{"mymtgo_endstep_capture":2,"something_new":true}"#;
        assert_eq!(
            parse_capture(text).unwrap_err(),
            CaptureError::UnsupportedFormat(2)
        );
    }

    #[test]
    fn a_header_without_a_format_number_is_bad() {
        let err = parse_capture(r#"{"unit_id":"x"}"#).unwrap_err();
        assert!(matches!(err, CaptureError::Header(_)), "{err}");
    }

    #[test]
    fn a_bad_unit_id_is_refused() {
        let text = format!(
            "{}\n{FRAME_A}\n",
            header(1).replace("0f8fad5b-d9cb-469f-a165-70867728950e", "../../x")
        );
        assert_eq!(
            parse_capture(&text).unwrap_err(),
            CaptureError::BadUnitId("../../x".to_string())
        );
    }
}
