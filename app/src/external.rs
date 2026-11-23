//! Opening links in the user's browser. The webview cannot: `target="_blank"`
//! does nothing on Linux (WebKitGTK) and opens an in-app popup on Windows
//! (WebView2). Only `https` links to hosts on `ALLOWED_HOSTS` are opened, and
//! links to the configured MyMTGO API (sign-in, the website's pages): `https`,
//! or `http` when the API itself is a loopback dev host (07 spec section 3).

use tauri::{AppHandle, Manager, Url};
use tauri_plugin_opener::OpenerExt;

use crate::api::session::ApiState;
use crate::commands::CommandError;

/// Hosts the app may open in the browser: decklist.org, Discord for Get
/// help's "Open Discord" (release spec section 8.1), and the recorder notice's
/// links on the Replays page: the MTGO EULA, MTGOSDK on GitHub and the
/// recorder's notices beside its build on the release host, and the Endstep
/// extension's store listings (Chrome Web Store and Firefox Add-ons) behind the
/// Integrations page's store buttons.
pub const ALLOWED_HOSTS: [&str; 8] = [
    "decklist.org",
    "www.decklist.org",
    "discord.gg",
    "www.mtgo.com",
    "github.com",
    "releases.mymtgo.com",
    "chromewebstore.google.com",
    "addons.mozilla.org",
];

/// The link as the browser should get it, or a refusal. The link must be
/// `https`, name an allowed host exactly, and carry no user name, password or
/// port. The returned text is the parser's own serialisation, so the opener
/// never sees a form another parser could read differently.
pub fn checked_external_url(url: &str) -> Result<String, CommandError> {
    let parsed = Url::parse(url).map_err(|_| refused())?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return Err(refused());
    }
    // `host_str` is lowercase for https links, with percent escapes decoded.
    let allowed = parsed
        .host_str()
        .is_some_and(|host| ALLOWED_HOSTS.contains(&host));
    if !allowed {
        return Err(refused());
    }
    Ok(parsed.to_string())
}

/// A link to the configured API, as the browser should get it, or a
/// refusal: the same scheme, host and port as `api_base_url` (already
/// checked at start: `https`, or `http` only for a loopback host) and no
/// user name or password.
pub fn checked_api_url(url: &str, api_base_url: &str) -> Result<String, CommandError> {
    let parsed = Url::parse(url).map_err(|_| refused())?;
    let base = Url::parse(api_base_url).map_err(|_| refused())?;
    let same_origin = parsed.scheme() == base.scheme()
        && parsed.host_str().is_some()
        && parsed.host_str() == base.host_str()
        && parsed.port() == base.port();
    let secure = parsed.scheme() == "https"
        || (parsed.scheme() == "http"
            && parsed
                .host_str()
                .is_some_and(|host| mymtgo_api::config::LOOPBACK_HOSTS.contains(&host)));
    if !same_origin || !secure || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(refused());
    }
    Ok(parsed.to_string())
}

/// A decklist link, or, when the API is configured, a link to it.
pub fn checked_link(url: &str, api_base_url: Option<&str>) -> Result<String, CommandError> {
    match checked_external_url(url) {
        Ok(url) => Ok(url),
        Err(refusal) => match api_base_url {
            Some(base) => checked_api_url(url, base),
            None => Err(refusal),
        },
    }
}

fn refused() -> CommandError {
    CommandError::Refused("This link cannot be opened.".to_string())
}

/// Hand a checked link to the browser through the official opener plugin
/// (Rust side only: the plugin's JS commands get no permission).
fn open_checked(app: &AppHandle, url: String) -> Result<(), CommandError> {
    app.opener().open_url(url, None::<&str>).map_err(|error| {
        tracing::warn!(%error, "the browser could not be opened");
        CommandError::Refused("The browser could not be opened.".to_string())
    })
}

/// The configured API's base URL, when it is configured.
fn api_base_url(app: &AppHandle) -> Option<String> {
    let state = app.try_state::<ApiState>()?;
    state.config().ok().map(|config| config.base_url.clone())
}

/// Open a page of the configured API (the sign-in authorize page).
pub fn open_api_url(app: &AppHandle, url: &str) -> Result<(), String> {
    let base = api_base_url(app).ok_or_else(|| "the API is not configured".to_string())?;
    let url = checked_api_url(url, &base).map_err(|_| "the link was refused".to_string())?;
    open_checked(app, url).map_err(|_| "the browser could not be opened".to_string())
}

