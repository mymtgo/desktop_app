//! The code behind `mymtgo-endstep-host`: answers native messaging requests
//! and writes delivered captures into the inbox folder. A capture too big for
//! one message arrives as numbered parts on one port and is assembled in a
//! partial file before it is checked and renamed into place.

use endstep_capture::{
    CaptureHeader, FramingError, HostReply, HostRequest, LastSeen, MAX_CAPTURE_BYTES,
    last_seen_json, parse_capture, read_message, valid_unit_id, write_message,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The inbox folder sits next to the helper executable.
pub fn inbox_beside(exe: &Path) -> PathBuf {
    match exe.parent() {
        Some(folder) => folder.join("inbox"),
        None => PathBuf::from("inbox"),
    }
}

/// Answers every message until the input ends. A refused request gets a
/// `failed` reply; only a framing or output error stops the loop.
pub fn serve(
    input: &mut impl Read,
    output: &mut impl Write,
    inbox: &Path,
    now: &dyn Fn() -> i64,
) -> Result<(), FramingError> {
    serve_with_limit(input, output, inbox, now, MAX_CAPTURE_BYTES)
}

/// `serve` with the largest capture size as a parameter, so a test does not
/// have to write 512 MiB.
fn serve_with_limit(
    input: &mut impl Read,
    output: &mut impl Write,
    inbox: &Path,
    now: &dyn Fn() -> i64,
    max_capture_bytes: u64,
) -> Result<(), FramingError> {
    // A unit sent in parts lives only as long as this port: when the input
    // ends mid-unit its partial file goes with it.
    let mut pending: Option<PendingUnit> = None;
    let result = serve_loop(input, output, inbox, now, max_capture_bytes, &mut pending);
    drop_pending(&mut pending);
    result
}

fn serve_loop(
    input: &mut impl Read,
    output: &mut impl Write,
    inbox: &Path,
    now: &dyn Fn() -> i64,
    max_capture_bytes: u64,
    pending: &mut Option<PendingUnit>,
) -> Result<(), FramingError> {
    while let Some(body) = read_message(input)? {
        let reply = answer(&body, inbox, now, max_capture_bytes, pending);
        let bytes = match serde_json::to_vec(&reply) {
            Ok(bytes) => bytes,
            Err(error) => return Err(FramingError::Io(error.to_string())),
        };
        write_message(output, &bytes)?;
    }
    Ok(())
}

/// A capture being assembled from parts: its partial file and how far it is.
struct PendingUnit {
    unit_id: String,
    next_index: u32,
    partial: PathBuf,
    bytes: u64,
}

/// Forgets the pending unit and deletes its partial file.
fn drop_pending(pending: &mut Option<PendingUnit>) {
    if let Some(unit) = pending.take() {
        let _ = std::fs::remove_file(&unit.partial);
    }
}

fn answer(
    body: &[u8],
    inbox: &Path,
    now: &dyn Fn() -> i64,
    max_capture_bytes: u64,
    pending: &mut Option<PendingUnit>,
) -> HostReply {
    let request: HostRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(_) => {
            // The extension is alive even when its request is not readable.
            record_alive(inbox, now());
            return HostReply::failed("bad request");
        }
    };
    match request {
        HostRequest::Ping { browser, extension } => {
            record_seen(
                inbox,
                now(),
                browser.unwrap_or_else(|| UNKNOWN.to_string()),
                extension.unwrap_or_else(|| UNKNOWN.to_string()),
            );
            HostReply::pong(env!("CARGO_PKG_VERSION"))
        }
        HostRequest::Deliver { unit_id, content } => {
            finish_reply(inbox, now, store(inbox, &unit_id, &content))
        }
        HostRequest::DeliverPart {
            unit_id,
            index,
            content,
        } => {
            let outcome = store_part(inbox, pending, max_capture_bytes, &unit_id, index, &content);
            match outcome {
                Ok(()) => HostReply::stored(),
                Err(failure) => {
                    drop_pending(pending);
                    failure_reply(inbox, now, failure)
                }
            }
        }
        HostRequest::DeliverEnd { unit_id, parts } => {
            let outcome = end_parts(inbox, pending, &unit_id, parts);
            // Whatever the outcome, the unit is no longer pending: on success
            // the partial was renamed, on failure it is deleted.
            drop_pending(pending);
            finish_reply(inbox, now, outcome)
        }
    }
}

