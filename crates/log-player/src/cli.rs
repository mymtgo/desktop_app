//! Command line parsing for the `log-player` binary, kept pure so it is tested.

use std::path::PathBuf;

pub const USAGE: &str = "usage: log-player --input <mtgo.log> --output <folder> [--speed 1.0]";

/// What the binary was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Pauses are divided by this; 0 means no pauses.
    pub speed: f64,
}

/// Parse the arguments after the program name. Every flag needs a value (not
/// another flag), each flag may appear once, and anything else is an error.
/// The error text ends with the usage line.
pub fn parse_args(args: &[String]) -> Result<PlayerArgs, String> {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut speed: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let slot = match flag {
            "--input" => &mut input,
            "--output" => &mut output,
            "--speed" => &mut speed,
            _ => return Err(with_usage(&format!("unknown argument {flag}"))),
        };
        let value = match args.get(index + 1) {
            Some(value) if !value.starts_with("--") => value.clone(),
            _ => return Err(with_usage(&format!("{flag} needs a value"))),
        };
        if slot.is_some() {
            return Err(with_usage(&format!("{flag} given twice")));
        }
        *slot = Some(value);
        index += 2;
    }

    let Some(input) = input else {
        return Err(with_usage("--input is required"));
    };
    let Some(output) = output else {
        return Err(with_usage("--output is required"));
    };
    let speed = match speed {
        Some(text) => parse_speed(&text)?,
        None => 1.0,
    };
    Ok(PlayerArgs {
        input: PathBuf::from(input),
        output: PathBuf::from(output),
        speed,
    })
}

fn parse_speed(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(speed) if speed.is_finite() && speed >= 0.0 => Ok(speed),
        _ => Err(with_usage(&format!(
            "--speed must be a finite number of at least 0, got {text}"
        ))),
    }
}

fn with_usage(problem: &str) -> String {
    format!("{problem}\n{USAGE}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn reads_input_output_and_speed() {
        let parsed = parse_args(&args(&[
            "--input", "in.log", "--output", "out", "--speed", "2.5",
        ]))
        .expect("parsed");
        assert_eq!(
            parsed,
            PlayerArgs {
                input: PathBuf::from("in.log"),
                output: PathBuf::from("out"),
                speed: 2.5,
            }
        );
    }

    #[test]
    fn speed_defaults_to_real_time_and_zero_means_no_pauses() {
        let parsed = parse_args(&args(&["--output", "out", "--input", "in.log"])).expect("parsed");
        assert_eq!(parsed.speed, 1.0);
        let fast =
            parse_args(&args(&["--input", "a", "--output", "b", "--speed", "0"])).expect("parsed");
        assert_eq!(fast.speed, 0.0);
    }

    #[test]
    fn a_flag_followed_by_another_flag_is_an_error() {
        let error = parse_args(&args(&["--input", "--output", "out"])).expect_err("error");
        assert!(error.contains("--input needs a value"), "{error}");
        assert!(error.contains(USAGE), "{error}");
    }

    #[test]
    fn a_flag_without_a_value_is_an_error() {
        let error = parse_args(&args(&["--output", "out", "--input"])).expect_err("error");
        assert!(error.contains("--input needs a value"), "{error}");
    }

    #[test]
    fn unknown_flags_and_stray_arguments_are_errors() {
        let error = parse_args(&args(&["--input", "a", "--output", "b", "--sped", "2"]))
            .expect_err("error");
        assert!(error.contains("unknown argument --sped"), "{error}");
        let error = parse_args(&args(&["--input", "a", "b"])).expect_err("error");
        assert!(error.contains("unknown argument b"), "{error}");
    }

    #[test]
    fn missing_input_or_output_is_an_error() {
        assert!(parse_args(&args(&["--output", "out"])).is_err());
        assert!(parse_args(&args(&["--input", "in.log"])).is_err());
        assert!(parse_args(&[]).is_err());
    }

    #[test]
    fn a_flag_given_twice_is_an_error() {
        let error = parse_args(&args(&["--input", "a", "--input", "b", "--output", "c"]))
            .expect_err("error");
        assert!(error.contains("--input given twice"), "{error}");
    }

    #[test]
    fn speed_must_be_a_finite_number_of_at_least_zero() {
        for bad in ["fast", "-1", "NaN", "inf", "-0.5"] {
            let error = parse_args(&args(&["--input", "a", "--output", "b", "--speed", bad]))
                .expect_err(bad);
            assert!(error.contains("--speed"), "{bad}: {error}");
        }
    }
}
