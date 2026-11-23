//! What a recorder exit code means.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// MTGO closed or the host asked it to stop (code 0).
    Stopped,
    /// Consent was not accepted (code 2).
    ConsentNotAccepted,
    /// Bad arguments (code 3).
    BadArguments,
    /// MTGO is not supported: attach or subscribe failed (code 4).
    MtgoNotSupported,
    /// Another recorder is running (code 5).
    AlreadyRunning,
    /// Any other code, or killed by a signal.
    Crashed,
}

/// Maps an exit code to its kind. `None` means the process was killed.
pub fn exit_kind(code: Option<i32>) -> ExitKind {
    match code {
        Some(0) => ExitKind::Stopped,
        Some(2) => ExitKind::ConsentNotAccepted,
        Some(3) => ExitKind::BadArguments,
        Some(4) => ExitKind::MtgoNotSupported,
        Some(5) => ExitKind::AlreadyRunning,
        _ => ExitKind::Crashed,
    }
}

/// Whether the supervisor starts the recorder again after this exit.
pub fn respawns(kind: ExitKind) -> bool {
    matches!(kind, ExitKind::Stopped | ExitKind::Crashed)
}