/// The reply for a finished delivery: stored (and the browser recorded), or
/// failed.
fn finish_reply(
    inbox: &Path,
    now: &dyn Fn() -> i64,
    outcome: Result<CaptureHeader, Failure>,
) -> HostReply {
    match outcome {
        Ok(header) => {
            record_seen(inbox, now(), header.browser, header.extension);
            HostReply::stored()
        }
        Err(failure) => failure_reply(inbox, now, failure),
    }
}

fn failure_reply(inbox: &Path, now: &dyn Fn() -> i64, failure: Failure) -> HostReply {
    record_alive(inbox, now());
    // The browser keeps helper stderr in its logs; the reply goes to the
    // extension and must not carry a path.
    eprintln!("mymtgo-endstep-host: {}", failure.detail);
    HostReply::failed(&failure.reply)
}

const UNKNOWN: &str = "unknown";

/// The folder `last_seen.json` sits in: beside the inbox.
fn seen_folder(inbox: &Path) -> &Path {
    match inbox.parent() {
        Some(folder) => folder,
        None => Path::new("."),
    }
}

/// A message that names no browser (an unreadable request, a refused
/// delivery): only the time moves, the browser and extension already
/// recorded stay (`unknown` when there are none).
fn record_alive(inbox: &Path, at: i64) {
    let previous = std::fs::read_to_string(seen_folder(inbox).join("last_seen.json"))
        .ok()
        .and_then(|text| endstep_capture::parse_last_seen(&text));
    let (browser, extension) = match previous {
        Some(seen) => (seen.browser, seen.extension),
        None => (UNKNOWN.to_string(), UNKNOWN.to_string()),
    };
    record_seen(inbox, at, browser, extension);
}

/// Writes `last_seen.json` beside the inbox. A failure is only logged: it
/// must never change the reply the extension gets.
fn record_seen(inbox: &Path, at: i64, browser: String, extension: String) {
    let folder = seen_folder(inbox);
    let text = last_seen_json(&LastSeen {
        at,
        browser,
        extension,
    });
    let target = folder.join("last_seen.json");
    let partial = folder.join(format!("last_seen.{}.partial", std::process::id()));
    if let Err(error) = std::fs::create_dir_all(folder) {
        eprintln!("mymtgo-endstep-host: could not write last seen: {error}");
        return;
    }
    if let Err(error) = write_whole(&partial, &target, &text) {
        eprintln!("mymtgo-endstep-host: could not write last seen: {error}");
    }
}

/// Why a delivery failed: `reply` is the short fixed text sent to the
/// extension, `detail` goes to stderr only (OS errors can name the inbox
/// path, which holds the user's name).
struct Failure {
    reply: String,
    detail: String,
}

impl Failure {
    /// A refusal whose text is safe to send as it is.
    fn refused(message: &str) -> Failure {
        Failure {
            reply: message.to_string(),
            detail: message.to_string(),
        }
    }

    /// A failed file operation: a fixed reply, the OS error as detail.
    fn io(reply: &str, error: &std::io::Error) -> Failure {
        Failure {
            reply: reply.to_string(),
            detail: format!("{reply}: {error}"),
        }
    }
}

/// The partial file of a unit in `inbox`. One helper process runs per port,
/// so the name carries the process id: two deliveries of one unit never
/// share a partial file.
fn partial_path(inbox: &Path, unit_id: &str) -> PathBuf {
    inbox.join(format!("{unit_id}.{}.partial", std::process::id()))
}

