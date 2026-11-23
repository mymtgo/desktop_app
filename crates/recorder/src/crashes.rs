//! The crash limit: too many exits in a short time stops the respawning.

/// Exits that trip the limit.
const LIMIT: usize = 5;
/// The window the limit is counted in (10 minutes).
const WINDOW_MS: i64 = 600_000;

#[derive(Debug, Default)]
pub struct CrashWindow {
    exits_ms: Vec<i64>,
}

impl CrashWindow {
    /// Records an exit at `now_ms`. True when this exit trips the limit
    /// (5 exits within 10 minutes, every exit counts).
    pub fn record(&mut self, now_ms: i64) -> bool {
        self.exits_ms.retain(|&at| now_ms - at < WINDOW_MS);
        self.exits_ms.push(now_ms);
        self.exits_ms.len() >= LIMIT
    }
}
