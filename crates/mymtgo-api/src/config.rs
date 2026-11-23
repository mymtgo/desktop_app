//! Where the API is and who the app is to it (07 spec section 3). Values come
//! from the build (baked from the build environment or the repo's git-ignored
//! `.env.local`) and from the process environment at app start, which wins
//! (in a release build, only for the keys outside `DEV_ONLY_KEYS`).
//! Like Laravel's `config/mymtgo_api.php` reading `env()`, but the reading of
//! files and variables happens in the app; this module only decides.

use std::fmt;

pub const API_URL_KEY: &str = "MYMTGO_API_URL";
pub const CLIENT_ID_KEY: &str = "MYMTGO_OAUTH_CLIENT_ID";
pub const CA_KEY: &str = "MYMTGO_API_CA";
/// The keys configuration reads, from the build and from the process.
pub const CONFIG_KEYS: [&str; 3] = [API_URL_KEY, CLIENT_ID_KEY, CA_KEY];
pub const DEFAULT_API_URL: &str = "https://mymtgo.com";
/// Hosts that may be reached over plain `http` (tests and local dev).
pub const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

/// The resolved configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    /// Scheme, host, optional port and optional path, no trailing slash.
    pub base_url: String,
    /// The host name alone, lowercase, without port (`[::1]` keeps its
    /// brackets).
    pub host: String,
    /// The OAuth client id; sign-in is disabled without one.
    pub client_id: Option<String>,
    /// A PEM file whose certificates are the only trusted roots (dev only).
    pub ca_pem_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The base URL could not be read.
    InvalidUrl(String),
    /// `http` to a host that is not a loopback address.
    InsecureUrl(String),
    /// A debug build with no base URL: the API is off, so a build from a
    /// fresh clone never talks to the production API.
    NotSet,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidUrl(url) => write!(f, "{API_URL_KEY} is not a valid URL: {url}"),
            ConfigError::InsecureUrl(url) => write!(
                f,
                "{API_URL_KEY} must use https (http only for 127.0.0.1, localhost or [::1]): {url}"
            ),
            ConfigError::NotSet => write!(
                f,
                "{API_URL_KEY} is not set, so the API is off in this debug build (set it in .env.local)"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Read a `.env` file: `KEY=VALUE` lines, `#` comments, blank lines, an
/// optional `export ` prefix, one pair of matching quotes around the value
/// stripped. No interpolation and no escapes. Lines without `=` are skipped.
pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        pairs.push((key.to_string(), unquote(value.trim()).to_string()));
    }
    pairs
}

/// The value without one pair of matching quotes around it.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// Resolve the configuration: for each key the process value wins over the
/// baked one; an empty value counts as unset.
pub fn resolve_config(
    baked: &[(String, String)],
    process: &dyn Fn(&str) -> Option<String>,
) -> Result<ApiConfig, ConfigError> {
    let value = |key: &str| -> Option<String> {
        let from_process = process(key).filter(|value| !value.is_empty());
        from_process.or_else(|| {
            baked
                .iter()
                .find(|(name, value)| name == key && !value.is_empty())
                .map(|(_, value)| value.clone())
        })
    };
    let url = value(API_URL_KEY).unwrap_or_else(|| DEFAULT_API_URL.to_string());
    let (base_url, host) = check_base_url(&url)?;
    Ok(ApiConfig {
        base_url,
        host,
        client_id: value(CLIENT_ID_KEY),
        ca_pem_path: value(CA_KEY),
    })
}

/// Keys only a debug build takes from the process environment: in a
/// release build another program that sets them could point the app at its
/// own server, or make it trust its own certificate. Their baked values
/// still apply.
pub const DEV_ONLY_KEYS: [&str; 2] = [API_URL_KEY, CA_KEY];

/// Whether the process value of `key` may override the baked one: always in
/// a debug build, never for a developer-only key in a release build.
pub fn process_override_allowed(key: &str, debug: bool) -> bool {
    debug || !DEV_ONLY_KEYS.contains(&key)
}

/// `resolve_config` for a build: `debug` is `cfg!(debug_assertions)`. A
/// release build ignores the developer-only keys in the process environment.
/// A debug build without a base URL has the API off (`ConfigError::NotSet`)
/// instead of using `DEFAULT_API_URL`.
pub fn resolve_config_for_build(
    baked: &[(String, String)],
    process: &dyn Fn(&str) -> Option<String>,
    debug: bool,
) -> Result<ApiConfig, ConfigError> {
    let allowed = |key: &str| -> Option<String> {
        if process_override_allowed(key, debug) {
            process(key)
        } else {
            None
        }
    };
    if debug && !url_is_set(baked, &allowed) {
        return Err(ConfigError::NotSet);
    }
    resolve_config(baked, &allowed)
}

/// Whether a non-empty base URL comes from the process or the build.
fn url_is_set(baked: &[(String, String)], process: &dyn Fn(&str) -> Option<String>) -> bool {
    let from_process = process(API_URL_KEY).is_some_and(|value| !value.is_empty());
    from_process
        || baked
            .iter()
            .any(|(name, value)| name == API_URL_KEY && !value.is_empty())
}