/// Checks the content is a capture of `unit_id`. Parse errors name a line of
/// the content, never a path.
fn validate(unit_id: &str, content: &str) -> Result<CaptureHeader, Failure> {
    let parsed = parse_capture(content).map_err(|error| Failure::refused(&error.to_string()))?;
    if parsed.header.unit_id != unit_id {
        return Err(Failure::refused("capture is for a different unit"));
    }
    Ok(parsed.header)
}

/// Moves a whole, checked partial file onto `<unit_id>.jsonl`. The partial
/// file is removed on failure.
fn publish(inbox: &Path, partial: &Path, unit_id: &str) -> Result<(), Failure> {
    let final_path = inbox.join(format!("{unit_id}.jsonl"));
    if let Err(error) = std::fs::rename(partial, &final_path) {
        let _ = std::fs::remove_file(partial);
        return Err(Failure::io("could not finish capture", &error));
    }
    // The file is whole and in place, so a failed folder sync is only logged.
    if let Err(error) = sync_folder(inbox) {
        eprintln!("mymtgo-endstep-host: could not sync inbox folder: {error}");
    }
    Ok(())
}

/// Checks the unit id before any path is built from it, then checks the
/// content, then writes it whole (partial file, then rename). Gives back the
/// parsed header so the caller does not parse the content again.
fn store(inbox: &Path, unit_id: &str, content: &str) -> Result<CaptureHeader, Failure> {
    if !valid_unit_id(unit_id) {
        return Err(Failure::refused("invalid unit id"));
    }
    let header = validate(unit_id, content)?;
    std::fs::create_dir_all(inbox)
        .map_err(|error| Failure::io("could not create inbox", &error))?;
    let partial = partial_path(inbox, unit_id);
    if let Err(error) = write_synced(&partial, content) {
        let _ = std::fs::remove_file(&partial);
        return Err(Failure::io("could not write capture", &error));
    }
    publish(inbox, &partial, unit_id)?;
    Ok(header)
}

/// Writes one part of a big capture. Part 0 starts the unit (and drops any
/// unit left pending); a later part must be the next one of the pending unit.
/// The caller drops the pending unit when this fails.
fn store_part(
    inbox: &Path,
    pending: &mut Option<PendingUnit>,
    max_capture_bytes: u64,
    unit_id: &str,
    index: u32,
    content: &str,
) -> Result<(), Failure> {
    if index == 0 {
        if !valid_unit_id(unit_id) {
            return Err(Failure::refused("invalid unit id"));
        }
        drop_pending(pending);
        std::fs::create_dir_all(inbox)
            .map_err(|error| Failure::io("could not create inbox", &error))?;
        let partial = partial_path(inbox, unit_id);
        let mut unit = PendingUnit {
            unit_id: unit_id.to_string(),
            next_index: 0,
            partial,
            bytes: 0,
        };
        // Created empty first, so the file is the pending unit's to delete.
        if let Err(error) = std::fs::File::create(&unit.partial) {
            return Err(Failure::io("could not write capture", &error));
        }
        if let Err(failure) = append_part(&mut unit, max_capture_bytes, content) {
            let _ = std::fs::remove_file(&unit.partial);
            return Err(failure);
        }
        *pending = Some(unit);
        return Ok(());
    }
    let Some(unit) = pending.as_mut() else {
        return Err(Failure::refused("part out of order"));
    };
    if unit.unit_id != unit_id || unit.next_index != index {
        return Err(Failure::refused("part out of order"));
    }
    append_part(unit, max_capture_bytes, content)
}

fn append_part(
    unit: &mut PendingUnit,
    max_capture_bytes: u64,
    content: &str,
) -> Result<(), Failure> {
    let total = unit.bytes + content.len() as u64;
    if total > max_capture_bytes {
        return Err(Failure::refused("capture too large"));
    }
    let written = std::fs::OpenOptions::new()
        .append(true)
        .open(&unit.partial)
        .and_then(|mut file| file.write_all(content.as_bytes()));
    if let Err(error) = written {
        return Err(Failure::io("could not write capture", &error));
    }
    unit.bytes = total;
    unit.next_index += 1;
    Ok(())
}

