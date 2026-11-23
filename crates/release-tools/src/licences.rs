//! The third-party licence list behind the About page (release spec
//! section 9). `cargo run -p release-tools -- licences` writes
//! `ui/src/content/licences.json`; `--check` fails when that file is stale.
//!
//! Rust crates come from `cargo metadata --filter-platform <target>` for each
//! release target: a walk from the `app` crate over normal and build
//! dependency edges (dev-dependencies are never shipped). UI packages come
//! from `pnpm licenses list --prod --json`, plus the build-time CSS packages
//! (`BUILD_CSS_PACKAGES`) and the fonts kept in the repository
//! (`VENDORED_ASSETS`). Each package's licence files are read from its own
//! folder, and identical texts are stored once.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The targets we release (spec section 9).
pub const TARGETS: [&str; 2] = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"];

/// The generated file, relative to the workspace root.
pub const OUTPUT: &str = "ui/src/content/licences.json";

/// Dev-dependencies whose CSS ships in the bundle through `ui/src/css/app.css`
/// (`@import`), so their notices are owed although `--prod` leaves them out.
pub const BUILD_CSS_PACKAGES: [&str; 2] = ["tailwindcss", "tw-animate-css"];

/// A package whose own files do not carry its whole licence: the licence
/// shown instead of the manifest's, and a notice (from
/// `crates/release-tools/notices/`) added to its texts.
pub struct PackageNotice {
    pub name: &'static str,
    pub licence: &'static str,
    /// The notice text; an em dash is kept as the six characters `\u2014`
    /// so the files stay plain ASCII (`notice_text` restores it).
    pub text: &'static str,
}

/// mana-font ships no licence file and declares only MIT, which covers its
/// CSS; the bundled font itself is under the SIL OFL 1.1 (its README).
/// Bungee and Teko are vendored fonts (`VENDORED_ASSETS`), under the SIL OFL 1.1.
/// mtg-vectors holds the two platform logos, under the MPL 2.0.
/// `@mtgreplay/mtgr` is the vendored .mtgr parser (`ui/src/vendor/mtgr/`), under the MIT licence.
pub const PACKAGE_NOTICES: [PackageNotice; 5] = [
    PackageNotice {
        name: "mana-font",
        licence: "MIT AND OFL-1.1",
        text: concat!(
            include_str!("../notices/mana-font.txt"),
            include_str!("../notices/ofl-1.1.txt")
        ),
    },
    PackageNotice {
        name: "Bungee",
        licence: "OFL-1.1",
        text: concat!(
            include_str!("../notices/bungee.txt"),
            include_str!("../notices/ofl-1.1.txt")
        ),
    },
    PackageNotice {
        name: "Teko",
        licence: "OFL-1.1",
        text: concat!(
            include_str!("../notices/teko.txt"),
            include_str!("../notices/ofl-1.1.txt")
        ),
    },
    PackageNotice {
        name: "mtg-vectors",
        licence: "MPL-2.0",
        text: concat!(
            include_str!("../notices/mtg-vectors.txt"),
            include_str!("../notices/mpl-2.0.txt")
        ),
    },
    PackageNotice {
        name: "@mtgreplay/mtgr",
        licence: "MIT",
        text: include_str!("../../../ui/src/vendor/mtgr/LICENSE"),
    },
];

/// A file kept in the repository (not a package) and bundled into the UI: its
/// name and version. Its licence and text are its `PACKAGE_NOTICES`
/// entry.
pub struct VendoredAsset {
    pub name: &'static str,
    pub version: &'static str,
}

/// `ui/src/fonts/Bungee-Regular.ttf` and `ui/src/fonts/Teko-Variable.ttf`
/// (`ui/src/css/app.css`; the version is the font's own, its name table) and
/// the logos in `ui/src/assets/platforms/` (the version is the commit).
pub const VENDORED_ASSETS: [VendoredAsset; 4] = [
    VendoredAsset {
        name: "Bungee",
        version: "2.000",
    },
    VendoredAsset {
        name: "Teko",
        version: "2.000",
    },
    VendoredAsset {
        name: "mtg-vectors",
        version: "e5fb84f",
    },
    VendoredAsset {
        name: "@mtgreplay/mtgr",
        version: "0.3.0",
    },
];

