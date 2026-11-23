//! p50/p95 of the timing lines in MyMTGO's log files (spec 8), for the
//! reference latency figures. Lines use `tracing`'s text format, for example
//! `2026-10-07T10:00:00.123456Z DEBUG mymtgo::timing: tick poll_us=40 total_us=90 events=3`.
//! Detection to UI is `tick.total` plus `push.emit`.

use std::collections::BTreeMap;

const TIMING_TARGET: &str = "mymtgo::timing: ";

/// Commands whose time includes waiting (for the user, a dialog, the
/// tracker's commit, or the network), so their figures are not latency.
pub const WAITING_COMMANDS: [&str; 3] = ["save_settings", "choose_log_folder", "check_for_updates"];

/// One timing line: its kind (`tick`, `push`, `command`) and `key=value` fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingLine {
    pub kind: String,
    pub fields: Vec<(String, String)>,
}

impl TimingLine {
    pub fn text(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn number(&self, key: &str) -> Option<u64> {
        self.text(key)?.parse().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measure {
    pub name: String,
    pub count: usize,
    pub p50_us: u64,
    pub p95_us: u64,
    pub max_us: u64,
    pub includes_waiting: bool,
}

/// The timing line in `line`, if it is one.
pub fn parse_line(line: &str) -> Option<TimingLine> {
    let start = line.find(TIMING_TARGET)? + TIMING_TARGET.len();
    let mut words = line[start..].split_whitespace();
    let kind = words.next()?.to_string();
    let mut fields = Vec::new();
    for word in words {
        if let Some((key, value)) = word.split_once('=') {
            fields.push((key.to_string(), value.trim_matches('"').to_string()));
        }
    }
    Some(TimingLine { kind, fields })
}

/// Measure every timing line in `text` (log files, joined).
pub fn summarise(text: &str) -> Vec<Measure> {
    let mut samples: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for line in text.lines() {
        let Some(timing) = parse_line(line) else {
            continue;
        };
        match timing.kind.as_str() {
            "tick" => {
                // Idle ticks would bury the ticks that did work.
                if timing.number("events").unwrap_or(0) == 0 {
                    continue;
                }
                for (key, value) in &timing.fields {
                    if let (Some(phase), Ok(us)) = (key.strip_suffix("_us"), value.parse::<u64>()) {
                        samples.entry(format!("tick.{phase}")).or_default().push(us);
                    }
                }
            }
            "push" => {
                if let Some(us) = timing.number("emit_us") {
                    samples.entry("push.emit".to_string()).or_default().push(us);
                }
            }
            "command" => {
                if let (Some(name), Some(us)) =
                    (timing.text("command"), timing.number("elapsed_us"))
                {
                    samples
                        .entry(format!("command.{name}"))
                        .or_default()
                        .push(us);
                }
            }
            _ => {}
        }
    }
    samples
        .into_iter()
        .map(|(name, values)| measure(name, values))
        .collect()
}

/// A plain text table, one row per measure.
pub fn table(measures: &[Measure]) -> String {
    let mut out = format!(
        "{:<36} {:>8} {:>10} {:>10} {:>10}\n",
        "measure", "count", "p50_us", "p95_us", "max_us"
    );
    for m in measures {
        let note = if m.includes_waiting {
            "  (includes waiting)"
        } else {
            ""
        };
        out.push_str(&format!(
            "{:<36} {:>8} {:>10} {:>10} {:>10}{note}\n",
            m.name, m.count, m.p50_us, m.p95_us, m.max_us
        ));
    }
    out
}

fn measure(name: String, mut values: Vec<u64>) -> Measure {
    values.sort_unstable();
    let includes_waiting = name
        .strip_prefix("command.")
        .is_some_and(|command| WAITING_COMMANDS.contains(&command));
    Measure {
        count: values.len(),
        p50_us: percentile(&values, 50),
        p95_us: percentile(&values, 95),
        max_us: values.last().copied().unwrap_or(0),
        includes_waiting,
        name,
    }
}

/// Nearest-rank percentile of sorted values (0 for none).
fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: &str = "2026-10-07T10:00:00.123456Z DEBUG mymtgo::timing: tick rescan_us=12 poll_us=40 lookups_us=1 finalize_us=2 rebuild_us=0 decks_us=0 write_us=30 total_us=90 events=3";
    const IDLE_TICK: &str = "2026-10-07T10:00:00.200000Z DEBUG mymtgo::timing: tick rescan_us=0 poll_us=5 lookups_us=0 finalize_us=0 rebuild_us=0 decks_us=0 write_us=0 total_us=6 events=0";
    const PUSH: &str =
        "2026-10-07T10:00:00.124000Z DEBUG mymtgo::timing: push emit_us=250 events=2";
    const COMMAND: &str = "2026-10-07T10:00:01.000000Z DEBUG mymtgo::timing: command command=\"list_matches\" elapsed_us=800";
    const SAVE: &str = "2026-10-07T10:00:02.000000Z DEBUG mymtgo::timing: command command=\"save_settings\" elapsed_us=90000";

    fn find<'a>(measures: &'a [Measure], name: &str) -> &'a Measure {
        measures
            .iter()
            .find(|measure| measure.name == name)
            .unwrap_or_else(|| panic!("no measure {name} in {measures:?}"))
    }

    #[test]
    fn a_timing_line_is_parsed_and_other_lines_are_not() {
        let line = parse_line(COMMAND).expect("parsed");
        assert_eq!(line.kind, "command");
        assert_eq!(line.text("command"), Some("list_matches"));
        assert_eq!(line.number("elapsed_us"), Some(800));
        assert_eq!(
            parse_line("2026-10-07T10:00:00Z  INFO app: MyMTGO starting"),
            None
        );
    }

    #[test]
    fn ticks_pushes_and_commands_are_measured() {
        let measures = summarise(&[TICK, PUSH, COMMAND].join("\n"));
        assert_eq!(find(&measures, "tick.total").p50_us, 90);
        assert_eq!(find(&measures, "tick.poll").p50_us, 40);
        assert_eq!(find(&measures, "push.emit").p50_us, 250);
        assert_eq!(find(&measures, "command.list_matches").p50_us, 800);
        assert!(measures.iter().all(|measure| measure.name != "tick.events"));
    }

    #[test]
    fn idle_ticks_are_left_out() {
        let measures = summarise(&[TICK, IDLE_TICK].join("\n"));
        assert_eq!(find(&measures, "tick.total").count, 1);
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let lines: Vec<String> = (1..=100)
            .map(|us| format!("x DEBUG mymtgo::timing: push emit_us={us} events=1"))
            .collect();
        let push = find(&summarise(&lines.join("\n")), "push.emit").clone();
        assert_eq!(
            (push.count, push.p50_us, push.p95_us, push.max_us),
            (100, 50, 95, 100)
        );
    }

    #[test]
    fn commands_that_wait_are_flagged() {
        let measures = summarise(&[COMMAND, SAVE].join("\n"));
        assert!(find(&measures, "command.save_settings").includes_waiting);
        assert!(!find(&measures, "command.list_matches").includes_waiting);
    }

    #[test]
    fn the_table_has_one_row_per_measure_and_marks_waiting() {
        let text = table(&summarise(&[TICK, SAVE].join("\n")));
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("measure"), "{text}");
        assert!(text.contains("tick.total"), "{text}");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("command.save_settings")
                    && line.contains("includes waiting")),
            "{text}"
        );
    }
}