/// Finishes a unit sent in parts: the pending file is read back, checked the
/// same way a single delivery is, and renamed into place. The caller drops
/// the pending unit afterwards (a no-op once the rename moved the file).
fn end_parts(
    inbox: &Path,
    pending: &mut Option<PendingUnit>,
    unit_id: &str,
    parts: u32,
) -> Result<CaptureHeader, Failure> {
    let Some(unit) = pending.as_ref() else {
        return Err(Failure::refused("part out of order"));
    };
    if unit.unit_id != unit_id || unit.next_index != parts {
        return Err(Failure::refused("part count does not match"));
    }
    // Windows flushes a file only through a handle with full write access, not an append-only one.
    let synced = std::fs::OpenOptions::new()
        .write(true)
        .open(&unit.partial)
        .and_then(|file| file.sync_all());
    if let Err(error) = synced {
        return Err(Failure::io("could not write capture", &error));
    }
    let content = match std::fs::read_to_string(&unit.partial) {
        Ok(content) => content,
        Err(error) => return Err(Failure::io("could not read capture", &error)),
    };
    let header = validate(unit_id, &content)?;
    publish(inbox, &unit.partial, unit_id)?;
    Ok(header)
}

/// Makes the rename durable on Unix. Windows cannot sync a folder this way.
#[cfg(unix)]
fn sync_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::File::open(folder)?.sync_all()
}