/// The vendored assets as UI packages. They have no package folder, so their
/// only text is their notice.
pub fn vendored_assets() -> Vec<ShippedPackage> {
    let mut packages = Vec::new();
    for font in VENDORED_ASSETS {
        packages.push(ShippedPackage {
            name: font.name.to_string(),
            version: font.version.to_string(),
            licence: "Unknown".to_string(),
            ecosystem: Ecosystem::Ui,
            folder: PathBuf::new(),
            licence_file: None,
        });
    }
    packages
}

/// The notice for a UI package, if it has one.
fn package_notice(package: &ShippedPackage) -> Option<&'static PackageNotice> {
    if package.ecosystem != Ecosystem::Ui {
        return None;
    }
    PACKAGE_NOTICES
        .iter()
        .find(|notice| notice.name == package.name)
}

/// A notice's text with its em dashes restored.
pub fn notice_text(notice: &PackageNotice) -> String {
    notice.text.replace("\\u2014", "\u{2014}")
}

/// Packages with a notice: the notice's licence replaces the manifest's.
pub fn apply_notice_licences(packages: &mut [ShippedPackage]) {
    for package in packages.iter_mut() {
        if let Some(notice) = package_notice(package) {
            package.licence = notice.licence.to_string();
        }
    }
}

/// A package's licence texts: its own files, then its notice (if any).
pub fn package_texts(package: &ShippedPackage) -> Vec<String> {
    let mut texts = Vec::new();
    // A vendored asset has no folder to read.
    if !package.folder.as_os_str().is_empty() {
        texts = read_licence_files(&package.folder, package.licence_file.as_deref());
    }
    if let Some(notice) = package_notice(package) {
        texts.push(notice_text(notice));
    }
    texts
}

/// Shown when a crate names a licence file but no SPDX expression.
pub const SEE_TEXT: &str = "See the licence text";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ecosystem {
    Rust,
    Ui,
}

impl Ecosystem {
    fn key(self) -> &'static str {
        match self {
            Ecosystem::Rust => "rust",
            Ecosystem::Ui => "ui",
        }
    }
}

/// One third-party package compiled into the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippedPackage {
    pub name: String,
    pub version: String,
    /// The licence expression as the package declares it.
    pub licence: String,
    pub ecosystem: Ecosystem,
    /// The package's own folder, where its licence files are.
    pub folder: PathBuf,
    /// A licence file the manifest names (relative to `folder`).
    pub licence_file: Option<PathBuf>,
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| format!("the JSON has no {key:?} field"))
}

fn string_field(value: &Value, key: &str) -> Result<String, String> {
    field(value, key)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{key:?} is not a string"))
}

fn array_field<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    field(value, key)?
        .as_array()
        .ok_or_else(|| format!("{key:?} is not a list"))
}

/// True when a resolve edge is a normal or build dependency.
fn is_shipped_edge(dep: &Value) -> bool {
    let Some(kinds) = dep.get("dep_kinds").and_then(Value::as_array) else {
        return false;
    };
    kinds.iter().any(|kind| match kind.get("kind") {
        None | Some(Value::Null) => true,
        Some(Value::String(kind)) => kind == "build",
        Some(_) => false,
    })
}

/// The third-party crates reachable from the workspace crate `root` in one
/// `cargo metadata --filter-platform` output, sorted by name and version.
pub fn shipped_crates(metadata_json: &str, root: &str) -> Result<Vec<ShippedPackage>, String> {
    let metadata: Value = serde_json::from_str(metadata_json)
        .map_err(|error| format!("cargo metadata is not JSON: {error}"))?;

    let mut workspace = BTreeSet::new();
    for member in array_field(&metadata, "workspace_members")? {
        if let Some(id) = member.as_str() {
            workspace.insert(id.to_string());
        }
    }

    let mut packages: HashMap<String, &Value> = HashMap::new();
    let mut root_id = None;
    for package in array_field(&metadata, "packages")? {
        let id = string_field(package, "id")?;
        if workspace.contains(&id) && string_field(package, "name")? == root {
            root_id = Some(id.clone());
        }
        packages.insert(id, package);
    }
    let root_id = root_id.ok_or_else(|| format!("no workspace crate is named {root}"))?;

    let resolve = field(&metadata, "resolve")?;
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for node in array_field(resolve, "nodes")? {
        let id = string_field(node, "id")?;
        let mut deps = Vec::new();
        for dep in array_field(node, "deps")? {
            if is_shipped_edge(dep) {
                deps.push(string_field(dep, "pkg")?);
            }
        }
        edges.insert(id, deps);
    }

    // A plain walk: a stack of ids still to visit and a set of ids seen.
    let mut seen = BTreeSet::new();
    let mut stack = vec![root_id];
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(deps) = edges.get(&id) {
            for dep in deps {
                stack.push(dep.clone());
            }
        }
    }

    let mut crates = Vec::new();
    for id in seen {
        if workspace.contains(&id) {
            continue;
        }
        let package = packages
            .get(&id)
            .ok_or_else(|| format!("the resolve names {id}, which is not a package"))?;
        crates.push(crate_package(package)?);
    }
    crates.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(crates)
}

