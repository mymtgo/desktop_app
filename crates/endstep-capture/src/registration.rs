//! Where the Endstep helper is registered with the browsers, and the native
//! messaging manifests that point them at it. Pure: every path is built from
//! the `home` folder passed in; MyMTGO's app writes the files and registry
//! keys (`app/src/endstep/host.rs`).

use std::path::{Path, PathBuf};

/// The native messaging host name the extension asks for
/// (`chrome.runtime.sendNativeMessage('com.mymtgo.endstep', ...)`).
pub const HOST_NAME: &str = "com.mymtgo.endstep";

/// The Chrome extension ids allowed to reach the helper. The first is the dev
/// build's id (from the key in `extension/src/manifest.ts`).
///
/// Owner: after the first Chrome Web Store upload, append the store id here
/// (32 letters a to p, from the listing's developer dashboard, not a URL).
/// Ship that app version before or with the listing going public. An app that
/// does not list the id answers the store extension with "helper not found".
pub const CHROME_EXTENSION_IDS: &[&str] = &[
    "oimkfhogicccpdcbddlfofkmghfhhlgb",
    // The Chrome Web Store listing.
    "cbmldoaipgkmodemgdfbajogkdabcopn",
];

/// The Firefox extension id (`browser_specific_settings.gecko.id`).
pub const FIREFOX_EXTENSION_ID: &str = "endstep@mymtgo.com";

/// The manifest's description, shown by some browsers.
const DESCRIPTION: &str = "MyMTGO Endstep capture";

/// The operating systems MyMTGO registers the helper on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

/// The two manifest shapes: Chrome, Edge, Brave, Chromium and Vivaldi share
/// one, Firefox has its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

/// Where a browser looks for the manifest: a file at a fixed path (macOS and
/// Linux), or a registry key under `HKCU` whose default value is the
/// manifest's path (Windows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    File(PathBuf),
    Registry(String),
}

/// One browser's registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub browser: &'static str,
    pub family: Family,
    pub place: Place,
}

impl Target {
    /// The folder whose presence shows the browser is installed: the folder
    /// above `NativeMessagingHosts` (or `native-messaging-hosts`) for a file
    /// target, such as `~/.config/google-chrome` or `~/.mozilla`. `None` for a
    /// registry target: the app writes its key whether or not the browser is
    /// there, as before.
    pub fn browser_folder(&self) -> Option<&Path> {
        match &self.place {
            Place::File(path) => path.parent().and_then(Path::parent),
            Place::Registry(_) => None,
        }
    }
}

/// The helper's file name on `os`.
pub fn helper_file_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "mymtgo-endstep-host.exe",
        Os::MacOs | Os::Linux => "mymtgo-endstep-host",
    }
}

/// The Chromium family manifest for the helper at `helper`.
pub fn chrome_manifest(helper: &str) -> String {
    let mut origins = Vec::new();
    for id in CHROME_EXTENSION_IDS {
        origins.push(format!("chrome-extension://{id}/"));
    }
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": DESCRIPTION,
        "path": helper,
        "type": "stdio",
        "allowed_origins": origins,
    });
    pretty(&manifest)
}

/// The Firefox manifest for the helper at `helper`.
pub fn firefox_manifest(helper: &str) -> String {
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": DESCRIPTION,
        "path": helper,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
    });
    pretty(&manifest)
}