#[cfg(not(unix))]
fn sync_folder(_folder: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Writes `content` to `partial`, then renames it onto `target`; the partial
/// file is removed on failure.
fn write_whole(partial: &Path, target: &Path, content: &str) -> std::io::Result<()> {
    let result = write_synced(partial, content).and_then(|()| std::fs::rename(partial, target));
    if result.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    result
}

/// Writes `content` and forces it to disk before returning, so a power loss
/// after the rename cannot leave an empty or short completed file.
fn write_synced(path: &Path, content: &str) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use endstep_capture::{HostReply, write_message};
    use fixture_tools::TestDir;
    use std::io::Cursor;

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const OTHER_ID: &str = "1f8fad5b-d9cb-469f-a165-70867728950e";

    fn capture(unit_id: &str, extra_frames: usize) -> String {
        let mut text = format!(
            r#"{{"mymtgo_endstep_capture":1,"unit_id":"{unit_id}","kind":"match","started_at":1700000000000,"counter":1,"extension":"0.1.0","browser":"firefox","developer":false,"classification":"human"}}"#
        );
        text.push('\n');
        for n in 0..extra_frames {
            text.push_str(&format!(
                r#"{{"at":{n},"src":"rest","dir":"in","url":"https://x"}}"#
            ));
            text.push('\n');
        }
        text
    }

    fn deliver(unit_id: &str, content: &str) -> Vec<u8> {
        serde_json::json!({"type":"deliver","unit_id":unit_id,"content":content})
            .to_string()
            .into_bytes()
    }

    fn run(messages: &[Vec<u8>], inbox: &std::path::Path) -> Vec<HostReply> {
        run_at(messages, inbox, 1_000)
    }

    fn run_at(messages: &[Vec<u8>], inbox: &std::path::Path, clock: i64) -> Vec<HostReply> {
        let mut input = Vec::new();
        for body in messages {
            write_message(&mut input, body).unwrap();
        }
        let mut output = Vec::new();
        serve(&mut Cursor::new(input), &mut output, inbox, &|| clock).unwrap();
        let mut replies = Vec::new();
        let mut reader = Cursor::new(output);
        while let Some(body) = endstep_capture::read_message(&mut reader).unwrap() {
            replies.push(serde_json::from_slice(&body).unwrap());
        }
        replies
    }

    fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(current) = pending.pop() {
            if let Ok(entries) = std::fs::read_dir(&current) {
                for entry in entries {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        pending.push(path);
                    } else {
                        found.push(path);
                    }
                }
            }
        }
        found
    }

    /// Files under `dir` other than `last_seen.json`, which every message
    /// now leaves behind.
    fn capture_files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = files_under(dir);
        files.retain(|path| path.file_name().is_none_or(|name| name != "last_seen.json"));
        files
    }

    #[test]
    fn ping_answers_with_the_version() {
        let dir = TestDir::new("host-ping");
        let replies = run(&[br#"{"type":"ping"}"#.to_vec()], &dir.join("inbox"));
        assert_eq!(replies.len(), 1);
        assert!(replies[0].ok);
        assert_eq!(
            replies[0].version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn a_delivered_unit_lands_whole_in_the_inbox() {
        let dir = TestDir::new("host-deliver");
        let inbox = dir.join("inbox");
        let content = capture(ID, 2);
        let replies = run(&[deliver(ID, &content)], &inbox);
        assert_eq!(replies, vec![HostReply::stored()]);
        assert_eq!(
            std::fs::read_to_string(inbox.join(format!("{ID}.jsonl"))).unwrap(),
            content
        );
        assert!(
            !inbox
                .join(format!("{ID}.{}.partial", std::process::id()))
                .exists()
        );
        assert_eq!(files_under(&inbox).len(), 1);
        assert_eq!(
            serde_json::to_string(&replies[0]).unwrap(),
            r#"{"ok":true}"#
        );
    }

    #[test]
    fn a_unit_id_with_a_path_is_refused() {
        let dir = TestDir::new("host-path");
        // Two levels down, so "../../x.jsonl" would land inside the test dir.
        let inbox = dir.join("a/b/inbox");
        let replies = run(&[deliver("../../x", &capture(ID, 0))], &inbox);
        assert!(!replies[0].ok);
        assert_eq!(replies[0].error.as_deref(), Some("invalid unit id"));
        assert!(capture_files_under(dir.path()).is_empty());
        assert!(!inbox.exists());
    }

    #[test]
    fn content_for_another_unit_is_refused() {
        let dir = TestDir::new("host-other");
        let inbox = dir.join("inbox");
        let replies = run(&[deliver(ID, &capture(OTHER_ID, 0))], &inbox);
        assert!(!replies[0].ok);
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn bad_content_is_refused() {
        let dir = TestDir::new("host-bad");
        let inbox = dir.join("inbox");
        let replies = run(&[deliver(ID, "not a capture\n")], &inbox);
        assert!(!replies[0].ok);
        assert!(replies[0].error.as_deref().unwrap().contains("line 1"));
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn a_failed_write_replies_without_a_path() {
        let dir = TestDir::new("host-nopath");
        let inbox = dir.join("inbox");
        // A folder where the partial file belongs makes the write fail.
        std::fs::create_dir_all(inbox.join(format!("{ID}.{}.partial", std::process::id())))
            .unwrap();
        let replies = run(&[deliver(ID, &capture(ID, 0))], &inbox);
        assert!(!replies[0].ok);
        let error = replies[0].error.as_deref().unwrap();
        assert_eq!(error, "could not write capture");
        assert!(!error.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn an_inbox_that_cannot_be_made_replies_without_a_path() {
        let dir = TestDir::new("host-nopath-inbox");
        let blocker = dir.write("blocker", b"a file, not a folder");
        let replies = run(&[deliver(ID, &capture(ID, 0))], &blocker.join("inbox"));
        let error = replies[0].error.as_deref().unwrap();
        assert_eq!(error, "could not create inbox");
        assert!(!error.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn undecodable_json_gets_a_failed_reply() {
        let dir = TestDir::new("host-junk");
        let replies = run(&[b"{nope".to_vec()], &dir.join("inbox"));
        assert_eq!(replies.len(), 1);
        assert!(!replies[0].ok);
    }

    #[test]
    fn a_repeat_delivery_overwrites() {
        let dir = TestDir::new("host-repeat");
        let inbox = dir.join("inbox");
        let first = capture(ID, 1);
        let second = capture(ID, 3);
        let replies = run(&[deliver(ID, &first), deliver(ID, &second)], &inbox);
        assert!(replies.iter().all(|r| r.ok));
        assert_eq!(
            std::fs::read_to_string(inbox.join(format!("{ID}.jsonl"))).unwrap(),
            second
        );
        assert_eq!(files_under(&inbox).len(), 1);
    }

    #[test]
    fn several_messages_are_answered_in_order() {
        let dir = TestDir::new("host-many");
        let inbox = dir.join("inbox");
        let ping = br#"{"type":"ping"}"#.to_vec();
        let replies = run(&[ping.clone(), deliver(ID, &capture(ID, 0)), ping], &inbox);
        assert_eq!(replies.len(), 3);
        assert!(replies[0].version.is_some());
        assert_eq!(replies[1], HostReply::stored());
        assert!(replies[2].version.is_some());
    }

    fn read_last_seen(dir: &std::path::Path) -> endstep_capture::LastSeen {
        let text = std::fs::read_to_string(dir.join("last_seen.json")).unwrap();
        endstep_capture::parse_last_seen(&text).unwrap()
    }

    #[test]
    fn a_ping_writes_last_seen() {
        let dir = TestDir::new("host-seen-ping");
        let inbox = dir.join("inbox");
        let ping = br#"{"type":"ping","browser":"chrome","extension":"0.2.0"}"#.to_vec();
        run(&[ping], &inbox);
        let seen = read_last_seen(dir.path());
        assert_eq!(seen.at, 1_000);
        assert_eq!(seen.browser, "chrome");
        assert_eq!(seen.extension, "0.2.0");
        assert!(
            files_under(dir.path())
                .iter()
                .all(|p| { p.extension().is_none_or(|e| e != "partial") })
        );
    }

    #[test]
    fn an_old_ping_writes_unknown() {
        let dir = TestDir::new("host-seen-old");
        run(&[br#"{"type":"ping"}"#.to_vec()], &dir.join("inbox"));
        let seen = read_last_seen(dir.path());
        assert_eq!(seen.browser, "unknown");
        assert_eq!(seen.extension, "unknown");
    }

    #[test]
    fn a_delivery_writes_last_seen_from_its_header() {
        let dir = TestDir::new("host-seen-deliver");
        let inbox = dir.join("inbox");
        run_at(&[deliver(ID, &capture(ID, 1))], &inbox, 7_000);
        let seen = read_last_seen(dir.path());
        assert_eq!(seen.at, 7_000);
        assert_eq!(seen.browser, "firefox");
        assert_eq!(seen.extension, "0.1.0");
    }

    #[test]
    fn a_failed_last_seen_write_never_changes_the_reply() {
        let dir = TestDir::new("host-seen-fail");
        // A folder where the file belongs makes the rename fail.
        std::fs::create_dir_all(dir.join("last_seen.json")).unwrap();
        let replies = run(&[br#"{"type":"ping"}"#.to_vec()], &dir.join("inbox"));
        assert!(replies[0].ok);
        assert!(replies[0].version.is_some());
    }

    #[test]
    fn a_refused_delivery_and_a_bad_request_still_write_last_seen() {
        let dir = TestDir::new("host-seen-errors");
        let inbox = dir.join("inbox");
        run_at(&[deliver(ID, "not a capture\n")], &inbox, 3_000);
        let seen = read_last_seen(dir.path());
        assert_eq!(seen.at, 3_000);
        assert_eq!(seen.browser, "unknown");
        assert_eq!(seen.extension, "unknown");

        run_at(&[b"{nope".to_vec()], &inbox, 4_000);
        let seen = read_last_seen(dir.path());
        assert_eq!(seen.at, 4_000);
        assert_eq!(seen.browser, "unknown");
    }

    #[test]
    fn a_refused_delivery_and_a_bad_request_keep_the_known_browser() {
        let dir = TestDir::new("host-seen-keeps");
        let inbox = dir.join("inbox");
        let ping = br#"{"type":"ping","browser":"chrome","extension":"0.2.0"}"#.to_vec();
        run_at(&[ping], &inbox, 1_000);

        run_at(&[deliver(ID, "not a capture\n")], &inbox, 3_000);
        let seen = read_last_seen(dir.path());
        assert_eq!(seen.at, 3_000);
        assert_eq!(seen.browser, "chrome");
        assert_eq!(seen.extension, "0.2.0");

        run_at(&[b"{nope".to_vec()], &inbox, 4_000);
        let seen = read_last_seen(dir.path());
        assert_eq!(seen.at, 4_000);
        assert_eq!(seen.browser, "chrome");
        assert_eq!(seen.extension, "0.2.0");
    }

    fn part(unit_id: &str, index: u32, content: &str) -> Vec<u8> {
        serde_json::json!({"type":"deliver_part","unit_id":unit_id,"index":index,"content":content})
            .to_string()
            .into_bytes()
    }

    fn end(unit_id: &str, parts: u32) -> Vec<u8> {
        serde_json::json!({"type":"deliver_end","unit_id":unit_id,"parts":parts})
            .to_string()
            .into_bytes()
    }

    /// Splits a capture after its header line and after every frame line.
    fn split_lines(content: &str) -> Vec<String> {
        content.split_inclusive('\n').map(str::to_string).collect()
    }

    #[test]
    fn parts_assemble_to_exactly_the_joined_content() {
        let dir = TestDir::new("host-parts");
        let inbox = dir.join("inbox");
        let content = capture(ID, 5);
        let pieces = split_lines(&content);
        let mut messages = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            messages.push(part(ID, index as u32, piece));
        }
        messages.push(end(ID, pieces.len() as u32));
        let replies = run_at(&messages, &inbox, 8_000);
        assert!(replies.iter().all(|reply| *reply == HostReply::stored()));
        assert_eq!(
            std::fs::read_to_string(inbox.join(format!("{ID}.jsonl"))).unwrap(),
            content
        );
        assert_eq!(capture_files_under(dir.path()).len(), 1);
        assert_eq!(read_last_seen(dir.path()).at, 8_000);
    }

    #[test]
    fn a_part_out_of_order_is_refused_and_leaves_no_partial() {
        let dir = TestDir::new("host-part-order");
        let inbox = dir.join("inbox");
        let pieces = split_lines(&capture(ID, 2));
        let replies = run(&[part(ID, 0, &pieces[0]), part(ID, 2, &pieces[1])], &inbox);
        assert!(replies[0].ok);
        assert_eq!(replies[1].error.as_deref(), Some("part out of order"));
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn a_later_part_with_nothing_pending_is_refused() {
        let dir = TestDir::new("host-part-orphan");
        let replies = run(&[part(ID, 1, "x\n")], &dir.join("inbox"));
        assert_eq!(replies[0].error.as_deref(), Some("part out of order"));
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn an_end_with_the_wrong_part_count_is_refused() {
        let dir = TestDir::new("host-part-count");
        let inbox = dir.join("inbox");
        let content = capture(ID, 1);
        let replies = run(&[part(ID, 0, &content), end(ID, 2)], &inbox);
        assert!(replies[0].ok);
        assert!(!replies[1].ok);
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn an_end_with_nothing_pending_is_refused() {
        let dir = TestDir::new("host-part-lone-end");
        let replies = run(&[end(ID, 1)], &dir.join("inbox"));
        assert!(!replies[0].ok);
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn a_bad_unit_id_in_part_zero_writes_nothing() {
        let dir = TestDir::new("host-part-badid");
        let inbox = dir.join("a/b/inbox");
        let replies = run(&[part("../../x", 0, "x\n")], &inbox);
        assert_eq!(replies[0].error.as_deref(), Some("invalid unit id"));
        assert!(capture_files_under(dir.path()).is_empty());
        assert!(!inbox.exists());
    }

    #[test]
    fn input_ending_mid_unit_leaves_no_partial() {
        let dir = TestDir::new("host-part-cut");
        let inbox = dir.join("inbox");
        let pieces = split_lines(&capture(ID, 3));
        let replies = run(&[part(ID, 0, &pieces[0]), part(ID, 1, &pieces[1])], &inbox);
        assert!(replies.iter().all(|reply| reply.ok));
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn an_end_validates_the_assembled_content() {
        let dir = TestDir::new("host-part-badcontent");
        let inbox = dir.join("inbox");
        let replies = run(
            &[part(ID, 0, "not a "), part(ID, 1, "capture\n"), end(ID, 2)],
            &inbox,
        );
        assert!(replies[1].ok);
        assert!(replies[2].error.as_deref().unwrap().contains("line 1"));
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn an_end_for_content_of_another_unit_is_refused() {
        let dir = TestDir::new("host-part-otherunit");
        let inbox = dir.join("inbox");
        let replies = run(&[part(ID, 0, &capture(OTHER_ID, 0)), end(ID, 1)], &inbox);
        assert!(!replies[1].ok);
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn a_part_for_a_different_unit_is_refused() {
        let dir = TestDir::new("host-part-switch");
        let inbox = dir.join("inbox");
        let pieces = split_lines(&capture(ID, 2));
        let replies = run(
            &[part(ID, 0, &pieces[0]), part(OTHER_ID, 1, &pieces[1])],
            &inbox,
        );
        assert!(replies[0].ok);
        assert_eq!(replies[1].error.as_deref(), Some("part out of order"));
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn a_new_part_zero_drops_the_unit_it_replaces() {
        let dir = TestDir::new("host-part-restart");
        let inbox = dir.join("inbox");
        let first = capture(ID, 1);
        let second = capture(OTHER_ID, 1);
        let replies = run(
            &[
                part(ID, 0, &first),
                part(OTHER_ID, 0, &second),
                end(OTHER_ID, 1),
            ],
            &inbox,
        );
        assert!(replies.iter().all(|reply| reply.ok));
        let files = capture_files_under(dir.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with(format!("{OTHER_ID}.jsonl")));
    }

    #[test]
    fn a_capture_over_the_limit_is_refused() {
        let dir = TestDir::new("host-part-limit");
        let inbox = dir.join("inbox");
        let content = capture(ID, 2);
        let pieces = split_lines(&content);
        let limit = content.len() as u64 - 1;
        let mut input = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            write_message(&mut input, &part(ID, index as u32, piece)).unwrap();
        }
        let mut output = Vec::new();
        serve_with_limit(
            &mut Cursor::new(input),
            &mut output,
            &inbox,
            &|| 1_000,
            limit,
        )
        .unwrap();
        let mut reader = Cursor::new(output);
        let mut replies: Vec<HostReply> = Vec::new();
        while let Some(body) = endstep_capture::read_message(&mut reader).unwrap() {
            replies.push(serde_json::from_slice(&body).unwrap());
        }
        assert!(replies[0].ok && replies[1].ok);
        assert_eq!(replies[2].error.as_deref(), Some("capture too large"));
        assert!(capture_files_under(dir.path()).is_empty());
    }

    #[test]
    fn a_failed_part_writes_last_seen() {
        let dir = TestDir::new("host-part-seen");
        run_at(&[part(ID, 1, "x\n")], &dir.join("inbox"), 5_000);
        assert_eq!(read_last_seen(dir.path()).at, 5_000);
    }

    #[test]
    fn inbox_sits_beside_the_executable() {
        let inbox = inbox_beside(std::path::Path::new("/opt/mymtgo/mymtgo-endstep-host"));
        assert_eq!(inbox, std::path::PathBuf::from("/opt/mymtgo/inbox"));
    }
}