fn crate_package(package: &Value) -> Result<ShippedPackage, String> {
    let manifest = PathBuf::from(string_field(package, "manifest_path")?);
    let folder = manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no folder", manifest.display()))?;
    let licence_file = package
        .get("license_file")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let licence = match package.get("license").and_then(Value::as_str) {
        Some(licence) => licence.to_string(),
        None if licence_file.is_some() => SEE_TEXT.to_string(),
        None => "Unknown".to_string(),
    };
    Ok(ShippedPackage {
        name: string_field(package, "name")?,
        version: string_field(package, "version")?,
        licence,
        ecosystem: Ecosystem::Rust,
        folder,
        licence_file,
    })
}

/// The UI's production packages from `pnpm licenses list --prod --json`
/// (an object of licence name to packages, each with parallel `versions`
/// and `paths` lists).
pub fn ui_packages(pnpm_json: &str) -> Result<Vec<ShippedPackage>, String> {
    let list: Value = serde_json::from_str(pnpm_json)
        .map_err(|error| format!("the pnpm licence list is not JSON: {error}"))?;
    let groups = list
        .as_object()
        .ok_or_else(|| "the pnpm licence list is not an object".to_string())?;
    let mut packages = Vec::new();
    for (group_licence, entries) in groups {
        let entries = entries
            .as_array()
            .ok_or_else(|| format!("the pnpm group {group_licence} is not a list"))?;
        for entry in entries {
            let name = string_field(entry, "name")?;
            let licence = entry
                .get("license")
                .and_then(Value::as_str)
                .unwrap_or(group_licence)
                .to_string();
            let versions = array_field(entry, "versions")?;
            let paths = array_field(entry, "paths")?;
            if versions.len() != paths.len() {
                return Err(format!(
                    "{name} has {} versions and {} paths",
                    versions.len(),
                    paths.len()
                ));
            }
            for (version, path) in versions.iter().zip(paths) {
                let version = version
                    .as_str()
                    .ok_or_else(|| format!("{name} has a version that is not a string"))?;
                let path = path
                    .as_str()
                    .ok_or_else(|| format!("{name} has a path that is not a string"))?;
                packages.push(ShippedPackage {
                    name: name.clone(),
                    version: version.to_string(),
                    licence: licence.clone(),
                    ecosystem: Ecosystem::Ui,
                    folder: PathBuf::from(path),
                    licence_file: None,
                });
            }
        }
    }
    packages.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(packages)
}

/// The build-time CSS packages, each read from `<node_modules>/<name>`
/// (its `package.json` for version and licence). A missing one is an error.
pub fn build_css_packages(node_modules: &Path) -> Result<Vec<ShippedPackage>, String> {
    let mut packages = Vec::new();
    for name in BUILD_CSS_PACKAGES {
        let folder = node_modules.join(name);
        let manifest = folder.join("package.json");
        let text = std::fs::read_to_string(&manifest)
            .map_err(|error| format!("could not read {name}'s package.json: {error}"))?;
        let manifest: Value = serde_json::from_str(&text)
            .map_err(|error| format!("{name}'s package.json is not JSON: {error}"))?;
        let licence = manifest
            .get("license")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string();
        packages.push(ShippedPackage {
            name: name.to_string(),
            version: string_field(&manifest, "version")?,
            licence,
            ecosystem: Ecosystem::Ui,
            folder,
            licence_file: None,
        });
    }
    Ok(packages)
}