/// The base URL without trailing slashes and its host, or why it is refused.
fn check_base_url(url: &str) -> Result<(String, String), ConfigError> {
    let invalid = || ConfigError::InvalidUrl(without_userinfo(url));
    let trimmed = url.trim().trim_end_matches('/');
    let (scheme, rest) = trimmed.split_once("://").ok_or_else(invalid)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(ConfigError::InsecureUrl(without_userinfo(url)));
    }
    if rest.contains(['?', '#', '@']) || rest.chars().any(|c| c.is_whitespace()) {
        return Err(invalid());
    }
    let authority = rest.split('/').next().unwrap_or("");
    let (host, port) = split_authority(authority).ok_or_else(invalid)?;
    if host.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let host = host.to_ascii_lowercase();
    if scheme == "http" && !LOOPBACK_HOSTS.contains(&host.as_str()) {
        return Err(ConfigError::InsecureUrl(without_userinfo(url)));
    }
    Ok((trimmed.to_string(), host))
}

/// The URL as it may be shown in an error: a `user:password@` part before
/// the host becomes `<redacted>@`, so a refused URL never leaks a password.
fn without_userinfo(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), url),
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    match authority.rfind('@') {
        Some(at) => format!("{scheme}<redacted>{}{tail}", &authority[at..]),
        None => url.to_string(),
    }
}

