//! Keep player names and the home folder out of anything that leaves the
//! machine (spec 9: "usernames scrubbed before send"). Names come from
//! storage, from every match the tracker pushes, and from MTGO text inside
//! the report itself, which reveals names the app has not stored yet.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use mtgo_protocol::anonymise::detect_names;
use regex::{Regex, RegexBuilder};
use serde_json::Value;

pub const PLAYER: &str = "<player>";
/// Shorter names are not replaced: they would match ordinary words. The
/// anonymiser uses the same floor.
const MIN_NAME_LEN: usize = 3;

static NAMES: RwLock<BTreeSet<String>> = RwLock::new(BTreeSet::new());

thread_local! {
    /// True while this thread is inside `remember`, holding the write lock.
    static REMEMBERING: Cell<bool> = const { Cell::new(false) };
}

/// Add player names to the list every report is scrubbed of.
pub fn remember<I: IntoIterator<Item = String>>(names: I) {
    REMEMBERING.with(|flag| flag.set(true));
    {
        let mut known = NAMES
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for name in names {
            if name.len() >= MIN_NAME_LEN {
                known.insert(name);
            }
        }
    }
    REMEMBERING.with(|flag| flag.set(false));
}

/// The names known so far. Waits for other threads to finish with the list.
/// `None` only when this thread is inside `remember`: a report raised there
/// must not wait for itself.
pub fn known() -> Option<Vec<String>> {
    if REMEMBERING.with(|flag| flag.get()) {
        return None;
    }
    let names = NAMES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Some(names.iter().cloned().collect())
}

/// Replace every known name, and every name `text` itself reveals, with
/// `<player>`, and the home folder and OS user name with `~` and `<user>`
/// (see `hide_home`).
pub fn scrub_text(text: &str, known: &[String], home: Option<&str>) -> String {
    scrub_text_with(&PATTERN_CACHE, text, known, home)
}

/// `scrub_text` with the known-list pattern kept in `cache`.
fn scrub_text_with(
    cache: &PatternCache,
    text: &str,
    known: &[String],
    home: Option<&str>,
) -> String {
    let home = HomePatterns::new(home);
    let text = hide_secrets(text);
    replace_names(&home.hide(&text), &name_patterns(cache, known, &text))
}

/// `scrub_text` for every string inside a JSON value (a whole Sentry report).
pub fn scrub_json(value: &mut Value, known: &[String], home: Option<&str>) {
    let mut strings = Vec::new();
    collect_strings(value, &mut strings);
    let names = name_patterns(&PATTERN_CACHE, known, &strings.join("\n"));
    scrub_strings(value, &names, &HomePatterns::new(home));
}

/// Hide API secrets: the value after `Bearer`, after an `X-Api-Key` header
/// or field, and of `access_token`, `refresh_token` and `api_key` fields
/// (JSON or form) becomes `<secret>`. Every report goes through it, so a
/// header or token answer quoted in an error never reaches Sentry.
pub fn hide_secrets(text: &str) -> String {
    let mut out = text.to_string();
    for pattern in [&*BEARER, &*API_KEY_HEADER, &*JSON_SECRET, &*FORM_SECRET] {
        out = pattern.replace_all(&out, "${1}<secret>").into_owned();
    }
    out
}

/// `Bearer <token>`: group 1 is the word and the space.
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\bbearer\s+)[A-Za-z0-9\-._~+/=]+").expect("valid regex"));

/// `X-Api-Key: <key>`, `x-api-key=<key>` or the tuple `Debug` form
/// `("X-Api-Key", "<key>")`, quoted or not.
static API_KEY_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(x-api-key"?\s*[:=,]\s*"?)[^\s",;&)]+"#).expect("valid regex")
});

/// `"access_token": "<value>"` and the same for `refresh_token`, `api_key`.
static JSON_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)("(?:access_token|refresh_token|api_key)"\s*:\s*")[^"]*"#)
        .expect("valid regex")
});

/// `access_token=<value>` in a form body or query; `grant_type=refresh_token`
/// is a value, not a field, and stays.
static FORM_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)((?:^|[?&\s])(?:access_token|refresh_token|api_key)=)[^&\s]+")
        .expect("valid regex")
});

