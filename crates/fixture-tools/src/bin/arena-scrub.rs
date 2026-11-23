//! Replace names and ids in an Arena `Player.log` with fakes.
//!
//! arena-scrub <in> <out> [--allow-existing=<to>]... <from=to>...
//!
//! Replaces every occurrence of each `from` with its `to` in the bytes
//! (longest `from` first, so a tag-bearing name goes before its bare form).
//! Fails, naming the string, if a `to` was already present before replacing
//! (unless that `to` is named by `--allow-existing=<to>`, for a short fake such
//! as `Player` that is also an ordinary word in the log) or if a `from` is
//! still present afterwards. There is no length rule.
//! Error messages name the pair's position, never the real string.

use std::process::ExitCode;

/// Count non-overlapping occurrences of `needle` in `haystack`.
fn count(haystack: &[u8], needle: &[u8]) -> usize {
    find_all(haystack, needle).len()
}

fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut found = Vec::new();
    if needle.is_empty() || haystack.len() < needle.len() {
        return found;
    }
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            found.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

fn replace_all(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut last = 0;
    for start in find_all(bytes, from) {
        out.extend_from_slice(&bytes[last..start]);
        out.extend_from_slice(to);
        last = start + from.len();
    }
    out.extend_from_slice(&bytes[last..]);
    out
}

/// Apply every `(from, to)` pair. Errors name the pair by number.
fn scrub(
    bytes: &[u8],
    pairs: &[(String, String)],
    allow_existing: &[String],
) -> Result<Vec<u8>, String> {
    for (n, (_, to)) in pairs.iter().enumerate() {
        if allow_existing.contains(to) {
            continue;
        }
        if count(bytes, to.as_bytes()) > 0 {
            return Err(format!(
                "pair {}: replacement already present in input",
                n + 1
            ));
        }
    }
    let mut order: Vec<usize> = (0..pairs.len()).collect();
    order.sort_by_key(|n| std::cmp::Reverse(pairs[*n].0.len()));
    let mut current = bytes.to_vec();
    for n in order {
        let (from, to) = &pairs[n];
        if from.is_empty() {
            return Err(format!("pair {}: empty from", n + 1));
        }
        current = replace_all(&current, from.as_bytes(), to.as_bytes());
    }
    for (n, (from, _)) in pairs.iter().enumerate() {
        if count(&current, from.as_bytes()) > 0 {
            return Err(format!(
                "pair {}: original still present after replacing",
                n + 1
            ));
        }
    }
    Ok(current)
}

fn parse_pair(arg: &str) -> Result<(String, String), String> {
    match arg.split_once('=') {
        Some((from, to)) => Ok((from.to_string(), to.to_string())),
        None => Err("pair without '='".to_string()),
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("usage: arena-scrub <in> <out> <from=to>...".to_string());
    }
    let mut pairs = Vec::new();
    let mut allow_existing = Vec::new();
    for arg in &args[2..] {
        match arg.strip_prefix("--allow-existing=") {
            Some(to) => allow_existing.push(to.to_string()),
            None => pairs.push(parse_pair(arg)?),
        }
    }
    let bytes = std::fs::read(&args[0]).map_err(|e| format!("reading input: {e}"))?;
    let out = scrub(&bytes, &pairs, &allow_existing)?;
    std::fs::write(&args[1], out).map_err(|e| format!("writing output: {e}"))?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("arena-scrub: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(from: &str, to: &str) -> (String, String) {
        (from.to_string(), to.to_string())
    }

    #[test]
    fn replaces_longest_first() {
        let pairs = vec![pair("Bob", "Player"), pair("Bob#1", "Player#00001")];
        let out = scrub(b"Bob#1 and Bob", &pairs, &[]).unwrap();
        assert_eq!(out, b"Player#00001 and Player");
    }

    #[test]
    fn fails_when_replacement_already_present() {
        let pairs = vec![pair("Zed", "Player")];
        assert!(scrub(b"Zed Player", &pairs, &[]).is_err());
        let allowed = vec!["Player".to_string()];
        assert!(scrub(b"Zed Player", &pairs, &allowed).is_ok());
    }

    #[test]
    fn fails_when_original_survives() {
        let pairs = vec![pair("ab", "a"), pair("a", "ab")];
        assert!(scrub(b"zz", &pairs, &[]).is_ok());
        let pairs = vec![pair("x", "xy")];
        assert!(scrub(b"x", &pairs, &[]).is_err());
    }
}
