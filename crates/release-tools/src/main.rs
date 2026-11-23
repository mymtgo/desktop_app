//! `release-tools`: write `latest.json` for a release (flags only, run by
//! release.yml), `release-tools licences [--check]` to write or check the
//! third-party licence list (release spec section 9),
//! `release-tools prune --keep N [--promoted VERSION ...]` to print, one per
//! line, a channel folder's version folders to delete (folder names on
//! stdin), `release-tools channel VERSION` to print the bucket folder a
//! version goes to (`edge` or `stable`), or `release-tools newer VERSION
//! CURRENT` to print `true` when VERSION is newer than CURRENT, else `false`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use release_tools::licences::{self, ShippedPackage};
use release_tools::{PlatformFile, latest_json, parse_args};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("licences") {
        return run_licences(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("prune") {
        return run_prune(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("channel") {
        return run_channel(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("newer") {
        return run_newer(&args[1..]);
    }
    let args = parse_args(&args)?;
    let notes = std::fs::read_to_string(&args.notes_file)
        .map_err(|error| format!("could not read {}: {error}", args.notes_file.display()))?;
    let mut files = Vec::new();
    for (platform, path) in &args.platforms {
        let sig_path = format!("{}.sig", path.display());
        let signature = std::fs::read_to_string(&sig_path)
            .map_err(|error| format!("could not read {sig_path}: {error}"))?;
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{} has no file name", path.display()))?;
        if !path.is_file() {
            return Err(format!("{} does not exist", path.display()));
        }
        files.push(PlatformFile {
            platform: platform.clone(),
            file_name,
            signature,
        });
    }
    let feed = latest_json(
        &args.version,
        &notes,
        &args.pub_date,
        &args.base_url,
        &files,
    )?;
    std::fs::write(&args.out, feed)
        .map_err(|error| format!("could not write {}: {error}", args.out.display()))
}

/// `channel`: the bucket folder for a version (`release_tools::channel_folder`).
fn run_channel(args: &[String]) -> Result<(), String> {
    match args {
        [version] => {
            println!("{}", release_tools::channel_folder(version)?);
            Ok(())
        }
        _ => Err("usage: release-tools channel VERSION".to_string()),
    }
}

/// `newer`: `true` when the first version is newer than the second
/// (`release_tools::prune::newer`). Fails when either is not a version.
fn run_newer(args: &[String]) -> Result<(), String> {
    let [candidate, current] = args else {
        return Err("usage: release-tools newer VERSION CURRENT".to_string());
    };
    match release_tools::prune::newer(candidate, current) {
        Some(newer) => {
            println!("{newer}");
            Ok(())
        }
        None => Err(format!("{candidate} or {current} is not a version")),
    }
}

/// `prune`: reads version folder names from stdin (one per line) and prints
/// the ones to delete (`release_tools::prune::to_delete`).
fn run_prune(args: &[String]) -> Result<(), String> {
    let usage = "usage: release-tools prune --keep N [--promoted VERSION ...]";
    let mut keep: Option<usize> = None;
    let mut promoted: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let value = args.get(index + 1).ok_or(usage)?;
        match args[index].as_str() {
            "--keep" => keep = Some(value.parse().map_err(|_| usage)?),
            "--promoted" => {
                if !value.is_empty() {
                    promoted.push(value.clone());
                }
            }
            _ => return Err(usage.to_string()),
        }
        index += 2;
    }
    let keep = keep.filter(|keep| *keep > 0).ok_or(usage)?;
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .map_err(|error| format!("could not read stdin: {error}"))?;
    let versions: Vec<String> = input
        .lines()
        .map(|line| line.trim().trim_end_matches('/').to_string())
        .filter(|line| !line.is_empty())
        .collect();
    for name in release_tools::prune::to_delete(&versions, &promoted, keep) {
        println!("{name}");
    }
    Ok(())
}

/// `licences`: walks `cargo metadata` for each release target and the UI's
/// production packages, then writes `ui/src/content/licences.json`, or with
/// `--check` fails when the committed file differs. Reads only local files
/// (cargo itself may fetch a crate source that is not on disk yet).
fn run_licences(args: &[String]) -> Result<(), String> {
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: release-tools licences [--check]".to_string()),
    };
    let root = workspace_root();
    let mut lists = Vec::new();
    for target in licences::TARGETS {
        // Default features, as `app` is built for a release.
        let metadata = run_tool(
            &cargo(),
            &[
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--filter-platform",
                target,
                "--manifest-path",
                &root.join("Cargo.toml").to_string_lossy(),
            ],
            &root,
        )?;
        lists.push(licences::shipped_crates(&metadata, "app")?);
    }
    let ui = root.join("ui");
    let pnpm_list = run_tool(
        pnpm(),
        &[
            "--dir",
            &ui.to_string_lossy(),
            "licenses",
            "list",
            "--prod",
            "--json",
        ],
        &root,
    )?;
    lists.push(licences::ui_packages(&pnpm_list)?);
    lists.push(licences::build_css_packages(&ui.join("node_modules"))?);
    lists.push(licences::vendored_assets());
    let mut packages = licences::merge_packages(lists);
    licences::apply_notice_licences(&mut packages);

    let mut read = |package: &ShippedPackage| licences::package_texts(package);
    let fresh = licences::licences_json(&packages, &mut read);
    let output = root.join(licences::OUTPUT);
    if check {
        let current = std::fs::read_to_string(&output).ok();
        licences::check_file(&fresh, current.as_deref())?;
        println!(
            "{} is up to date ({} packages)",
            licences::OUTPUT,
            packages.len()
        );
        return Ok(());
    }
    if let Some(folder) = output.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("could not create {}: {error}", folder.display()))?;
    }
    std::fs::write(&output, fresh)
        .map_err(|error| format!("could not write {}: {error}", output.display()))?;
    println!("wrote {} ({} packages)", licences::OUTPUT, packages.len());
    Ok(())
}

/// The workspace root: this crate sits at `crates/release-tools`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The cargo that runs us (`cargo run` sets `CARGO`), else the one on PATH.
fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// pnpm is a `.cmd` shim on Windows, which `Command` does not find by name.
fn pnpm() -> &'static str {
    if cfg!(windows) { "pnpm.cmd" } else { "pnpm" }
}

fn run_tool(program: &str, args: &[&str], folder: &Path) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(folder)
        .output()
        .map_err(|error| format!("could not run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{program} printed non-UTF-8: {error}"))
}