/// Every list merged into one: a package in more than one list (both
/// targets) is kept once. Rust crates first, then UI packages, each by name
/// and version.
pub fn merge_packages(lists: Vec<Vec<ShippedPackage>>) -> Vec<ShippedPackage> {
    let mut merged: BTreeMap<(Ecosystem, String, String), ShippedPackage> = BTreeMap::new();
    for list in lists {
        for package in list {
            let key = (
                package.ecosystem,
                package.name.clone(),
                package.version.clone(),
            );
            merged.entry(key).or_insert(package);
        }
    }
    merged.into_values().collect()
}

/// True for a file name that holds a licence or a notice.
fn is_licence_file(name: &str) -> bool {
    let upper = name.to_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "UNLICENSE", "NOTICE"]
        .iter()
        .any(|prefix| upper.starts_with(prefix))
}

/// The licence texts in a package's folder: the manifest's own licence file
/// first, then every `LICENSE*`, `LICENCE*`, `COPYING*`, `UNLICENSE*` and
/// `NOTICE*` file, by name. Text that is not UTF-8 is decoded lossily, so a
/// notice is never dropped; a file that cannot be read at all is left out.
pub fn read_licence_files(folder: &Path, licence_file: Option<&Path>) -> Vec<String> {
    let mut texts = Vec::new();
    let mut named = None;
    if let Some(file) = licence_file {
        let path = folder.join(file);
        if let Some(text) = read_text(&path) {
            texts.push(text);
            named = path.file_name().map(|name| name.to_os_string());
        }
    }
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let is_file = entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false);
            let name = entry.file_name();
            if is_file && is_licence_file(&name.to_string_lossy()) && Some(&name) != named.as_ref()
            {
                names.push(name);
            }
        }
    }
    names.sort();
    for name in names {
        if let Some(text) = read_text(&folder.join(&name)) {
            texts.push(text);
        }
    }
    texts
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Line endings as `\n`, so a text checked out on Windows matches.
fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// The JSON the About page reads: `packages` (name, version, licence,
/// ecosystem and the indexes of its texts) and `texts` (each distinct
/// licence text once). `read` gives a package's licence texts.
pub fn licences_json(
    packages: &[ShippedPackage],
    read: &mut dyn FnMut(&ShippedPackage) -> Vec<String>,
) -> String {
    let mut texts: Vec<String> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut entries = Vec::new();
    for package in packages {
        let mut indexes: Vec<usize> = Vec::new();
        for text in read(package) {
            let text = normalise(&text);
            let index = match index_of.get(&text) {
                Some(index) => *index,
                None => {
                    texts.push(text.clone());
                    index_of.insert(text, texts.len() - 1);
                    texts.len() - 1
                }
            };
            if !indexes.contains(&index) {
                indexes.push(index);
            }
        }
        entries.push(json!({
            "name": package.name,
            "version": package.version,
            "licence": package.licence,
            "ecosystem": package.ecosystem.key(),
            "texts": indexes,
        }));
    }
    let value = json!({ "packages": entries, "texts": texts });
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
    let mut out = ascii_only(&pretty);
    out.push('\n');
    out
}

/// Writes every non-ASCII character as a JSON `\uXXXX` escape (a surrogate
/// pair above U+FFFF). The texts read back unchanged, and the committed file
/// stays plain ASCII. Safe because JSON only has non-ASCII inside strings.
fn ascii_only(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for character in json.chars() {
        if character.is_ascii() {
            out.push(character);
            continue;
        }
        let mut units = [0u16; 2];
        for unit in character.encode_utf16(&mut units) {
            out.push_str(&format!("\\u{unit:04x}"));
        }
    }
    out
}

/// Ok when the file on disk equals a fresh run.
pub fn check_file(expected: &str, current: Option<&str>) -> Result<(), String> {
    match current {
        None => Err(format!(
            "{OUTPUT} is missing: run `cargo run -p release-tools -- licences`"
        )),
        Some(current) if normalise(current) == expected => Ok(()),
        Some(_) => Err(format!(
            "{OUTPUT} is stale: run `cargo run -p release-tools -- licences` and commit it"
        )),
    }
}
