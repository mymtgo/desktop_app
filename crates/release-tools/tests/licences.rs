//! The third-party licence list (release spec section 9), over small
//! `cargo metadata` and `pnpm licenses` fixtures.

use std::path::{Path, PathBuf};

use release_tools::licences::{
    BUILD_CSS_PACKAGES, Ecosystem, ShippedPackage, apply_notice_licences, build_css_packages,
    check_file, licences_json, merge_packages, package_texts, read_licence_files, shipped_crates,
    ui_packages, vendored_assets,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/licences")
        .join(name)
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

fn names(packages: &[ShippedPackage]) -> Vec<String> {
    packages
        .iter()
        .map(|package| format!("{}@{}", package.name, package.version))
        .collect()
}

fn package(name: &str, version: &str, ecosystem: Ecosystem) -> ShippedPackage {
    ShippedPackage {
        name: name.to_string(),
        version: version.to_string(),
        licence: "MIT".to_string(),
        ecosystem,
        folder: PathBuf::from(format!("/registry/{name}-{version}")),
        licence_file: None,
    }
}

#[test]
fn the_graph_walk_keeps_only_shipped_packages() {
    let crates = shipped_crates(&read_fixture("metadata-linux.json"), "app").unwrap();
    // Workspace crates are ours; windows-sys is listed but only reached on
    // the other target, so the filtered resolve has no edge to it.
    assert_eq!(
        names(&crates),
        vec![
            "cc@1.2.0",
            "libc@0.2.170",
            "odd-licence@0.1.0",
            "serde@1.0.200"
        ]
    );

    let crates = shipped_crates(&read_fixture("metadata-windows.json"), "app").unwrap();
    // insta is a dev-dependency (and similar only its dependency);
    // release-only is reached from a workspace crate the app does not use.
    assert_eq!(
        names(&crates),
        vec![
            "cc@1.2.0",
            "odd-licence@0.1.0",
            "serde@1.0.200",
            "windows-sys@0.59.0"
        ]
    );
    let serde = crates.iter().find(|c| c.name == "serde").unwrap();
    assert_eq!(serde.licence, "MIT OR Apache-2.0");
    assert_eq!(serde.ecosystem, Ecosystem::Rust);
    assert_eq!(serde.folder, PathBuf::from("/registry/serde-1.0.200"));
    let odd = crates.iter().find(|c| c.name == "odd-licence").unwrap();
    assert_eq!(odd.licence, "See the licence text");
    assert_eq!(odd.licence_file, Some(PathBuf::from("COPYRIGHT.txt")));
}

#[test]
fn a_missing_root_is_an_error() {
    let error = shipped_crates(&read_fixture("metadata-linux.json"), "nope").unwrap_err();
    assert!(error.contains("nope"), "{error}");
}

#[test]
fn both_targets_are_merged() {
    let windows = shipped_crates(&read_fixture("metadata-windows.json"), "app").unwrap();
    let linux = shipped_crates(&read_fixture("metadata-linux.json"), "app").unwrap();
    let ui = ui_packages(&read_fixture("pnpm.json")).unwrap();
    let merged = merge_packages(vec![windows, linux, ui]);
    assert_eq!(
        names(&merged),
        vec![
            "cc@1.2.0",
            "libc@0.2.170",
            "odd-licence@0.1.0",
            "serde@1.0.200",
            "windows-sys@0.59.0",
            "@scope/pair@1.0.0",
            "@scope/pair@2.0.0",
            "tiny@0.1.0",
            "vue@3.5.43",
        ]
    );
}

#[test]
fn ui_packages_come_from_the_pnpm_list() {
    let ui = ui_packages(&read_fixture("pnpm.json")).unwrap();
    let pair = ui
        .iter()
        .find(|p| p.name == "@scope/pair" && p.version == "2.0.0")
        .unwrap();
    assert_eq!(pair.ecosystem, Ecosystem::Ui);
    assert_eq!(pair.licence, "MIT");
    assert_eq!(
        pair.folder,
        PathBuf::from("/ui/node_modules/.pnpm/@scope+pair@2.0.0/node_modules/@scope/pair")
    );
    let tiny = ui.iter().find(|p| p.name == "tiny").unwrap();
    assert_eq!(tiny.licence, "ISC");
}

#[test]
fn licence_files_are_read_from_the_package_folder() {
    let folder = fixture("crate-folder");
    let texts = read_licence_files(&folder, None);
    // Sorted by file name; the README is not a licence file.
    assert_eq!(
        texts,
        vec![
            "Apache text\n".to_string(),
            "MIT text\n".to_string(),
            "notice text\n".to_string()
        ]
    );
    let texts = read_licence_files(&folder, Some(Path::new("COPYRIGHT.txt")));
    assert_eq!(texts[0], "own copyright\n");
    assert_eq!(texts.len(), 4);
}

#[test]
fn identical_licence_texts_are_stored_once() {
    let packages = vec![
        package("alpha", "1.0.0", Ecosystem::Rust),
        package("beta", "2.0.0", Ecosystem::Rust),
        package("gamma", "3.0.0", Ecosystem::Ui),
        package("delta", "4.0.0", Ecosystem::Ui),
    ];
    let mut read = |package: &ShippedPackage| -> Vec<String> {
        match package.name.as_str() {
            "alpha" => vec!["MIT text\n".to_string(), "Apache text\n".to_string()],
            // Same text with Windows line endings is the same text.
            "beta" => vec!["MIT text\r\n".to_string()],
            "gamma" => vec!["Apache text\n".to_string()],
            _ => Vec::new(),
        }
    };
    let json = licences_json(&packages, &mut read);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value["texts"],
        serde_json::json!(["MIT text\n", "Apache text\n"])
    );
    let entries = value["packages"].as_array().unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries[0],
        serde_json::json!({
            "name": "alpha", "version": "1.0.0", "licence": "MIT",
            "ecosystem": "rust", "texts": [0, 1]
        })
    );
    assert_eq!(entries[1]["texts"], serde_json::json!([0]));
    assert_eq!(entries[2]["ecosystem"], "ui");
    assert_eq!(entries[2]["texts"], serde_json::json!([1]));
    assert_eq!(entries[3]["texts"], serde_json::json!([]));
    // No folder or path from the machine that generated it.
    assert!(!json.contains("/registry/"), "{json}");
    assert!(json.ends_with("}\n"));
}

