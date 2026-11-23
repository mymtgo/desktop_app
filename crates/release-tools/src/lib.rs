//! Builds the update feed's `latest.json` (the Tauri updater format) for a
//! release (spec 9). A wrong feed breaks updates for every tester, so every
//! input is checked and the tool refuses rather than guessing. The
//! `licences` module builds the About page's third-party list.

pub mod licences;
pub mod prune;

use std::path::PathBuf;

use serde_json::{Map, json};

/// The platforms we ship, as the updater names them.
pub const PLATFORMS: [&str; 3] = ["windows-x86_64", "linux-x86_64", "darwin-aarch64"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformFile {
    pub platform: String,
    /// The file's name under `<base url>/<version>/`.
    pub file_name: String,
    /// The `.sig` file's content.
    pub signature: String,
}

/// `latest.json` for `version`, its files under `<base_url>/<version>/`.
pub fn latest_json(
    version: &str,
    notes: &str,
    pub_date: &str,
    base_url: &str,
    files: &[PlatformFile],
) -> Result<String, String> {
    check_version(version)?;
    chrono::DateTime::parse_from_rfc3339(pub_date)
        .map_err(|error| format!("the publish date {pub_date} is not RFC 3339: {error}"))?;
    if !base_url.starts_with("https://") || base_url.ends_with('/') {
        return Err(format!(
            "the base URL {base_url} must be https and have no trailing slash"
        ));
    }
    if files.is_empty() {
        return Err("no platform files".to_string());
    }
    let mut platforms = Map::new();
    for file in files {
        if !PLATFORMS.contains(&file.platform.as_str()) {
            return Err(format!("{} is not a platform we ship", file.platform));
        }
        if platforms.contains_key(&file.platform) {
            return Err(format!("{} is given twice", file.platform));
        }
        if file.file_name.is_empty() || file.file_name.contains([' ', '/', '\\']) {
            return Err(format!(
                "the file name {:?} must have no spaces or slashes",
                file.file_name
            ));
        }
        let signature = file.signature.trim();
        if signature.is_empty() || signature.contains('\n') {
            return Err(format!(
                "the signature for {} must be one non-empty line",
                file.platform
            ));
        }
        platforms.insert(
            file.platform.clone(),
            json!({
                "signature": signature,
                "url": format!("{base_url}/{version}/{}", file.file_name),
            }),
        );
    }
    let feed = json!({
        "version": version,
        "notes": notes.trim(),
        "pub_date": pub_date,
        "platforms": platforms,
    });
    serde_json::to_string_pretty(&feed).map_err(|error| error.to_string())
}

/// `1.2.3` or `1.2.3-rc.1`: what the updater compares.
fn check_version(version: &str) -> Result<(), String> {
    let (core, pre_release) = match version.split_once('-') {
        Some((core, pre_release)) => (core, Some(pre_release)),
        None => (version, None),
    };
    let numeric = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    let core_ok = core.split('.').count() == 3 && core.split('.').all(numeric);
    // Each pre-release part (`beta`, `1`) is letters, digits or hyphens.
    let pre_release_ok = pre_release.is_none_or(|pre_release| {
        pre_release.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    });
    if core_ok && pre_release_ok {
        Ok(())
    } else {
        Err(format!(
            "{version} is not a version like 1.0.1 or 1.1.0-rc.1"
        ))
    }
}

/// The bucket folder a version is uploaded to: `edge` for a pre-release
/// (`1.1.0-rc.1`), `stable` for a release.
pub fn channel_folder(version: &str) -> Result<&'static str, String> {
    check_version(version)?;
    if version.contains('-') {
        Ok("edge")
    } else {
        Ok("stable")
    }
}

/// The command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub version: String,
    pub notes_file: PathBuf,
    pub pub_date: String,
    pub base_url: String,
    /// `(platform, installer path)`; the signature is read from `<path>.sig`.
    pub platforms: Vec<(String, PathBuf)>,
    pub out: PathBuf,
}