/// Hide the OS user name. In order: the home folder (either slash, any case)
/// becomes `~`; any Windows user folder (`C:\Users\<anything>`, which covers
/// 8.3 short names like `QHOMEU~1` and names with spaces) becomes `~`; a Wine
/// user folder (`drive_c/users/<name>`) keeps its shape with `<user>` as the
/// name, except the shared `steamuser` and `Public`; finally the home folder's
/// last part (the user name, 3 bytes or more) becomes `<user>` wherever it
/// still shows as a word of its own. Another person's `/home/<name>` is left
/// alone.
pub fn hide_home(text: &str, home: Option<&str>) -> String {
    HomePatterns::new(home).hide(text)
}

/// The patterns for one home folder, built once per report rather than once
/// per string.
struct HomePatterns {
    /// The home folder itself; `None` without a home, or for a home of `/`.
    home: Option<Regex>,
    /// The OS user name as a word of its own.
    name: Option<Regex>,
}

impl HomePatterns {
    fn new(home: Option<&str>) -> Self {
        // A home with no folder name in it (`/`) would match every slash.
        let home = home.filter(|home| !home.trim_matches(['/', '\\']).is_empty());
        HomePatterns {
            home: home.map(home_regex),
            name: home.and_then(user_name_of).map(|name| name_regex(&name)),
        }
    }

    fn hide(&self, text: &str) -> String {
        let mut out = text.to_string();
        if let Some(home) = &self.home {
            out = home.replace_all(&out, "~").into_owned();
        }
        out = WINDOWS_USER_DIR
            .replace_all(&out, |caps: &regex::Captures| {
                // Keep the slash that ended a folder name with spaces.
                format!("~{}", caps.get(1).map_or("", |slash| slash.as_str()))
            })
            .into_owned();
        out = WINE_USER_DIR
            .replace_all(&out, |caps: &regex::Captures| {
                let name = caps[2].to_lowercase();
                if name == "steamuser" || name == "public" {
                    caps[0].to_string()
                } else {
                    format!("{}<user>", &caps[1])
                }
            })
            .into_owned();
        if let Some(name) = &self.name {
            out = name.replace_all(&out, "<user>").into_owned();
        }
        out
    }
}

/// `C:\Users\<name>`, with either slash and any case. A name with spaces is
/// taken up to the next slash (group 1); without a slash after it, the name
/// stops at the first space.
static WINDOWS_USER_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)[a-z]:[\\/]users[\\/](?:[^\\/"'<>|\r\n]+([\\/])|[^\\/\s"'<>|]+)"#)
        .expect("valid regex")
});

/// `drive_c/users/<name>`: group 1 is the folder prefix, group 2 the name.
static WINE_USER_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(drive_c[\\/]users[\\/])([^\\/\s"'<>|]+)"#).expect("valid regex")
});

/// The home folder as a case-insensitive pattern that accepts either slash
/// wherever the home has one.
fn home_regex(home: &str) -> Regex {
    let parts: Vec<String> = home.split(['/', '\\']).map(regex::escape).collect();
    Regex::new(&format!("(?i){}", parts.join(r"[\\/]"))).expect("escaped pattern is valid")
}

/// The user name, any case, as a word of its own: `deck` is hidden, but not
/// the `deck` in `deck_used` or `DeckUsed`.
fn name_regex(name: &str) -> Regex {
    Regex::new(&format!("(?i){}", whole_word(name))).expect("escaped pattern is valid")
}

/// `name` escaped, with a word boundary at each end that is a word character,
/// so it matches only as a word of its own. An end that is not a word
/// character (a name ending in `.`) needs none. The boundary is ASCII
/// (`(?-u:\b)`), so a name next to a non-ASCII letter is still replaced
/// (hiding too much is safe), and an end that is a non-ASCII letter gets no
/// boundary at all.
fn whole_word(name: &str) -> String {
    word_pattern(name, r"(?-u:\b)")
}

/// `name` escaped, with `word_start` before it when it starts with a word
/// character and an ASCII word boundary after it when it ends with one.
fn word_pattern(name: &str, word_start: &str) -> String {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let start = if word(name.chars().next()) {
        word_start
    } else {
        ""
    };
    let end = if word(name.chars().last()) {
        r"(?-u:\b)"
    } else {
        ""
    };
    format!("{start}{}{end}", regex::escape(name))
}

/// A player name as `whole_word` matches it, or right after MTGO's `@P`
/// player tag (`@PQname wins`), where no word boundary falls before the name.
/// The tag is replaced along with the name.
fn player_word(name: &str) -> String {
    word_pattern(name, r"(?:@P|(?-u:\b))")
}