/// Host and port (empty when absent) of `host[:port]` or `[v6][:port]`.
fn split_authority(authority: &str) -> Option<(&str, &str)> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let rest = &authority[end + 1..];
        let port = if rest.is_empty() {
            ""
        } else {
            rest.strip_prefix(':')?
        };
        return Some((host, port));
    }
    match authority.split_once(':') {
        Some((host, port)) => Some((host, port)),
        None => Some((authority, "")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn no_process(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn env_files_read_comments_quotes_and_export() {
        let text = "# local API\n\
            \n\
            MYMTGO_API_URL=https://mymtgo.test\n\
            export MYMTGO_OAUTH_CLIENT_ID=\"qclient-id\"\n\
            MYMTGO_API_CA='/home/qdev/ca.pem'\n\
            \x20 SPACED = padded value \n\
            HALF=\"open\n\
            EMPTY=\n\
            NOT A PAIR\n\
            KEEP=a=b#c\n\
            MYMTGO_SENTRY_DSN=ignored-by-config\r\n";
        assert_eq!(
            parse_env_file(text),
            pairs(&[
                ("MYMTGO_API_URL", "https://mymtgo.test"),
                ("MYMTGO_OAUTH_CLIENT_ID", "qclient-id"),
                ("MYMTGO_API_CA", "/home/qdev/ca.pem"),
                ("SPACED", "padded value"),
                ("HALF", "\"open"),
                ("EMPTY", ""),
                ("KEEP", "a=b#c"),
                ("MYMTGO_SENTRY_DSN", "ignored-by-config"),
            ])
        );
    }

    #[test]
    fn unknown_keys_are_ignored_by_the_config() {
        let baked = pairs(&[
            ("MYMTGO_SENTRY_DSN", "https://qkey@example.invalid/1"),
            ("OTHER", "x"),
        ]);
        let config = resolve_config(&baked, &no_process).expect("valid");
        assert_eq!(
            config,
            ApiConfig {
                base_url: "https://mymtgo.com".to_string(),
                host: "mymtgo.com".to_string(),
                client_id: None,
                ca_pem_path: None,
            }
        );
    }

    #[test]
    fn the_default_url_is_mymtgo_com_and_the_client_id_may_be_missing() {
        let config = resolve_config(&[], &no_process).expect("valid");
        assert_eq!(config.base_url, DEFAULT_API_URL);
        assert_eq!(config.client_id, None);
        let empty = pairs(&[("MYMTGO_OAUTH_CLIENT_ID", ""), ("MYMTGO_API_URL", "")]);
        let config = resolve_config(&empty, &no_process).expect("valid");
        assert_eq!(config.base_url, DEFAULT_API_URL);
        assert_eq!(config.client_id, None, "an empty value is unset");
    }

    #[test]
    fn process_values_override_baked_ones() {
        let baked = pairs(&[
            ("MYMTGO_API_URL", "https://mymtgo.com"),
            ("MYMTGO_OAUTH_CLIENT_ID", "baked-client"),
        ]);
        let process = |key: &str| match key {
            "MYMTGO_API_URL" => Some("https://MyMTGO.test/".to_string()),
            "MYMTGO_API_CA" => Some("/home/qdev/ca.pem".to_string()),
            _ => None,
        };
        let config = resolve_config(&baked, &process).expect("valid");
        assert_eq!(config.base_url, "https://MyMTGO.test");
        assert_eq!(config.host, "mymtgo.test");
        assert_eq!(config.client_id.as_deref(), Some("baked-client"));
        assert_eq!(config.ca_pem_path.as_deref(), Some("/home/qdev/ca.pem"));
    }

    #[test]
    fn a_release_build_ignores_the_developer_overrides() {
        let baked = pairs(&[
            ("MYMTGO_API_URL", "https://baked.mymtgo.com"),
            ("MYMTGO_OAUTH_CLIENT_ID", "baked-client"),
        ]);
        let process = |key: &str| match key {
            "MYMTGO_API_URL" => Some("https://attacker.example".to_string()),
            "MYMTGO_API_CA" => Some("/tmp/attacker-ca.pem".to_string()),
            "MYMTGO_OAUTH_CLIENT_ID" => Some("process-client".to_string()),
            _ => None,
        };
        let release = resolve_config_for_build(&baked, &process, false).expect("valid");
        assert_eq!(release.base_url, "https://baked.mymtgo.com");
        assert_eq!(release.ca_pem_path, None);
        assert_eq!(release.client_id.as_deref(), Some("process-client"));

        let debug = resolve_config_for_build(&baked, &process, true).expect("valid");
        assert_eq!(debug.base_url, "https://attacker.example");
        assert_eq!(debug.ca_pem_path.as_deref(), Some("/tmp/attacker-ca.pem"));
    }

    #[test]
    fn a_debug_build_without_a_url_has_the_api_off() {
        let error = resolve_config_for_build(&[], &no_process, true).expect_err("off");
        assert_eq!(error, ConfigError::NotSet);
        let empty = pairs(&[("MYMTGO_API_URL", "")]);
        let error = resolve_config_for_build(&empty, &no_process, true).expect_err("off");
        assert_eq!(error, ConfigError::NotSet, "an empty value is unset");

        let baked = pairs(&[("MYMTGO_API_URL", "https://mymtgo.com")]);
        let debug = resolve_config_for_build(&baked, &no_process, true).expect("valid");
        assert_eq!(debug.base_url, "https://mymtgo.com");
        let process = |key: &str| (key == API_URL_KEY).then(|| "https://mymtgo.test".to_string());
        let debug = resolve_config_for_build(&[], &process, true).expect("valid");
        assert_eq!(debug.base_url, "https://mymtgo.test");

        let release = resolve_config_for_build(&[], &no_process, false).expect("valid");
        assert_eq!(
            release.base_url, DEFAULT_API_URL,
            "a release build keeps the default"
        );
    }

    #[test]
    fn only_the_url_and_ca_are_developer_overrides() {
        assert!(!process_override_allowed(API_URL_KEY, false));
        assert!(!process_override_allowed(CA_KEY, false));
        assert!(process_override_allowed(CLIENT_ID_KEY, false));
        for key in CONFIG_KEYS {
            assert!(process_override_allowed(key, true), "{key}");
        }
    }

    #[test]
    fn http_is_refused_for_a_public_host() {
        for url in ["http://mymtgo.com", "http://mymtgo.test:8080", "ftp://x"] {
            let baked = pairs(&[("MYMTGO_API_URL", url)]);
            assert!(resolve_config(&baked, &no_process).is_err(), "{url}");
        }
        assert!(matches!(
            resolve_config(
                &pairs(&[("MYMTGO_API_URL", "http://mymtgo.com")]),
                &no_process
            ),
            Err(ConfigError::InsecureUrl(_))
        ));
    }

    #[test]
    fn http_is_allowed_for_loopback_hosts() {
        for (url, host) in [
            ("http://127.0.0.1:8123", "127.0.0.1"),
            ("http://localhost/", "localhost"),
            ("http://[::1]:9000/api-root/", "[::1]"),
        ] {
            let config = resolve_config(&pairs(&[("MYMTGO_API_URL", url)]), &no_process)
                .unwrap_or_else(|error| panic!("{url}: {error}"));
            assert_eq!(config.host, host);
            assert!(!config.base_url.ends_with('/'), "{}", config.base_url);
        }
    }

    #[test]
    fn malformed_urls_are_refused() {
        for url in [
            "mymtgo.com",
            "https://",
            "https://qdev:secret@mymtgo.com",
            "https://mymtgo.com?x=1",
            "https://mymtgo.com:port",
            "https://my mtgo.com",
        ] {
            assert!(
                matches!(
                    resolve_config(&pairs(&[("MYMTGO_API_URL", url)]), &no_process),
                    Err(ConfigError::InvalidUrl(_))
                ),
                "{url}"
            );
        }
        let with_password = pairs(&[("MYMTGO_API_URL", "https://qdev:qsecret@mymtgo.com/api")]);
        let error = resolve_config(&with_password, &no_process).expect_err("refused");
        let shown = format!("{error} {error:?}");
        assert!(
            !shown.contains("qsecret") && !shown.contains("qdev"),
            "{shown}"
        );
        assert!(shown.contains("mymtgo.com"), "{shown}");
        let insecure = pairs(&[("MYMTGO_API_URL", "ftp://qdev:qsecret@mymtgo.com")]);
        let error = resolve_config(&insecure, &no_process).expect_err("refused");
        assert!(!format!("{error} {error:?}").contains("qsecret"), "{error}");
    }
}