fn pretty(value: &serde_json::Value) -> String {
    // A `Value` built by `json!` always serialises.
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Every place the helper is registered on `os`, for the user whose home
/// folder is `home`.
pub fn targets(os: Os, home: &Path) -> Vec<Target> {
    let file_name = format!("{HOST_NAME}.json");
    match os {
        Os::MacOs => {
            let support = home.join("Library").join("Application Support");
            let chromium_browsers = [
                ("Chrome", support.join("Google").join("Chrome")),
                ("Chromium", support.join("Chromium")),
                ("Brave", support.join("BraveSoftware").join("Brave-Browser")),
                ("Edge", support.join("Microsoft Edge")),
                ("Vivaldi", support.join("Vivaldi")),
            ];
            let mut list = Vec::new();
            for (browser, folder) in chromium_browsers {
                list.push(file_target(
                    browser,
                    Family::Chromium,
                    folder.join("NativeMessagingHosts").join(&file_name),
                ));
            }
            list.push(file_target(
                "Firefox",
                Family::Firefox,
                support
                    .join("Mozilla")
                    .join("NativeMessagingHosts")
                    .join(&file_name),
            ));
            list
        }
        Os::Linux => {
            let config = home.join(".config");
            let chromium_browsers = [
                ("Chrome", config.join("google-chrome")),
                ("Chromium", config.join("chromium")),
                ("Brave", config.join("BraveSoftware").join("Brave-Browser")),
                ("Edge", config.join("microsoft-edge")),
                ("Vivaldi", config.join("vivaldi")),
            ];
            let mut list = Vec::new();
            for (browser, folder) in chromium_browsers {
                list.push(file_target(
                    browser,
                    Family::Chromium,
                    folder.join("NativeMessagingHosts").join(&file_name),
                ));
            }
            list.push(file_target(
                "Firefox",
                Family::Firefox,
                home.join(".mozilla")
                    .join("native-messaging-hosts")
                    .join(&file_name),
            ));
            list
        }
        Os::Windows => vec![
            registry_target("Chrome", Family::Chromium, r"Software\Google\Chrome"),
            registry_target("Edge", Family::Chromium, r"Software\Microsoft\Edge"),
            registry_target("Firefox", Family::Firefox, r"Software\Mozilla"),
        ],
    }
}

fn file_target(browser: &'static str, family: Family, path: PathBuf) -> Target {
    Target {
        browser,
        family,
        place: Place::File(path),
    }
}

fn registry_target(browser: &'static str, family: Family, vendor: &str) -> Target {
    Target {
        browser,
        family,
        place: Place::Registry(format!(r"HKCU\{vendor}\NativeMessagingHosts\{HOST_NAME}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("manifest is JSON")
    }

    fn file_path(target: &Target) -> PathBuf {
        match &target.place {
            Place::File(path) => path.clone(),
            Place::Registry(key) => panic!("expected a file, got {key}"),
        }
    }

    #[test]
    fn a_file_target_is_proved_by_the_folder_above_its_hosts_folder() {
        let home = Path::new("/home/player");
        let list = targets(Os::Linux, home);
        let folder_of = |browser: &str| {
            list.iter()
                .find(|target| target.browser == browser)
                .and_then(|target| target.browser_folder())
                .map(Path::to_path_buf)
        };
        assert_eq!(
            folder_of("Chrome"),
            Some(PathBuf::from("/home/player/.config/google-chrome"))
        );
        assert_eq!(
            folder_of("Brave"),
            Some(PathBuf::from(
                "/home/player/.config/BraveSoftware/Brave-Browser"
            ))
        );
        assert_eq!(
            folder_of("Firefox"),
            Some(PathBuf::from("/home/player/.mozilla"))
        );
        let mac = targets(Os::MacOs, Path::new("/Users/player"));
        let mac_chrome = mac.iter().find(|t| t.browser == "Chrome").unwrap();
        assert_eq!(
            mac_chrome.browser_folder(),
            Some(Path::new(
                "/Users/player/Library/Application Support/Google/Chrome"
            ))
        );
        let mac_firefox = mac.iter().find(|t| t.browser == "Firefox").unwrap();
        assert_eq!(
            mac_firefox.browser_folder(),
            Some(Path::new(
                "/Users/player/Library/Application Support/Mozilla"
            ))
        );
    }

    #[test]
    fn a_registry_target_has_no_browser_folder() {
        for target in targets(Os::Windows, Path::new("C:/Users/player")) {
            assert_eq!(target.browser_folder(), None);
        }
    }

    #[test]
    fn chrome_manifest_lists_the_extension_origins() {
        let manifest = parse(&chrome_manifest(r"C:\Users\Player\mymtgo-endstep-host.exe"));
        assert_eq!(manifest["name"], "com.mymtgo.endstep");
        assert_eq!(manifest["type"], "stdio");
        assert_eq!(manifest["path"], r"C:\Users\Player\mymtgo-endstep-host.exe");
        assert_eq!(
            manifest["allowed_origins"][0],
            "chrome-extension://oimkfhogicccpdcbddlfofkmghfhhlgb/"
        );
        assert!(manifest.get("allowed_extensions").is_none());
    }

    #[test]
    fn every_listed_id_is_an_allowed_origin() {
        let manifest = parse(&chrome_manifest("/home/player/mymtgo-endstep-host"));
        let origins: Vec<String> = manifest["allowed_origins"]
            .as_array()
            .expect("origins are a list")
            .iter()
            .map(|origin| origin.as_str().expect("origin is text").to_string())
            .collect();
        assert_eq!(origins.len(), CHROME_EXTENSION_IDS.len());
        for id in CHROME_EXTENSION_IDS {
            let is_id = id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte));
            assert!(
                is_id,
                "CHROME_EXTENSION_IDS entry {id:?} is not an id: paste only the id from the Chrome Web Store developer dashboard, 32 lower-case letters a to p, no URL, no spaces"
            );
            assert!(origins.contains(&format!("chrome-extension://{id}/")));
        }
        for (index, id) in CHROME_EXTENSION_IDS.iter().enumerate() {
            assert!(
                !CHROME_EXTENSION_IDS[..index].contains(id),
                "CHROME_EXTENSION_IDS lists {id} twice: remove the duplicate"
            );
        }
    }

    #[test]
    fn firefox_manifest_lists_the_gecko_id() {
        let manifest = parse(&firefox_manifest("/home/player/mymtgo-endstep-host"));
        assert_eq!(manifest["name"], "com.mymtgo.endstep");
        assert_eq!(manifest["type"], "stdio");
        assert_eq!(manifest["path"], "/home/player/mymtgo-endstep-host");
        assert_eq!(
            manifest["allowed_extensions"],
            serde_json::json!(["endstep@mymtgo.com"])
        );
        assert!(manifest.get("allowed_origins").is_none());
    }

    #[test]
    fn mac_targets_cover_each_chromium_browser_and_firefox() {
        let home = Path::new("/Users/player");
        let list = targets(Os::MacOs, home);
        let folders = [
            "Google/Chrome",
            "Chromium",
            "BraveSoftware/Brave-Browser",
            "Microsoft Edge",
            "Vivaldi",
        ];
        assert_eq!(list.len(), folders.len() + 1);
        for (target, folder) in list.iter().zip(folders) {
            assert_eq!(target.family, Family::Chromium);
            assert_eq!(
                file_path(target),
                home.join(format!(
                    "Library/Application Support/{folder}/NativeMessagingHosts/com.mymtgo.endstep.json"
                ))
            );
        }
        let firefox = list.last().expect("firefox");
        assert_eq!(firefox.family, Family::Firefox);
        assert_eq!(
            file_path(firefox),
            home.join(
                "Library/Application Support/Mozilla/NativeMessagingHosts/com.mymtgo.endstep.json"
            )
        );
    }

    #[test]
    fn linux_targets_cover_each_chromium_browser() {
        let home = Path::new("/home/player");
        let list = targets(Os::Linux, home);
        assert_eq!(list.len(), 6);
        let folders = [
            ".config/google-chrome",
            ".config/chromium",
            ".config/BraveSoftware/Brave-Browser",
            ".config/microsoft-edge",
            ".config/vivaldi",
        ];
        for (index, folder) in folders.iter().enumerate() {
            assert_eq!(list[index].family, Family::Chromium);
            assert_eq!(
                file_path(&list[index]),
                home.join(folder)
                    .join("NativeMessagingHosts/com.mymtgo.endstep.json")
            );
        }
        assert_eq!(list[5].family, Family::Firefox);
        assert_eq!(
            file_path(&list[5]),
            home.join(".mozilla/native-messaging-hosts/com.mymtgo.endstep.json")
        );
    }

    #[test]
    fn windows_targets_are_registry_keys() {
        let list = targets(Os::Windows, Path::new(r"C:\Users\Player"));
        let places: Vec<(Family, Place)> = list
            .iter()
            .map(|target| (target.family, target.place.clone()))
            .collect();
        assert_eq!(
            places,
            vec![
                (
                    Family::Chromium,
                    Place::Registry(
                        r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.mymtgo.endstep"
                            .to_string()
                    )
                ),
                (
                    Family::Chromium,
                    Place::Registry(
                        r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.mymtgo.endstep"
                            .to_string()
                    )
                ),
                (
                    Family::Firefox,
                    Place::Registry(
                        r"HKCU\Software\Mozilla\NativeMessagingHosts\com.mymtgo.endstep"
                            .to_string()
                    )
                ),
            ]
        );
    }

    #[test]
    fn helper_name_has_exe_only_on_windows() {
        assert_eq!(helper_file_name(Os::Windows), "mymtgo-endstep-host.exe");
        assert_eq!(helper_file_name(Os::MacOs), "mymtgo-endstep-host");
        assert_eq!(helper_file_name(Os::Linux), "mymtgo-endstep-host");
    }
}
