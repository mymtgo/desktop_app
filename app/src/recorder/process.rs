//! The recorder process itself: starting it, stopping it gracefully, and
//! reading the status file it writes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use recorder::{PIN, RecorderState, parse_status};

/// How long a graceful stop waits before killing.
const STOP_GRACE: Duration = Duration::from_secs(10);
/// Where the recorder writes its status (Windows only).
pub fn status_file_in(local_app_data: Option<OsString>) -> Option<PathBuf> {
    let base = PathBuf::from(local_app_data?);
    Some(base.join("MtgoReplay").join("status.txt"))
}

/// Start the recorder: no console window, stdin piped (closing it is the
/// stop signal), stdout and stderr discarded.
pub fn spawn(exe: &Path, folder: &Path) -> std::io::Result<Child> {
    let mut command = Command::new(exe);
    command
        .args(["--output"])
        .arg(folder)
        .args(["--parent-pid", &std::process::id().to_string()])
        .args(["--host-accepted", &PIN.consent_version.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if debug_captures(env!("CARGO_PKG_VERSION")) {
        command.arg("--debug-captures");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        command.creation_flags(0x0800_0000);
    }
    command.spawn()
}

/// Whether the recorder keeps each match's `Match_<token>.log` and captures,
/// so a game can become a recorder test recording: on edge builds (a
/// prerelease version such as `1.2.0-rc.8`) and development builds. Stable
/// releases never do: the captures folder is never pruned and holds
/// opponents' names. The risk notice says so.
pub fn debug_captures(version: &str) -> bool {
    cfg!(debug_assertions) || crate::identity::is_prerelease(version)
}

/// The recorder's state from its status file, if the file was written since
/// `since` (an older one is from an earlier run).
pub fn read_status(path: &Path, since: SystemTime) -> Option<RecorderState> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    if modified < since {
        return None;
    }
    parse_status(&std::fs::read_to_string(path).ok()?)
}

/// Close stdin, wait up to `STOP_GRACE` for the exit, then kill.
pub fn stop_child(child: &mut Child) {
    drop(child.stdin.take());
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_builds_keep_debug_captures() {
        assert!(debug_captures("1.2.0-rc.8"));
        assert!(debug_captures("1.3.0-beta.1+build.5"));
    }

    #[test]
    fn stable_builds_keep_them_only_in_development() {
        assert_eq!(debug_captures("1.2.0"), cfg!(debug_assertions));
        assert_eq!(debug_captures("1.2.0+build.5"), cfg!(debug_assertions));
    }

    #[test]
    fn the_status_file_is_in_local_app_data() {
        let path = status_file_in(Some(OsString::from("C:/L"))).expect("path");
        assert!(path.ends_with("MtgoReplay/status.txt"));
        assert_eq!(status_file_in(None), None);
    }
}
