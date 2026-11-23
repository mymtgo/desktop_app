//! Card name keys (the API `App\Support\CardName`): the server's rules name
//! cards as written, and both sides compare normalised keys.

/// Trimmed, inner whitespace collapsed to one space, lower case
/// (`CardName::normalize`).
pub fn name_key(name: &str) -> String {
    let words: Vec<&str> = name.split_ascii_whitespace().collect();
    words.join(" ").to_lowercase()
}

/// Every lookup key of a card name: the full key and, for a split or double
/// faced name ("A // B"), the front face on its own (`CardName::faces`).
pub fn name_keys(name: &str) -> Vec<String> {
    let full = name_key(name);
    let mut keys = vec![full.clone()];
    if full.contains("//") {
        let front_text = name.split("//").next().unwrap_or("");
        let front = name_key(front_text);
        if !front.is_empty() && !keys.contains(&front) {
            keys.push(front);
        }
    }
    keys
}