/// The last part of the home folder, when long enough to replace safely.
fn user_name_of(home: &str) -> Option<String> {
    let last = home
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    if last.len() >= MIN_NAME_LEN {
        Some(last.to_string())
    } else {
        None
    }
}

/// Every string value inside `value`, as real text (not escaped JSON).
fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_strings(item, out);
            }
        }
        Value::Object(fields) => {
            for item in fields.values() {
                collect_strings(item, out);
            }
        }
        _ => {}
    }
}

fn scrub_strings(value: &mut Value, names: &NamePatterns, home: &HomePatterns) {
    match value {
        Value::String(text) => *text = replace_names(&home.hide(&hide_secrets(text)), names),
        Value::Array(items) => {
            for item in items {
                scrub_strings(item, names, home);
            }
        }
        Value::Object(fields) => {
            for (_, item) in fields.iter_mut() {
                scrub_strings(item, names, home);
            }
        }
        _ => {}
    }
}

/// One pattern matching every name to hide, or `None` when there are none.
type NamePattern = Option<Arc<Regex>>;

/// The patterns one report is scrubbed with, applied in this order.
struct NamePatterns {
    /// Names only this report reveals, built for this report. Also holds any
    /// known name that contains one of them, so that longer name is replaced
    /// whole rather than leaving its end behind.
    extra: NamePattern,
    /// The known list's pattern, shared through the cache.
    known: NamePattern,
}

/// The compiled pattern for the last `known` list a report was scrubbed with.
/// Compiling thousands of names is the slow part, so it is done once and
/// reused until the list changes.
struct PatternCache {
    slot: Mutex<Option<(Vec<String>, NamePattern)>>,
}

impl PatternCache {
    const fn new() -> Self {
        PatternCache {
            slot: Mutex::new(None),
        }
    }

    /// The pattern for `known`, compiled only when the list has changed.
    fn known_pattern(&self, known: &[String]) -> NamePattern {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached_for, pattern)) = slot.as_ref()
            && cached_for.as_slice() == known
        {
            return pattern.clone();
        }
        let pattern = build_pattern(known);
        *slot = Some((known.to_vec(), pattern.clone()));
        pattern
    }
}

/// The cache every report shares.
static PATTERN_CACHE: PatternCache = PatternCache::new();

/// The patterns for `known` plus the names found in `text`. The known list's
/// pattern always comes from the cache; names only `text` reveals get a small
/// pattern of their own, so thousands of known names are never compiled again
/// for one report.
fn name_patterns(cache: &PatternCache, known: &[String], text: &str) -> NamePatterns {
    let known_set: BTreeSet<&String> = known.iter().collect();
    let found: Vec<String> = detect_names(text.as_bytes())
        .into_iter()
        .filter(|name| name.len() >= MIN_NAME_LEN && !known_set.contains(name))
        .collect();
    let mut extra = found.clone();
    if !found.is_empty() {
        for name in known {
            if found.iter().any(|short| name.contains(short.as_str())) {
                extra.push(name.clone());
            }
        }
    }
    NamePatterns {
        extra: build_pattern(&extra),
        known: cache.known_pattern(known),
    }
}

/// Names long enough to replace, each as a word of its own (so a name that
/// is also part of a longer word leaves that word alone), longest first so a
/// short name never clobbers part of a longer one (the first alternative that
/// matches wins).
fn build_pattern(names: &[String]) -> NamePattern {
    let mut names: Vec<String> = names
        .iter()
        .filter(|name| name.len() >= MIN_NAME_LEN)
        .cloned()
        .collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    names.dedup();
    if names.is_empty() {
        return None;
    }
    let escaped: Vec<String> = names.iter().map(|name| player_word(name)).collect();
    let regex = RegexBuilder::new(&escaped.join("|"))
        .size_limit(256 * 1024 * 1024)
        .build()
        .expect("escaped names make a valid pattern");
    Some(Arc::new(regex))
}

/// Replace the extra names, then the known names. The known pass works only
/// on the text between placeholders, so a known name never matches inside a
/// `<player>` the first pass wrote (as with one combined pattern).
fn replace_names(text: &str, names: &NamePatterns) -> String {
    let first = replace_with(text, &names.extra);
    let mut parts: Vec<String> = Vec::new();
    for part in first.split(PLAYER) {
        parts.push(replace_with(part, &names.known));
    }
    parts.join(PLAYER)
}