#[test]
fn check_fails_on_a_stale_file() {
    let fresh = "{\"packages\":[]}\n";
    assert_eq!(check_file(fresh, Some(fresh)), Ok(()));
    let error = check_file(fresh, Some("{\"packages\":[1]}\n")).unwrap_err();
    assert!(error.contains("stale"), "{error}");
    assert!(
        error.contains("cargo run -p release-tools -- licences"),
        "{error}"
    );
    let error = check_file(fresh, None).unwrap_err();
    assert!(error.contains("missing"), "{error}");
}

#[test]
fn the_file_is_plain_ascii_and_keeps_every_character() {
    let packages = vec![package("alpha", "1.0.0", Ecosystem::Rust)];
    let mut read = |_: &ShippedPackage| vec!["a \u{2014} b \u{e9} \u{1f600}\n".to_string()];
    let json = licences_json(&packages, &mut read);
    assert!(json.is_ascii(), "{json}");
    assert!(json.contains("\\u2014"), "{json}");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["texts"][0], "a \u{2014} b \u{e9} \u{1f600}\n");
}

#[test]
fn build_time_css_packages_are_listed_as_ui_packages() {
    // Dev-dependencies whose CSS `app.css` bundles: `--prod` leaves them out.
    assert_eq!(BUILD_CSS_PACKAGES, ["tailwindcss", "tw-animate-css"]);
    let node_modules = fixture("css-packages");
    let css = build_css_packages(&node_modules).unwrap();
    assert_eq!(
        names(&css),
        vec!["tailwindcss@4.3.3", "tw-animate-css@1.4.0"]
    );
    assert!(
        css.iter()
            .all(|p| p.ecosystem == Ecosystem::Ui && p.licence == "MIT")
    );
    assert_eq!(css[0].folder, node_modules.join("tailwindcss"));
    assert_eq!(
        read_licence_files(&css[0].folder, None),
        vec!["tailwind MIT text\n".to_string()]
    );

    let ui = ui_packages(&read_fixture("pnpm.json")).unwrap();
    let merged = merge_packages(vec![ui, css]);
    let merged = names(&merged);
    assert!(
        merged.contains(&"tailwindcss@4.3.3".to_string()),
        "{merged:?}"
    );
    assert!(
        merged.contains(&"tw-animate-css@1.4.0".to_string()),
        "{merged:?}"
    );
}

#[test]
fn a_missing_build_time_css_package_is_an_error() {
    let error = build_css_packages(&fixture("crate-folder")).unwrap_err();
    assert!(error.contains("tailwindcss"), "{error}");
}

#[test]
fn the_committed_list_has_the_build_time_css_packages() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/content/licences.json");
    let list: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for name in BUILD_CSS_PACKAGES {
        let entry = list["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name && p["ecosystem"] == "ui")
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert!(!entry["texts"].as_array().unwrap().is_empty(), "{name}");
    }
}

#[test]
fn a_licence_file_that_is_not_utf8_is_kept() {
    let folder = fixture("latin1-folder");
    assert_eq!(
        read_licence_files(&folder, None),
        vec!["Copyright \u{fffd} Qauthor\n".to_string()]
    );
}

