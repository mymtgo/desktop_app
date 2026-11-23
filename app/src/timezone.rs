//! The OS timezone: only an input for dating log lines (spec 5.4). Stored times
//! are UTC; the UI shows them in the viewer's timezone.

use chrono_tz::Tz;

/// The OS timezone, or UTC plus a warning for the status bar.
pub fn system_timezone() -> (Tz, Option<String>) {
    resolve(iana_time_zone::get_timezone().map_err(|error| error.to_string()))
}

/// Turn the OS answer into a timezone. Never fails: an unusable answer dates
/// events in UTC and says why.
pub fn resolve(name: Result<String, String>) -> (Tz, Option<String>) {
    match name {
        Ok(name) if name.trim().is_empty() => (
            Tz::UTC,
            Some("Your system timezone is empty, so match times are dated in UTC.".to_string()),
        ),
        Ok(name) => match name.trim().parse::<Tz>() {
            Ok(tz) => (tz, None),
            Err(_) => (
                Tz::UTC,
                Some(format!(
                    "Your system timezone \"{name}\" is not recognised, so match times are dated in UTC."
                )),
            ),
        },
        Err(error) => (
            Tz::UTC,
            Some(format!(
                "Your system timezone could not be read ({error}), so match times are dated in UTC."
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_iana_name_is_used_as_is() {
        let (tz, warning) = resolve(Ok("Europe/London".to_string()));
        assert_eq!(tz, Tz::Europe__London);
        assert_eq!(warning, None);
    }

    #[test]
    fn an_unknown_timezone_falls_back_to_utc_with_a_warning() {
        for name in ["Not/AZone", "Pacific Standard Time", ""] {
            let (tz, warning) = resolve(Ok(name.to_string()));
            assert_eq!(tz, Tz::UTC, "{name}");
            let warning = warning.expect("a warning");
            assert!(warning.contains("UTC"), "{warning}");
        }
    }

    #[test]
    fn an_empty_timezone_says_it_is_empty() {
        let (tz, warning) = resolve(Ok("  ".to_string()));
        assert_eq!(tz, Tz::UTC);
        let warning = warning.expect("a warning");
        assert!(warning.contains("is empty"), "{warning}");
        assert!(warning.contains("UTC"), "{warning}");
    }

    #[test]
    fn a_missing_timezone_falls_back_to_utc_with_a_warning() {
        let (tz, warning) = resolve(Err("no TZ set".to_string()));
        assert_eq!(tz, Tz::UTC);
        let warning = warning.expect("a warning");
        assert!(warning.contains("no TZ set"), "{warning}");
        assert!(warning.contains("UTC"), "{warning}");
    }
}