pub const USAGE: &str = "usage: release-tools --version <x.y.z> --notes-file <path> --pub-date <rfc3339> --base-url <https://.../stable> --platform <name>=<installer> [--platform ...] --out <latest.json>";

pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut version = None;
    let mut notes_file = None;
    let mut pub_date = None;
    let mut base_url = None;
    let mut out = None;
    let mut platforms = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .filter(|value| !value.starts_with("--"))
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag {
            "--version" => version = Some(value),
            "--notes-file" => notes_file = Some(PathBuf::from(value)),
            "--pub-date" => pub_date = Some(value),
            "--base-url" => base_url = Some(value),
            "--out" => out = Some(PathBuf::from(value)),
            "--platform" => {
                let (name, path) = value
                    .split_once('=')
                    .ok_or_else(|| format!("--platform needs <name>=<installer>\n{USAGE}"))?;
                platforms.push((name.to_string(), PathBuf::from(path)));
            }
            _ => return Err(format!("unknown argument {flag}\n{USAGE}")),
        }
        index += 2;
    }
    let missing = |name: &str| format!("{name} is required\n{USAGE}");
    Ok(Args {
        version: version.ok_or_else(|| missing("--version"))?,
        notes_file: notes_file.ok_or_else(|| missing("--notes-file"))?,
        pub_date: pub_date.ok_or_else(|| missing("--pub-date"))?,
        base_url: base_url.ok_or_else(|| missing("--base-url"))?,
        platforms,
        out: out.ok_or_else(|| missing("--out"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn files() -> Vec<PlatformFile> {
        vec![
            PlatformFile {
                platform: "windows-x86_64".to_string(),
                file_name: "MyMTGO-Beta_0.1.1_x64-setup.exe".to_string(),
                signature: "c2lnbmF0dXJlIHdpbmRvd3M=\n".to_string(),
            },
            PlatformFile {
                platform: "linux-x86_64".to_string(),
                file_name: "MyMTGO-Beta_0.1.1_amd64.AppImage".to_string(),
                signature: "c2lnbmF0dXJlIGxpbnV4".to_string(),
            },
        ]
    }

    const URL: &str = "https://updates.example.com/beta";
    const DATE: &str = "2026-10-08T09:00:00Z";

    #[test]
    fn the_feed_points_each_platform_at_its_signed_file() {
        let text = latest_json("0.1.1", "  Fixes.\n", DATE, URL, &files()).expect("feed");
        let feed: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(feed["version"], "0.1.1");
        assert_eq!(feed["notes"], "Fixes.");
        assert_eq!(feed["pub_date"], DATE);
        assert_eq!(
            feed["platforms"]["windows-x86_64"]["url"],
            "https://updates.example.com/beta/0.1.1/MyMTGO-Beta_0.1.1_x64-setup.exe"
        );
        assert_eq!(
            feed["platforms"]["windows-x86_64"]["signature"],
            "c2lnbmF0dXJlIHdpbmRvd3M="
        );
        assert_eq!(
            feed["platforms"]["linux-x86_64"]["url"],
            "https://updates.example.com/beta/0.1.1/MyMTGO-Beta_0.1.1_amd64.AppImage"
        );
    }

    #[test]
    fn a_feed_that_would_break_updates_is_refused() {
        let ok = files();
        assert!(
            latest_json("v0.1.1", "", DATE, URL, &ok).is_err(),
            "a v prefix"
        );
        assert!(latest_json("0.1", "", DATE, URL, &ok).is_err(), "two parts");
        assert!(
            latest_json("0.1.1", "", "yesterday", URL, &ok).is_err(),
            "a date that is not RFC 3339"
        );
        assert!(
            latest_json("0.1.1", "", DATE, "http://updates.example.com/beta", &ok).is_err(),
            "not https"
        );
        assert!(
            latest_json("0.1.1", "", DATE, "https://updates.example.com/beta/", &ok).is_err(),
            "trailing slash"
        );
        assert!(
            latest_json("0.1.1", "", DATE, URL, &[]).is_err(),
            "no platforms"
        );

        let mut spaced = files();
        spaced[0].file_name = "MyMTGO Beta_0.1.1_x64-setup.exe".to_string();
        assert!(
            latest_json("0.1.1", "", DATE, URL, &spaced).is_err(),
            "a space in a file name"
        );
        let mut unsigned = files();
        unsigned[1].signature = " \n".to_string();
        assert!(
            latest_json("0.1.1", "", DATE, URL, &unsigned).is_err(),
            "an empty signature"
        );
        let mut unknown = files();
        unknown[1].platform = "darwin-x86_64".to_string();
        assert!(
            latest_json("0.1.1", "", DATE, URL, &unknown).is_err(),
            "a platform we do not ship"
        );
        let mut twice = files();
        twice[1].platform = "windows-x86_64".to_string();
        assert!(
            latest_json("0.1.1", "", DATE, URL, &twice).is_err(),
            "a platform twice"
        );
    }

    #[test]
    fn the_mac_package_is_a_platform_we_ship() {
        let mut with_mac = files();
        with_mac.push(PlatformFile {
            platform: "darwin-aarch64".to_string(),
            file_name: "MyMTGO_1.3.0_aarch64.app.tar.gz".to_string(),
            signature: "c2lnbmF0dXJlIG1hYw==".to_string(),
        });
        let text = latest_json("1.3.0", "", DATE, URL, &with_mac).expect("feed");
        let feed: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(
            feed["platforms"]["darwin-aarch64"]["url"],
            "https://updates.example.com/beta/1.3.0/MyMTGO_1.3.0_aarch64.app.tar.gz"
        );
        assert_eq!(
            feed["platforms"]["linux-x86_64"]["url"],
            "https://updates.example.com/beta/1.3.0/MyMTGO-Beta_0.1.1_amd64.AppImage"
        );
    }

    #[test]
    fn a_beta_version_is_published_under_its_own_folder() {
        let text = latest_json("1.0.0-beta.1", "", DATE, URL, &files()).expect("feed");
        let feed: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(feed["version"], "1.0.0-beta.1");
        let url = feed["platforms"]["windows-x86_64"]["url"]
            .as_str()
            .expect("url");
        assert!(url.starts_with(&format!("{URL}/1.0.0-beta.1/")), "{url}");
        let ok = files();
        for bad in ["1.0.0-", "1.0.0-beta..1", "1.0.0-beta_1", "1.0.0-be ta"] {
            assert!(latest_json(bad, "", DATE, URL, &ok).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_pre_release_goes_to_edge_and_a_release_to_stable() {
        assert_eq!(channel_folder("1.0.0-beta.8"), Ok("edge"));
        assert_eq!(channel_folder("1.1.0-rc.2"), Ok("edge"));
        assert_eq!(channel_folder("1.0.0"), Ok("stable"));
        assert_eq!(channel_folder("1.0.1"), Ok("stable"));
        assert!(channel_folder("1.0").is_err());
    }

    #[test]
    fn arguments_are_parsed() {
        let args: Vec<String> = [
            "--version",
            "0.1.1",
            "--notes-file",
            "notes.txt",
            "--pub-date",
            DATE,
            "--base-url",
            URL,
            "--platform",
            "windows-x86_64=dist/a.exe",
            "--platform",
            "linux-x86_64=dist/b.AppImage",
            "--out",
            "dist/latest.json",
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
        let parsed = parse_args(&args).expect("parsed");
        assert_eq!(parsed.version, "0.1.1");
        assert_eq!(
            parsed.platforms,
            vec![
                ("windows-x86_64".to_string(), PathBuf::from("dist/a.exe")),
                ("linux-x86_64".to_string(), PathBuf::from("dist/b.AppImage")),
            ]
        );
        assert_eq!(parsed.out, PathBuf::from("dist/latest.json"));
        assert!(
            parse_args(&args[..args.len() - 2]).is_err(),
            "--out is required"
        );
        assert!(parse_args(&["--platform".to_string(), "no-equals".to_string()]).is_err());
    }
}
