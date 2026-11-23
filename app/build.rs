const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    // `tauri::generate_context!` fails to compile if the frontend folder is
    // missing. `cargo check`, `clippy` and `test` run without a UI build, so make
    // sure the folder exists (an empty one is fine); `tauri build` fills it.
    std::fs::create_dir_all("../ui/dist").expect("create ui/dist");
    bake_api_config();
    // Declaring the commands makes Tauri check each one against the calling
    // window's capability (`capabilities/*.json`), so a window can call only
    // the commands it is granted (#389).
    let manifest = tauri_build::AppManifest::new().commands(app_commands());
    let mut attributes = tauri_build::Attributes::new().app_manifest(manifest);
    if windows_msvc() {
        // The manifest below replaces tauri's default one (two would clash).
        let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        attributes = attributes.windows_attributes(windows);
    }
    tauri_build::try_build(attributes).expect("tauri build script failed");
    if windows_msvc() {
        embed_manifest();
    }
}

/// The app's command names, read from the `collect_commands!` list in
/// `src/bindings.rs` so a new command is listed once. Each becomes an
/// `allow-<name>` permission (underscores as hyphens).
fn app_commands() -> &'static [&'static str] {
    println!("cargo:rerun-if-changed=src/bindings.rs");
    let text = std::fs::read_to_string("src/bindings.rs").expect("read src/bindings.rs");
    let start = text
        .find("collect_commands![")
        .expect("src/bindings.rs has a collect_commands! list");
    let list = &text[start + "collect_commands![".len()..];
    let end = list.find(']').expect("the collect_commands! list ends");
    let mut names: Vec<&'static str> = Vec::new();
    for path in list[..end].split(',') {
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let name = path.rsplit("::").next().unwrap_or(path).to_string();
        // tauri-build wants `'static` names; a build script runs once, so
        // leaking these few strings is fine.
        names.push(Box::leak(name.into_boxed_str()));
    }
    Box::leak(names.into_boxed_slice())
}

/// Bake the API configuration (07 spec section 3): each key from the build
/// environment, else from the git-ignored `.env.local` at the repo root. The
/// app reads them as `MYMTGO_BAKED_*`; process variables at app start win.
/// The CA path is baked into debug builds only, so a release build made on a
/// developer's machine never trusts a local CA alone.
fn bake_api_config() {
    use mymtgo_api::config::{API_URL_KEY, CA_KEY, CLIENT_ID_KEY};
    let env_file = std::path::Path::new("../.env.local");
    // Cargo reruns a build script on every build while a watched file is
    // missing, which would rebuild the app each time; so the file is watched
    // only once it exists (touch build.rs after creating it).
    if env_file.exists() {
        println!("cargo:rerun-if-changed=../.env.local");
    }
    let from_file = std::fs::read_to_string(env_file)
        .map(|text| mymtgo_api::parse_env_file(&text))
        .unwrap_or_default();
    let debug = std::env::var("PROFILE").is_ok_and(|profile| profile == "debug");
    let keys = [
        (API_URL_KEY, "MYMTGO_BAKED_API_URL", true),
        (CLIENT_ID_KEY, "MYMTGO_BAKED_OAUTH_CLIENT_ID", true),
        (CA_KEY, "MYMTGO_BAKED_API_CA", debug),
    ];
    for (key, baked_name, bake) in keys {
        println!("cargo:rerun-if-env-changed={key}");
        if !bake {
            continue;
        }
        let from_env = std::env::var(key).ok().filter(|value| !value.is_empty());
        let value = from_env.or_else(|| {
            from_file
                .iter()
                .find(|(name, value)| name == key && !value.is_empty())
                .map(|(_, value)| value.clone())
        });
        if let Some(value) = value {
            println!("cargo:rustc-env={baked_name}={value}");
        }
    }
}

fn windows_msvc() -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc")
}

/// On Windows the dialog plugin needs Common Controls v6. The app binary gets
/// it from a manifest, but the unit-test executable had none and failed to
/// start with STATUS_ENTRYPOINT_NOT_FOUND. Cargo has no link argument for
/// library unit tests alone, so one manifest is embedded in every executable.
fn embed_manifest() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let path = std::path::Path::new(&out_dir).join("app.manifest");
    std::fs::write(&path, MANIFEST).expect("write the manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", path.display());
}