#[test]
fn mana_font_lists_its_font_licence() {
    // mana-font ships no licence file and declares only MIT (its CSS); the
    // font is under the SIL OFL 1.1.
    let mut packages = vec![
        package("mana-font", "1.18.0", Ecosystem::Ui),
        package("mana-font", "1.0.0", Ecosystem::Rust),
    ];
    apply_notice_licences(&mut packages);
    assert_eq!(packages[0].licence, "MIT AND OFL-1.1");
    assert_eq!(packages[1].licence, "MIT");

    let texts = package_texts(&packages[0]);
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("SIL OPEN FONT LICENSE"));
    assert!(texts[0].contains("MIT License"));
    assert!(texts[0].contains("Mana font by Andrew Gioia"));
    assert!(texts[0].contains("in part or in whole \u{2014} any"));
    assert!(!texts[0].contains("\\u2014"));
    assert!(package_texts(&packages[1]).is_empty());
}

/// The committed list's text for a UI package, checking its licence.
fn committed_text(name: &str, licence: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/content/licences.json");
    let list: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let entry = list["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name && p["ecosystem"] == "ui")
        .unwrap_or_else(|| panic!("{name} is not listed"));
    assert_eq!(entry["licence"], licence);
    let text = entry["texts"][0].as_u64().unwrap() as usize;
    list["texts"][text].as_str().unwrap().to_string()
}

#[test]
fn the_committed_list_has_mana_font_and_its_font_licence() {
    let text = committed_text("mana-font", "MIT AND OFL-1.1");
    assert!(text.contains("SIL OPEN FONT LICENSE"));
    assert!(text.contains("Mana font by Andrew Gioia"));
}

#[test]
fn the_vendored_bungee_font_is_listed_with_its_copyright() {
    let mut fonts = vendored_assets();
    assert_eq!(
        names(&fonts),
        vec![
            "Bungee@2.000",
            "Teko@2.000",
            "mtg-vectors@e5fb84f",
            "@mtgreplay/mtgr@0.3.0"
        ]
    );
    assert_eq!(fonts[0].ecosystem, Ecosystem::Ui);
    apply_notice_licences(&mut fonts);
    assert_eq!(fonts[0].licence, "OFL-1.1");
    let texts = package_texts(&fonts[0]);
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("Copyright 2023 The Bungee Project Authors"));
    assert!(texts[0].contains("SIL OPEN FONT LICENSE"));

    let text = committed_text("Bungee", "OFL-1.1");
    assert!(text.contains("Copyright 2023 The Bungee Project Authors"));
}

#[test]
fn the_vendored_teko_font_is_listed_with_its_copyright() {
    let mut fonts = vendored_assets();
    assert_eq!(
        names(&fonts),
        vec![
            "Bungee@2.000",
            "Teko@2.000",
            "mtg-vectors@e5fb84f",
            "@mtgreplay/mtgr@0.3.0"
        ]
    );
    assert_eq!(fonts[1].ecosystem, Ecosystem::Ui);
    apply_notice_licences(&mut fonts);
    assert_eq!(fonts[1].licence, "OFL-1.1");
    let texts = package_texts(&fonts[1]);
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("Copyright 2023 The Teko Project Authors"));
    assert!(texts[0].contains("SIL OPEN FONT LICENSE"));

    let text = committed_text("Teko", "OFL-1.1");
    assert!(text.contains("Copyright 2023 The Teko Project Authors"));
}

#[test]
fn vendored_assets_include_mtg_vectors() {
    let mut assets = vendored_assets();
    assert_eq!(names(&assets)[2], "mtg-vectors@e5fb84f");
    apply_notice_licences(&mut assets);
    assert_eq!(assets[2].licence, "MPL-2.0");
    let texts = package_texts(&assets[2]);
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("Mozilla Public License Version 2.0"));
    assert!(texts[0].contains("github.com/Investigamer/mtg-vectors"));

    let text = committed_text("mtg-vectors", "MPL-2.0");
    assert!(text.contains("Mozilla Public License Version 2.0"));
}

#[test]
fn vendored_assets_include_the_mtgr_parser() {
    let mut assets = vendored_assets();
    assert_eq!(names(&assets)[3], "@mtgreplay/mtgr@0.3.0");
    apply_notice_licences(&mut assets);
    assert_eq!(assets[3].licence, "MIT");
    let texts = package_texts(&assets[3]);
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("MIT License"));

    let text = committed_text("@mtgreplay/mtgr", "MIT");
    assert!(text.contains("MIT License"));
}

#[test]
fn every_vendored_asset_file_exists() {
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/fonts");
    assert!(fonts.join("Bungee-Regular.ttf").is_file());
    assert!(fonts.join("Teko-Variable.ttf").is_file());
    let logos = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/assets/platforms");
    assert!(logos.join("mtgo.svg").is_file());
    assert!(logos.join("arena.svg").is_file());
}