fn replace_with(text: &str, pattern: &NamePattern) -> String {
    match pattern {
        Some(pattern) => pattern
            .replace_all(text, regex::NoExpand(PLAYER))
            .into_owned(),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bearer_tokens_and_api_keys_are_stripped() {
        let text = "Authorization: Bearer qaccess.tok-en_1/+= sent; \
            X-Api-Key: qkey-123 and x-api-key=qkey-456, \
            {\"access_token\":\"qjson-access\",\"refresh_token\": \"qjson-refresh\",\"api_key\":\"qjson-key\"} \
            grant_type=refresh_token&refresh_token=qform-refresh&client_id=qclient \
            [(\"X-Device-Id\", \"qdevice\"), (\"X-Api-Key\", \"qkey-tuple\")]";
        let out = hide_secrets(text);
        for secret in [
            "qaccess",
            "qkey-123",
            "qkey-456",
            "qjson-access",
            "qjson-refresh",
            "qjson-key",
            "qform-refresh",
            "qkey-tuple",
        ] {
            assert!(!out.contains(secret), "{secret} in {out}");
        }
        assert!(out.contains("Bearer <secret>"), "{out}");
        assert!(out.contains("grant_type=refresh_token"), "{out}");
        assert!(out.contains("client_id=qclient"), "{out}");
        assert_eq!(hide_secrets("no secrets here"), "no secrets here");
    }

    #[test]
    fn reports_are_stripped_of_secrets() {
        let mut report = json!({
            "message": "request failed: Authorization: Bearer qreport-token",
            "extra": { "headers": "X-Api-Key: qreport-key" }
        });
        scrub_json(&mut report, &[], None);
        let text = report.to_string();
        assert!(!text.contains("qreport-token"), "{text}");
        assert!(!text.contains("qreport-key"), "{text}");
        assert_eq!(
            scrub_text("Bearer qtext-token", &[], None),
            "Bearer <secret>"
        );
    }

    #[test]
    fn known_names_are_replaced_longest_first() {
        let known = vec!["Qab".to_string(), "Qabcd".to_string()];
        assert_eq!(
            scrub_text("Qabcd met Qab", &known, None),
            "<player> met <player>"
        );
    }

    #[test]
    fn known_names_are_replaced_only_as_words_of_their_own() {
        let known = vec!["Tron".to_string(), "Q.dot.".to_string()];
        assert_eq!(
            scrub_text("MegaTron failed; Tron's deck, (Tron) Q.dot.x", &known, None),
            "MegaTron failed; <player>'s deck, (<player>) <player>x",
            "a name inside a word stays; an edge that is not a word character needs no boundary"
        );
        assert_eq!(
            scrub_text("Username: Qfound (1) Qfounder", &[], None),
            "Username: <player> (1) Qfounder",
            "names found in the text too"
        );
    }

    #[test]
    fn names_in_mtgo_player_tags_are_replaced() {
        let known = vec!["Qtagged".to_string()];
        let scrubbed = scrub_text("@PQtagged wins the game. @PQtagged@P", &known, None);
        assert!(!scrubbed.contains("Qtagged"), "{scrubbed}");
    }

    #[test]
    fn names_with_symbols_or_non_ascii_letters_are_replaced() {
        let known = vec![
            "Q.dots+(x)".to_string(),
            "Qñandú".to_string(),
            "ÉQaccent".to_string(),
        ];
        let scrubbed = scrub_text(
            "Q.dots+(x) met Qñandú and ÉQaccent, then xÉQaccent",
            &known,
            None,
        );
        for name in &known {
            assert!(!scrubbed.contains(name.as_str()), "{name}: {scrubbed}");
        }
    }

    #[test]
    fn names_in_mtgo_text_are_found_even_when_unknown() {
        let text = "07:00:00 [INF] (Login|MtGO Login Success) Username: Qhidden (123)\n{\"UserName\":\"Qjsonname\"}";
        let scrubbed = scrub_text(text, &[], None);
        assert!(!scrubbed.contains("Qhidden"), "{scrubbed}");
        assert!(!scrubbed.contains("Qjsonname"), "{scrubbed}");
    }

    #[test]
    fn the_home_folder_becomes_a_tilde_with_either_slash() {
        let home = Some(r"C:\Users\qhome");
        assert_eq!(
            hide_home(r"C:\Users\qhome\AppData and C:/Users/qhome/AppData", home),
            r"~\AppData and ~/AppData"
        );
        assert_eq!(hide_home("/home/qhome/x", Some("")), "/home/qhome/x");
    }

    #[test]
    fn every_string_in_a_report_is_scrubbed() {
        let mut report = json!({
            "message": "Qnested failed",
            "frames": [{ "vars": { "name": "Qnested", "path": "/home/qhome/a" } }],
            "count": 3
        });
        scrub_json(&mut report, &["Qnested".to_string()], Some("/home/qhome"));
        assert_eq!(
            report,
            json!({
                "message": "<player> failed",
                "frames": [{ "vars": { "name": "<player>", "path": "~/a" } }],
                "count": 3
            })
        );
    }

    #[test]
    fn remembered_names_are_known_and_short_ones_are_ignored() {
        remember(["Qremembered".to_string(), "Qa".to_string()]);
        let known = known().expect("not busy");
        assert!(known.contains(&"Qremembered".to_string()));
        assert!(
            !known.contains(&"Qa".to_string()),
            "too short to replace safely"
        );
    }

    #[test]
    fn known_answers_while_another_thread_remembers() {
        let writer = std::thread::spawn(|| {
            for index in 0..200 {
                remember([format!("Qbusy{index:03}")]);
            }
        });
        for _ in 0..200 {
            assert!(known().is_some(), "a report was dropped");
        }
        writer.join().expect("no panic");
    }

    #[test]
    fn known_is_none_inside_remember_on_the_same_thread() {
        REMEMBERING.with(|flag| flag.set(true));
        let inside = known();
        REMEMBERING.with(|flag| flag.set(false));
        assert!(inside.is_none());
        assert!(known().is_some());
    }

    #[test]
    fn scrub_json_finds_unknown_names_inside_multiline_text() {
        let mut report = json!({
            "log": "12:00:00 [INF] (Login|MtGO Login Success) Username: Qleaker\nnext line",
            "fragment": "{\"UserName\":\"Qquoted\"}"
        });
        scrub_json(&mut report, &[], None);
        let text = report.to_string();
        assert!(!text.contains("Qleaker"), "{text}");
        assert!(!text.contains("Qquoted"), "{text}");
        assert!(text.contains("<player>"), "{text}");
    }

    fn has_ci(text: &str, needle: &str) -> bool {
        text.to_lowercase().contains(&needle.to_lowercase())
    }

    #[test]
    fn the_wine_user_folder_is_hidden() {
        let out = hide_home(
            "/home/qhome/.wine/drive_c/users/qhome/AppData/x",
            Some("/home/qhome"),
        );
        assert!(!has_ci(&out, "qhome"), "{out}");
    }

    #[test]
    fn a_short_path_windows_user_folder_is_hidden() {
        let out = scrub_text(
            r"C:\Users\QHOMEU~1\AppData\Local\Temp\x",
            &[],
            Some(r"C:\Users\qhomeuser"),
        );
        assert!(!has_ci(&out, "QHOMEU"), "{out}");
        assert!(!has_ci(&out, "qhomeuser"), "{out}");
    }

    #[test]
    fn a_differently_cased_home_is_hidden() {
        let out = hide_home(r"c:\users\QHomeUser\AppData", Some(r"C:\Users\qhomeuser"));
        assert!(!has_ci(&out, "qhomeuser"), "{out}");
    }

    #[test]
    fn the_steamuser_wine_folder_is_kept() {
        let out = hide_home(
            "/home/qhome/.wine/drive_c/users/steamuser/AppData/x",
            Some("/home/qhome"),
        );
        assert!(out.contains("steamuser"), "{out}");
        assert!(!has_ci(&out, "qhome"), "{out}");
    }

    #[test]
    fn the_os_user_name_is_hidden_anywhere_in_the_text() {
        let out = hide_home("owner is QHome here", Some("/home/qhome"));
        assert_eq!(out, "owner is <user> here");
    }

    #[test]
    fn a_breadcrumb_with_a_wine_path_is_scrubbed() {
        let mut report = json!({
            "breadcrumbs": { "values": [{
                "message": "log no longer detected",
                "data": { "path": "/home/qhome/.wine/drive_c/users/qhome/AppData/Roaming/x" }
            }] }
        });
        scrub_json(&mut report, &[], Some("/home/qhome"));
        let text = report.to_string();
        assert!(!has_ci(&text, "qhome"), "{text}");
    }

    #[test]
    fn a_common_user_name_is_not_hidden_inside_words() {
        // The Steam Deck user is `deck`: reports full of deck_used and
        // DeckUsed must stay readable, while the name on its own is hidden.
        let out = hide_home("deck_used DeckUsed by deck.", Some("/home/deck"));
        assert_eq!(out, "deck_used DeckUsed by <user>.");
    }

    #[test]
    fn another_windows_user_folder_with_spaces_is_hidden_whole() {
        let out = hide_home(
            r"read C:\Users\Q Other Q Name\AppData\x failed",
            Some(r"C:\Users\qhomeuser"),
        );
        assert_eq!(out, r"read ~\AppData\x failed");
    }

    #[test]
    fn a_root_home_folder_hides_nothing() {
        assert_eq!(hide_home("/var/log/x", Some("/")), "/var/log/x");
    }

    #[test]
    fn scrubbing_with_five_thousand_known_names_takes_a_few_milliseconds() {
        let known: Vec<String> = (0..5000).map(|i| format!("Qplayer{i:04}")).collect();
        let mut text = String::new();
        for index in 0..50 {
            text.push_str(&format!("{} joined the match. ", known[index * 97]));
            while text.len() < (index + 1) * 400 {
                text.push_str("ordinary log filler text. ");
            }
        }
        // A cache of its own: other tests running at the same time replace
        // the shared one, which would make this call compile the pattern.
        // Warm up: the first call compiles the pattern, which is allowed to
        // take longer than a scrub that reuses it.
        let cache = PatternCache::new();
        let _ = scrub_text_with(&cache, "warm up", &known, None);
        let start = std::time::Instant::now();
        let out = scrub_text_with(&cache, &text, &known, None);
        let elapsed = start.elapsed();
        println!("scrub took {elapsed:?}");
        assert_eq!(out.matches(PLAYER).count(), 50);
        assert!(!out.contains("Qplayer"), "a name survived");
        let limit = if cfg!(debug_assertions) { 200 } else { 20 };
        assert!(
            elapsed < std::time::Duration::from_millis(limit),
            "took {elapsed:?}, limit {limit} ms"
        );
    }

    #[test]
    fn an_extra_name_is_hidden_without_rebuilding_the_known_pattern() {
        let cache = PatternCache::new();
        let known = vec!["Qknownone".to_string(), "Qknowntwo".to_string()];
        let cached = cache.known_pattern(&known).expect("a pattern");
        let text = "Username: Qextraname (1)\nQknownone met Qextraname";
        let names = name_patterns(&cache, &known, text);
        let used = names.known.as_ref().expect("the known pattern");
        assert!(
            Arc::ptr_eq(used, &cached),
            "the cached known pattern is used as it is"
        );
        let extra = names.extra.as_ref().expect("an extra pattern");
        assert!(
            extra.as_str().contains("Qextraname") && !extra.as_str().contains("Qknown"),
            "only the extra name: {}",
            extra.as_str()
        );
        assert_eq!(
            scrub_text_with(&cache, text, &known, None),
            "Username: <player> (1)\n<player> met <player>"
        );
    }

    #[test]
    fn known_and_extra_names_inside_each_other_are_replaced_whole() {
        // An extra name that starts with a known name.
        let known = vec!["Qabc".to_string()];
        assert_eq!(
            scrub_text("Username: Qabcdef (1) Qabc", &known, None),
            "Username: <player> (1) <player>"
        );
        // A known name that starts with an extra name.
        let known = vec!["Qxyzlong".to_string()];
        assert_eq!(
            scrub_text("Username: Qxyz (1) Qxyzlong", &known, None),
            "Username: <player> (1) <player>"
        );
    }

    #[test]
    fn a_known_name_inside_the_placeholder_is_not_replaced_again() {
        let known = vec!["layer".to_string()];
        assert_eq!(
            scrub_text("Username: Qfound (1) layer", &known, None),
            "Username: <player> (1) <player>"
        );
    }

    #[test]
    fn a_name_found_in_the_text_is_hidden_with_a_cached_known_list() {
        let known = vec!["Qabc".to_string()];
        assert_eq!(scrub_text("Qabc x", &known, None), "<player> x");
        let text = "Username: Qabcdef (1)\nQabc";
        let out = scrub_text(text, &known, None);
        assert!(!out.contains("Qabc"), "{out}");
    }
}