/// Open an allowed link (decklist, Discord, or the configured API) in the user's
/// browser.
#[tauri::command]
#[specta::specta]
pub fn open_external(app: AppHandle, url: String) -> Result<(), CommandError> {
    let url = checked_link(&url, api_base_url(&app).as_deref())?;
    open_checked(&app, url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_refused(url: &str) -> bool {
        matches!(checked_external_url(url), Err(CommandError::Refused(_)))
    }

    #[test]
    fn an_https_decklist_link_is_accepted_as_written() {
        let url = "https://decklist.org/?deckmain=4%20Qcard&format=modern";
        assert_eq!(checked_external_url(url), Ok(url.to_string()));
        assert_eq!(
            checked_external_url("https://www.decklist.org/"),
            Ok("https://www.decklist.org/".to_string())
        );
    }

    #[test]
    fn the_recorder_notice_links_are_accepted() {
        for url in [
            "https://www.mtgo.com/eula",
            "https://github.com/videre-project/MTGOSDK",
            "https://github.com/videre-project/MTGOSDK/blob/main/LICENSE",
            "https://releases.mymtgo.com/mtgr-record/0.1.0/NOTICE",
            "https://releases.mymtgo.com/mtgr-record/0.1.0/THIRD-PARTY-NOTICES.md",
        ] {
            assert_eq!(checked_external_url(url), Ok(url.to_string()));
        }
        assert!(is_refused(
            "http://releases.mymtgo.com/mtgr-record/0.1.0/NOTICE"
        ));
        assert!(is_refused("https://evil.github.com/x"));
    }

    #[test]
    fn the_extension_store_listings_are_accepted() {
        for url in [
            "https://chromewebstore.google.com/detail/mymtgo-endstep/abcdefghijklmnopabcdefghijklmnop",
            "https://addons.mozilla.org/en-US/firefox/addon/mymtgo-endstep/",
        ] {
            assert_eq!(checked_external_url(url), Ok(url.to_string()));
        }
        assert!(is_refused(
            "https://chromewebstore.google.com.evil.example/x"
        ));
        assert!(is_refused("https://addons.mozilla.org.evil.example/x"));
        assert!(is_refused("http://chromewebstore.google.com/x"));
    }

    #[test]
    fn the_discord_invite_is_accepted() {
        // Get help's "Open Discord" (release spec section 8.1).
        let url = "https://discord.gg/KuFv4Jcm38";
        assert_eq!(checked_external_url(url), Ok(url.to_string()));
        assert!(is_refused("https://evil.discord.gg/KuFv4Jcm38"));
        assert!(is_refused("http://discord.gg/KuFv4Jcm38"));
    }

    #[test]
    fn the_host_is_compared_without_case_and_the_link_is_normalised() {
        assert_eq!(
            checked_external_url("https://DeckList.ORG"),
            Ok("https://decklist.org/".to_string())
        );
    }

    #[test]
    fn other_schemes_are_refused() {
        for url in [
            "http://decklist.org/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://decklist.org/",
            "decklist.org",
            "//decklist.org/",
            "",
        ] {
            assert!(is_refused(url), "{url}");
        }
    }

    #[test]
    fn other_hosts_are_refused() {
        for url in [
            "https://evil.com/",
            "https://example.org/decklist.org",
            "https://notdecklist.org/",
            "https://sub.decklist.org/",
            "https://127.0.0.1/",
        ] {
            assert!(is_refused(url), "{url}");
        }
    }

    #[test]
    fn hosts_that_only_look_allowed_are_refused() {
        for url in [
            "https://decklist.org.evil.com/",
            "https://evil.com/decklist.org",
            "https://evil.com?https://decklist.org",
            "https://evil.com#decklist.org",
            "https://decklist.org@evil.com/",
            "https://user:pass@decklist.org/",
            "https://decklist.org:8443/",
            "https://decklist.org%2eevil.com/",
            " https://evil.com/ https://decklist.org/",
        ] {
            assert!(is_refused(url), "{url}");
        }
    }

    #[test]
    fn a_backslash_is_a_path_separator_so_the_link_stays_on_the_allowed_host() {
        // Browsers read `\` as `/` in https links; the opener gets the
        // normalised form, which cannot be read as a user name.
        assert_eq!(
            checked_external_url("https://decklist.org\\@evil.com/"),
            Ok("https://decklist.org/@evil.com/".to_string())
        );
    }

    // Q3: the configured API host joins the allow-list (sign-in opens its
    // authorize page), https only unless the API itself is a loopback dev
    // host on http.
    #[test]
    fn the_api_host_is_allowed() {
        let base = "https://mymtgo.test";
        let url = "https://mymtgo.test/oauth/authorize?client_id=q&state=s";
        assert_eq!(checked_api_url(url, base), Ok(url.to_string()));
        assert_eq!(
            checked_api_url("https://MyMTGO.test/settings/devices", base),
            Ok("https://mymtgo.test/settings/devices".to_string())
        );
        assert!(checked_api_url("https://mymtgo.test/x", "https://mymtgo.test/api-root").is_ok());
    }

    #[test]
    fn http_to_the_api_host_other_hosts_and_other_ports_are_refused() {
        let base = "https://mymtgo.test";
        for url in [
            "http://mymtgo.test/oauth/authorize",
            "https://evil.com/oauth/authorize",
            "https://mymtgo.test.evil.com/",
            "https://mymtgo.test:8443/oauth/authorize",
            "https://user:pass@mymtgo.test/",
            "https://decklist.org/",
            "javascript:alert(1)",
            "",
        ] {
            assert!(
                matches!(checked_api_url(url, base), Err(CommandError::Refused(_))),
                "{url}"
            );
        }
    }

    #[test]
    fn a_loopback_dev_api_is_allowed_on_http_and_its_own_port() {
        let base = "http://127.0.0.1:8123";
        assert_eq!(
            checked_api_url("http://127.0.0.1:8123/oauth/authorize?x=1", base),
            Ok("http://127.0.0.1:8123/oauth/authorize?x=1".to_string())
        );
        for url in [
            "http://127.0.0.1:9999/oauth/authorize",
            "http://127.0.0.1/oauth/authorize",
            "https://127.0.0.1:8123/oauth/authorize",
            "http://localhost:8123/oauth/authorize",
        ] {
            assert!(checked_api_url(url, base).is_err(), "{url}");
        }
        assert!(checked_api_url("http://[::1]:9000/oauth/authorize", "http://[::1]:9000").is_ok());
    }

    #[test]
    fn decklist_links_still_open_beside_the_api() {
        let api = Some("https://mymtgo.test");
        assert!(checked_link("https://decklist.org/", api).is_ok());
        assert!(checked_link("https://mymtgo.test/settings", api).is_ok());
        assert!(checked_link("https://mymtgo.test/settings", None).is_err());
        assert!(checked_link("https://evil.com/", api).is_err());
    }
}
