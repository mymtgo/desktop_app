//! Reading JSON that MTGO wrote without its closing brackets (0.x
//! `RepairJson::firstObject`).
//!
//! Some MTGO payloads miss closing brackets: the draft PendingPick message has
//! no outer `}`, and every tournament round info line misses exactly one.
//! `first_object` decodes the first JSON object in the line and, when that
//! fails, retries with up to three of the missing closers added, read from the
//! text's own bracket balance.

use serde_json::Value;

/// At most this many missing closing brackets are added back.
const MAX_PADDING: usize = 3;

/// The first JSON object in a log line: from the first `{` to the last `}`
/// (or the end of the text when no `}` follows), tolerating up to three
/// missing closing brackets. `None` when there is no `{` or the text cannot be
/// repaired.
pub fn first_object(text: &str) -> Option<Value> {
    first_object_padded(text, MAX_PADDING)
}

/// `first_object` adding back at most `max_padding` missing closers. A reader
/// that knows how many its lines miss passes that number, so a line cut
/// short deeper inside does not decode as a part of itself.
pub fn first_object_padded(text: &str, max_padding: usize) -> Option<Value> {
    let start = text.find('{')?;
    let candidate = match text.rfind('}') {
        Some(end) if end >= start => &text[start..=end],
        _ => &text[start..],
    };
    for pad in 0..=max_padding {
        let attempt = format!("{candidate}{}", closers(candidate, pad));
        if let Ok(value) = serde_json::from_str::<Value>(&attempt)
            && (value.is_object() || value.is_array())
        {
            return Some(value);
        }
    }
    None
}

/// The closing brackets for the innermost `count` unclosed brackets of
/// `candidate`, innermost first. Brackets inside strings are ignored.
fn closers(candidate: &str, count: usize) -> String {
    if count == 0 {
        return String::new();
    }
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    for ch in candidate.chars() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let keep_from = stack.len().saturating_sub(count);
    stack[keep_from..].iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_complete_object_after_text() {
        let value = first_object("12:00:00 [INF] Message: {\"A\":1,\"B\":[2]} Receiver: X")
            .expect("object");
        assert_eq!(value["A"], 1);
        assert_eq!(value["B"][0], 2);
    }

    #[test]
    fn repairs_a_missing_outer_brace() {
        let text = "Message: {\"DraftToken\":\"t\",\"PendingPick\":{\"CardsAvailable\":[1,2],\"PackID\":9} Receiver: X";
        let value = first_object(text).expect("repaired");
        assert_eq!(value["PendingPick"]["PackID"], 9);
        assert_eq!(value["DraftToken"], "t");
    }

    #[test]
    fn repairs_up_to_three_missing_brackets_in_order() {
        let value = first_object("x {\"A\":{\"B\":[1").expect("repaired");
        assert_eq!(value["A"]["B"][0], 1);
    }

    #[test]
    fn ignores_brackets_inside_strings() {
        let value = first_object("x {\"A\":\"{[\",\"B\":{\"C\":2}").expect("repaired");
        assert_eq!(value["A"], "{[");
        assert_eq!(value["B"]["C"], 2);
    }

    #[test]
    fn a_padding_limit_refuses_deeper_cuts() {
        assert!(first_object_padded("x {\"A\":{\"B\":1}", 1).is_some());
        assert_eq!(first_object_padded("x {\"A\":{\"B\":[1", 1), None);
    }

    #[test]
    fn gives_none_without_an_object_or_when_beyond_repair() {
        assert_eq!(first_object("no json here"), None);
        assert_eq!(first_object("x {\"A\":{\"B\":{\"C\":{\"D\":1"), None);
        assert_eq!(first_object("x {\"A\":"), None);
    }
}
