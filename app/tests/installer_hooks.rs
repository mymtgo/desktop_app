//! The Windows installer hooks (`app/installer/hooks.nsh`, release spec
//! section 13 item 5 and section 13.2) are NSIS script: no test here can run
//! them, so these read the script and pin the parts that keep 0.x users
//! safe. The owner runs them on a Windows VM (Plan W, W16 step 6).

use std::path::PathBuf;

const QUESTION: &str = "MyMTGO 0.x is installed. Remove it now? Its data stays on this computer, so you can import it into MyMTGO.";
/// The migration question (release spec section 13.2), as NSIS text: `$\r$\n`
/// is a line break.
const MIGRATION_QUESTION: &str = r"MyMTGO 0.x data was found on this computer.$\r$\n$\r$\nYes: migrate from 0.x. MyMTGO imports your matches, decks and settings when it first opens.$\r$\nNo: start fresh. You can import later from the Import page.";
/// The importer's packaged 0.x candidates (release spec section 13 item 1).
const LEGACY_DATABASES: [&str; 2] = [
    r"$APPDATA\mymtgo\database\database.sqlite",
    r"$APPDATA\com.mymtgo.app\database\database.sqlite",
];
const V1_DATABASE: &str = r"$APPDATA\com.mymtgo.desktop\mymtgo.sqlite";
const CHOICE_FILE: &str = r"$APPDATA\com.mymtgo.desktop\first-run-choice.txt";

fn app_file(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn hooks() -> String {
    app_file("installer/hooks.nsh")
}

/// The body of one `!macro NAME` ... `!macroend` block.
fn macro_body(script: &str, name: &str) -> String {
    let start_marker = format!("!macro {name}\n");
    let start = script
        .find(&start_marker)
        .unwrap_or_else(|| panic!("no {name} macro"));
    let body = &script[start + start_marker.len()..];
    let end = body.find("!macroend").expect("macro end");
    body[..end].to_string()
}

/// Script lines without `;` comments, so a word in a comment never passes a
/// check meant for code.
fn code_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with(';') && !line.starts_with('#'))
        .map(|line| line.to_string())
        .collect()
}

fn position(lines: &[String], what: &str) -> usize {
    lines
        .iter()
        .position(|line| line.contains(what))
        .unwrap_or_else(|| panic!("no line with {what}:\n{}", lines.join("\n")))
}

#[test]
fn the_config_names_the_hooks_file() {
    let config: serde_json::Value =
        serde_json::from_str(&app_file("tauri.conf.json")).expect("tauri.conf.json");
    assert_eq!(
        config["bundle"]["windows"]["nsis"]["installerHooks"],
        "installer/hooks.nsh"
    );
}

#[test]
fn the_hook_asks_before_removing_0x() {
    let lines = code_lines(&macro_body(&hooks(), "NSIS_HOOK_PREINSTALL"));
    // 0.x's entry: an electron-builder key under HKCU's Uninstall list whose
    // DisplayName is exactly "mymtgo". `S==` is case sensitive, so v1's own
    // "MyMTGO" entry never matches.
    let enumerate = position(
        &lines,
        r#"EnumRegKey $Mymtgo0xKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall""#,
    );
    let name = position(&lines, r#"$Mymtgo0xName S== "mymtgo""#);
    let question = position(&lines, QUESTION);
    assert!(enumerate < name && name < question);
    // The path comes from the registry: it must be set, name a file that
    // exists, and be electron-builder's `Uninstall mymtgo.exe` (LogicLib `==`
    // ignores case), all checked before the question.
    let file_name = position(&lines, "${GetFileName} $Mymtgo0xUninstaller $Mymtgo0xName");
    let not_empty = position(&lines, r#"${If} $Mymtgo0xUninstaller != """#);
    assert_eq!(
        lines[not_empty + 1],
        "${AndIf} ${FileExists} $Mymtgo0xUninstaller"
    );
    assert_eq!(
        lines[not_empty + 2],
        r#"${AndIf} $Mymtgo0xName == "Uninstall mymtgo.exe""#
    );
    assert!(name < file_name && file_name < not_empty && not_empty < question);
    assert!(
        lines[question].contains(&format!("\"{QUESTION}\"")),
        "{}",
        lines[question]
    );
    // Silent, passive and update installs never ask: all three are checked
    // before the question.
    let silent = position(&lines, "${Silent}");
    let passive = position(&lines, "$PassiveMode");
    let update = position(&lines, "$UpdateMode");
    assert!(silent < question && passive < question && update < question);
    assert!(
        lines[silent].contains("${IfNot} ${Silent}"),
        "{}",
        lines[silent]
    );
    assert!(lines[passive].contains("$PassiveMode != 1"));
    assert!(lines[update].contains("$UpdateMode != 1"));
    // A silent run that reached it anyway answers No.
    assert!(lines[question].contains("/SD IDNO"), "{}", lines[question]);
}

#[test]
fn the_0x_removal_runs_only_after_a_yes() {
    let script = hooks();
    let lines = code_lines(&macro_body(&script, "NSIS_HOOK_PREINSTALL"));
    let question = position(&lines, QUESTION);
    assert!(
        lines[question].ends_with("IDYES mymtgo_0x_remove IDNO mymtgo_0x_keep"),
        "{}",
        lines[question]
    );
    let yes = position(&lines, "mymtgo_0x_remove:");
    let no = position(&lines, "mymtgo_0x_keep:");
    let runs: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("ExecWait") || line.starts_with("Exec "))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(runs.len(), 1, "one program run: 0.x's uninstaller");
    let run = runs[0];
    assert!(question < yes && yes < run && run < no);
    // Silent, waited for (`_?=` keeps the uninstaller from copying itself to
    // TEMP and returning at once), and never with the delete-data option.
    let call = &lines[run];
    assert!(
        call.starts_with(r#"ExecWait '"$Mymtgo0xUninstaller" /currentuser /S _?=$Mymtgo0xFolder'"#),
        "{call}"
    );
    // Its own leftover file and folder go only after it succeeded.
    let checked = position(&lines, "${If} $Mymtgo0xResult == 0");
    assert_eq!(lines[checked + 1], "${AndIfNot} ${Errors}");
    let delete = position(&lines, "Delete $Mymtgo0xUninstaller");
    let folder = position(&lines, "RMDir $Mymtgo0xFolder");
    assert!(run < checked && checked < delete && delete < folder && folder < no);
}

#[test]
fn the_hooks_never_delete_application_data() {
    let lines = code_lines(&hooks());
    for line in &lines {
        assert!(!line.contains("--delete-app-data"), "{line}");
        assert!(!line.contains("RMDir /r"), "{line}");
    }
    let deletes: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("Delete ") || line.starts_with("RMDir "))
        .collect();
    assert_eq!(
        deletes,
        vec![
            "Delete $Mymtgo0xUninstaller",
            "RMDir $Mymtgo0xFolder",
            "Delete $MymtgoBetaUninstaller",
            "RMDir $MymtgoBetaFolder",
            r#"Delete "$SMPROGRAMS\MyMTGO Beta.lnk""#,
            r#"Delete "$DESKTOP\MyMTGO Beta.lnk""#,
            r#"Delete "$INSTDIR\$MymtgoBetaExe""#,
        ],
        "only the leftover uninstallers of 0.x and the beta, their folders if empty, and the beta's own shortcuts"
    );
}

// Release spec section 13.2: the installer asks "migrate or start fresh" and
// records the answer for v1's first start.

#[test]
fn the_migration_question_is_the_spec_text() {
    let lines = code_lines(&macro_body(&hooks(), "NSIS_HOOK_PREINSTALL"));
    let question = position(&lines, "MyMTGO 0.x data was found");
    assert!(
        lines[question].starts_with(&format!(
            "MessageBox MB_YESNO|MB_ICONQUESTION \"{MIGRATION_QUESTION}\""
        )),
        "{}",
        lines[question]
    );
    // Yes migrates, No starts fresh; a silent run that reached it answers No.
    assert!(
        lines[question].ends_with("/SD IDNO IDYES mymtgo_choice_migrate IDNO mymtgo_choice_fresh"),
        "{}",
        lines[question]
    );
    let migrate = position(&lines, "mymtgo_choice_migrate:");
    let fresh = position(&lines, "mymtgo_choice_fresh:");
    assert_eq!(
        lines[migrate + 1],
        r#"StrCpy $MymtgoFirstRunChoice "migrate""#
    );
    assert_eq!(lines[fresh + 1], r#"StrCpy $MymtgoFirstRunChoice "fresh""#);
}

#[test]
fn the_migration_question_skips_silent_passive_and_update_installs() {
    let lines = code_lines(&macro_body(&hooks(), "NSIS_HOOK_PREINSTALL"));
    let question = position(&lines, MIGRATION_QUESTION);
    let silent = position(&lines, "${Silent}");
    let passive = position(&lines, "$PassiveMode");
    let update = position(&lines, "$UpdateMode");
    assert!(silent < question && passive < question && update < question);
    assert_eq!(lines[silent], "${IfNot} ${Silent}");
    assert_eq!(lines[passive], "${AndIf} $PassiveMode != 1");
    assert_eq!(lines[update], "${AndIf} $UpdateMode != 1");
    // Nothing is chosen before the guard, so a skipped question writes no
    // file.
    let cleared = position(&lines, r#"StrCpy $MymtgoFirstRunChoice """#);
    assert!(cleared < silent, "the choice starts empty");
}

#[test]
fn the_question_needs_0x_data_and_no_v1_database() {
    let lines = code_lines(&macro_body(&hooks(), "NSIS_HOOK_PREINSTALL"));
    let question = position(&lines, MIGRATION_QUESTION);
    let v1 = position(&lines, &format!("IfFileExists \"{V1_DATABASE}\""));
    assert!(v1 < question);
    assert_eq!(lines[v1 + 1], "StrCpy $MymtgoV1Data 1");
    for database in LEGACY_DATABASES {
        let found = position(&lines, &format!("IfFileExists \"{database}\""));
        assert!(found < question, "{database}");
        assert_eq!(lines[found + 1], "StrCpy $Mymtgo0xData 1");
    }
    let asked = position(&lines, "${If} $Mymtgo0xData == 1");
    assert!(v1 < asked && asked < question);
    assert_eq!(lines[asked + 1], "${AndIf} $MymtgoV1Data == 0");
}

#[test]
fn the_choice_is_written_only_after_install() {
    let script = hooks();
    let pre = code_lines(&macro_body(&script, "NSIS_HOOK_PREINSTALL"));
    for line in &pre {
        assert!(
            !line.contains("FileOpen") && !line.contains("FileWrite"),
            "{line}"
        );
        assert!(!line.contains("first-run-choice.txt"), "{line}");
    }
    let post = code_lines(&macro_body(&script, "NSIS_HOOK_POSTINSTALL"));
    let asked = position(&post, r#"${If} $MymtgoFirstRunChoice != """#);
    let folder = position(&post, r#"CreateDirectory "$APPDATA\com.mymtgo.desktop""#);
    let open = position(
        &post,
        &format!("FileOpen $MymtgoChoiceHandle \"{CHOICE_FILE}\" w"),
    );
    let write = position(&post, "FileWrite $MymtgoChoiceHandle $MymtgoFirstRunChoice");
    let close = position(&post, "FileClose $MymtgoChoiceHandle");
    assert!(asked < folder && folder < open && open < write && write < close);
    // The one word and nothing else: no line break for v1 to trim.
    let writes: Vec<&String> = post
        .iter()
        .filter(|line| line.starts_with("FileWrite"))
        .collect();
    assert_eq!(writes.len(), 1);
}

#[test]
fn the_installer_never_touches_the_0x_database() {
    for line in code_lines(&hooks()) {
        let names_0x = LEGACY_DATABASES
            .iter()
            .any(|database| line.contains(database))
            || line.contains(r"\mymtgo\database")
            || line.contains(r"\com.mymtgo.app");
        if names_0x {
            assert!(line.starts_with("IfFileExists "), "{line}");
        }
    }
}

#[test]
fn the_migration_question_comes_before_the_removal_offer() {
    let lines = code_lines(&macro_body(&hooks(), "NSIS_HOOK_PREINSTALL"));
    let migration = position(&lines, MIGRATION_QUESTION);
    let removal = position(&lines, QUESTION);
    let enumerate = position(&lines, "EnumRegKey");
    assert!(migration < enumerate && enumerate < removal);
}

// Release spec section 13.1: leaving the "MyMTGO Beta" name. The post-install
// hook removes the old install and moves its launch at login, compiled in
// only when the product name is no longer the beta name.

const BETA_NAME: &str = "MyMTGO Beta";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const BETA_UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MyMTGO Beta";
const INERT_GUARD: &str = r#"!if "${PRODUCTNAME}" != "MyMTGO Beta""#;

/// The lines from `first` to the `!endif` that closes it.
fn guarded_block(lines: &[String], first: usize) -> Vec<String> {
    let mut depth = 0;
    let mut block = Vec::new();
    for line in &lines[first..] {
        if line.starts_with("!if") {
            depth += 1;
        }
        block.push(line.clone());
        if line.starts_with("!endif") {
            depth -= 1;
            if depth == 0 {
                return block;
            }
        }
    }
    panic!("no !endif for line {first}");
}

#[test]
fn the_hooks_never_pass_the_delete_data_option() {
    // Tauri 2.12's uninstaller deletes %APPDATA%\com.mymtgo.desktop only when
    // its confirm page's "Delete the application data" box was ticked
    // ($DeleteAppDataCheckboxState, set in un.ConfirmLeave); a silent run
    // skips every page, and no command line option sets it. The hooks never
    // touch that state, never pass 0.x's --delete-app-data, and never delete
    // a folder recursively.
    for line in code_lines(&hooks()) {
        let lower = line.to_lowercase();
        assert!(!lower.contains("delete-app-data"), "{line}");
        assert!(!lower.contains("deleteappdata"), "{line}");
        assert!(!lower.contains("rmdir /r"), "{line}");
        assert!(
            !lower.contains("$appdata\\com.mymtgo.desktop\" /r"),
            "{line}"
        );
        if line.starts_with("ExecWait") {
            // Every program the hooks run is an uninstaller run silently,
            // in place, and never as an update.
            assert!(line.contains(" /S _?="), "{line}");
            assert!(!line.contains("/UPDATE"), "{line}");
        }
    }
}

#[test]
fn the_post_install_hook_is_inert_under_the_beta_name() {
    let script = hooks();
    let post = code_lines(&macro_body(&script, "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    // Everything that names the beta install sits inside the guard.
    for (index, line) in post.iter().enumerate() {
        if line.contains(BETA_NAME) && index != guard {
            assert!(
                index > guard && index < guard + block.len(),
                "outside the guard: {line}"
            );
        }
    }
    // The pre-install hook (and so the migration question) never names it.
    for line in code_lines(&macro_body(&script, "NSIS_HOOK_PREINSTALL")) {
        assert!(!line.contains(BETA_NAME), "{line}");
    }
    // Tauri 2.12 includes this file before it defines PRODUCTNAME, so a
    // top level `!if` on it would compare the literal text and always hold.
    // The guard therefore lives inside the macro (expanded in Section
    // Install, after the define), and the rename's variables are declared
    // there with `Var /GLOBAL`: the last beta declares none of them.
    let mut outside_macros = Vec::new();
    let mut in_macro = false;
    for line in code_lines(&script) {
        if line.starts_with("!macro ") {
            in_macro = true;
        } else if line.starts_with("!macroend") {
            in_macro = false;
        } else if !in_macro {
            outside_macros.push(line);
        }
    }
    for line in &outside_macros {
        assert!(!line.contains("${PRODUCTNAME}"), "{line}");
        assert!(!line.contains("MymtgoBeta"), "{line}");
    }
    let declarations: Vec<&String> = post
        .iter()
        .filter(|line| line.starts_with("Var ") && line.contains("MymtgoBeta"))
        .collect();
    assert_eq!(declarations.len(), 18, "{declarations:?}");
    for declaration in declarations {
        assert!(
            declaration.starts_with("Var /GLOBAL MymtgoBeta"),
            "{declaration}"
        );
        let at = position(&post, declaration);
        assert!(at > guard && at < guard + block.len(), "{declaration}");
    }
}

#[test]
fn the_post_install_hook_removes_the_beta_install_silently() {
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    let read = position(
        &block,
        &format!(
            r#"ReadRegStr $MymtgoBetaUninstaller HKCU "{BETA_UNINSTALL_KEY}" "UninstallString""#
        ),
    );
    let file_name = position(
        &block,
        "${GetFileName} $MymtgoBetaUninstaller $MymtgoBetaQuote",
    );
    let not_empty = position(&block, r#"${If} $MymtgoBetaUninstaller != """#);
    let exists = position(&block, "${AndIf} ${FileExists} $MymtgoBetaUninstaller");
    assert_eq!(exists, not_empty + 1);
    // Tauri's uninstaller is `uninstall.exe`; a registry value naming any
    // other file is never run or deleted.
    let named = position(&block, r#"${AndIf} $MymtgoBetaQuote == "uninstall.exe""#);
    assert!(read < file_name && file_name < not_empty);
    let not_self = position(&block, "${If} $MymtgoBetaSameFolder == 0");
    let runs: Vec<usize> = block
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("ExecWait") || line.starts_with("Exec "))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(runs.len(), 1, "one program run: the beta's uninstaller");
    let run = runs[0];
    assert_eq!(
        block[run],
        r#"ExecWait '"$MymtgoBetaUninstaller" /S _?=$MymtgoBetaFolder' $MymtgoBetaResult"#
    );
    assert!(read < exists && exists < named && named < not_self && not_self < run);
    // Its leftover file, and its folder only if empty, after it succeeded.
    let checked = position(&block, "${If} $MymtgoBetaResult == 0");
    // A program that could not run sets the error flag, not a result.
    assert_eq!(block[checked + 1], "${AndIfNot} ${Errors}");
    let delete = position(&block, "Delete $MymtgoBetaUninstaller");
    let folder = position(&block, "RMDir $MymtgoBetaFolder");
    assert!(run < checked && checked < delete && delete < folder);
}

#[test]
fn the_post_install_hook_moves_the_launch_at_login_value() {
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    // Read before the old uninstaller runs: it deletes its own Run value.
    let read = position(
        &block,
        &format!(r#"ReadRegStr $MymtgoBetaAutostartValue HKCU "{RUN_KEY}" "{BETA_NAME}""#),
    );
    let run = position(&block, "ExecWait");
    let only_if = position(&block, "${If} $MymtgoBetaAutostart == 1");
    let delete = position(
        &block,
        &format!(r#"DeleteRegValue HKCU "{RUN_KEY}" "{BETA_NAME}""#),
    );
    // The autostart plugin's format (auto-launch 0.6 on Windows): the
    // program path, a space, then its arguments (none), under the product
    // name.
    let write = position(
        &block,
        &format!(
            r#"WriteRegStr HKCU "{RUN_KEY}" "${{PRODUCTNAME}}" "$INSTDIR\${{MAINBINARYNAME}}.exe ""#
        ),
    );
    assert!(read < run && run < only_if && only_if < delete && delete < write);
}

// From 1.0.0 the licence page no longer calls MyMTGO beta software (owner,
// 2026-10-19, replacing ruling 25's beta wording).
#[test]
fn the_licence_page_says_neither_beta_nor_alpha() {
    let terms = app_file("installer/terms.txt");
    let flowing = terms.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(!terms.to_lowercase().contains("alpha"));
    assert!(!flowing.contains("beta software"), "{flowing}");
    assert!(!flowing.contains("in beta"), "{flowing}");
    assert!(
        flowing.contains("MyMTGO is provided \"as is\", without warranty of any kind"),
        "{flowing}"
    );
    assert!(
        flowing.contains("The statistics MyMTGO shows can be incomplete or wrong."),
        "{flowing}"
    );
}

#[test]
fn the_post_install_hook_replaces_the_beta_shortcuts() {
    // The beta's updater runs this installer with /UPDATE, which makes no
    // shortcuts, and the beta's uninstaller removes the beta's: so each
    // shortcut the beta had is made under the new name. Always made, as a
    // first install makes it: 0.x's shortcut is mymtgo.lnk, the same file
    // to Windows (names ignore case), and must not stop it.
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    let run = position(&block, "ExecWait");
    for (seen_variable, old, new) in [
        (
            "$MymtgoBetaStartMenu",
            r"$SMPROGRAMS\MyMTGO Beta.lnk",
            r"$SMPROGRAMS\${PRODUCTNAME}.lnk",
        ),
        (
            "$MymtgoBetaDesktop",
            r"$DESKTOP\MyMTGO Beta.lnk",
            r"$DESKTOP\${PRODUCTNAME}.lnk",
        ),
    ] {
        let seen = position(&block, &format!(r#"IfFileExists "{old}" 0 +2"#));
        assert_eq!(block[seen + 1], format!("StrCpy {seen_variable} 1"));
        // The last such test: the same-folder branch checks the variable too.
        let only_if = block
            .iter()
            .rposition(|line| line == &format!("${{If}} {seen_variable} == 1"))
            .expect("shortcut test");
        assert!(
            !block
                .iter()
                .any(|line| line.contains(&format!(r#"${{FileExists}} "{new}""#))),
            "{new} is made even when a file of that name is there"
        );
        // A non-update install already made (or, by the user's choice, did
        // not make) the shortcuts; /NS asks for none.
        assert_eq!(block[only_if + 1], "${AndIf} $UpdateMode = 1");
        assert_eq!(block[only_if + 2], "${AndIf} $NoShortcutMode <> 1");
        let create = position(
            &block,
            &format!(r#"CreateShortcut "{new}" "$INSTDIR\${{MAINBINARYNAME}}.exe""#),
        );
        assert_eq!(
            block[create + 1],
            format!(r#"!insertmacro SetLnkAppUserModelId "{new}""#)
        );
        assert!(seen < run && run < only_if && only_if < create, "{old}");
    }
}

/// Plan W review I1: the installer's 0.x checks are exactly the importer's
/// packaged candidates under `%APPDATA%`, in the importer's order, so a
/// "migrate" answer never points at a file the importer cannot find.
#[test]
fn the_installer_and_the_importer_agree_on_the_0x_database_path() {
    use importer::locate::{Os, candidate_paths};
    let appdata = PathBuf::from("APPDATA");
    let importer: Vec<String> = candidate_paths(None, Some(&appdata), Os::Windows)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == "database.sqlite")
        })
        .map(|path| {
            let parts: Vec<String> = path
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("${}", parts.join(r"\"))
        })
        .collect();
    assert_eq!(importer, LEGACY_DATABASES.to_vec());

    let installer: Vec<String> = code_lines(&macro_body(&hooks(), "NSIS_HOOK_PREINSTALL"))
        .windows(2)
        .filter(|pair| pair[1] == "StrCpy $Mymtgo0xData 1")
        .map(|pair| {
            pair[0]
                .strip_prefix("IfFileExists \"")
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| panic!("not a file check: {}", pair[0]))
                .to_string()
        })
        .collect();
    assert_eq!(installer, importer);
}

const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

#[test]
fn the_beta_install_in_this_folder_loses_its_uninstall_entry_and_shortcuts() {
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    // The beta's executable name is read from its Uninstall key before the
    // beta's uninstaller (which deletes the key) can run, with a fallback.
    let read_exe = position(
        &block,
        &format!(r#"ReadRegStr $MymtgoBetaExe HKCU "{BETA_UNINSTALL_KEY}" "MainBinaryName""#),
    );
    let run = position(&block, "ExecWait");
    assert!(read_exe < run);
    assert_eq!(block[read_exe + 1], r#"${If} $MymtgoBetaExe == """#);
    assert_eq!(
        block[read_exe + 2],
        r#"StrCpy $MymtgoBetaExe "${MAINBINARYNAME}.exe""#
    );
    // A name with a separator or wildcard falls back to the new name.
    for bad in [r#""\""#, r#""*""#, r#""?""#] {
        let line = block
            .iter()
            .position(|line| line.contains(&format!("$MymtgoBetaChar == {bad}")))
            .unwrap_or_else(|| panic!("no check for {bad}"));
        assert!(read_exe < line && line < run);
    }
    let same = position(&block, "${If} $MymtgoBetaSameFolder == 0");
    let other = position(&block, "${ElseIf} $MymtgoBetaSameFolder == 1");
    assert!(same < run && run < other);
    let key = position(
        &block,
        &format!(r#"DeleteRegKey HKCU "{BETA_UNINSTALL_KEY}""#),
    );
    assert!(other < key);
    // Each link goes only when it targets the beta's executable here.
    for (seen, link) in [
        ("$MymtgoBetaStartMenu", r"$SMPROGRAMS\MyMTGO Beta.lnk"),
        ("$MymtgoBetaDesktop", r"$DESKTOP\MyMTGO Beta.lnk"),
    ] {
        let seen_at = position(&block[key..], &format!("${{If}} {seen} == 1")) + key;
        let target = position(
            &block[key..],
            &format!(r#"!insertmacro IsShortcutTarget "{link}" "$INSTDIR\$MymtgoBetaExe""#),
        ) + key;
        assert_eq!(block[target + 1], "Pop $MymtgoBetaExit");
        assert_eq!(block[target + 2], "${If} $MymtgoBetaExit = 1");
        let delete = position(&block[key..], &format!(r#"Delete "{link}""#)) + key;
        assert!(seen_at < target && target < delete);
    }
    // The old executable: non-empty, differing from this build's, and a
    // plain name (checked where it is read).
    let exe_if = position(&block, r#"${If} $MymtgoBetaExe != """#);
    assert_eq!(
        block[exe_if + 1],
        r#"${AndIf} $MymtgoBetaExe != "${MAINBINARYNAME}.exe""#
    );
    assert_eq!(block[exe_if + 2], r#"Delete "$INSTDIR\$MymtgoBetaExe""#);
    assert!(other < exe_if);
}

#[test]
fn the_beta_folder_is_compared_as_short_paths_and_unknown_leaves_the_beta_alone() {
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    let assume = position(&block, "StrCpy $MymtgoBetaSameFolder 1");
    let a = position(
        &block,
        "GetFullPathName /SHORT $MymtgoBetaShort $MymtgoBetaFolder",
    );
    let b = position(
        &block,
        "GetFullPathName /SHORT $MymtgoBetaShortInstall $INSTDIR",
    );
    // Either side empty: unknown (2); otherwise different (0) when they differ.
    let cmp = position(&block, r#"${If} $MymtgoBetaShort == """#);
    assert_eq!(block[cmp + 1], r#"${OrIf} $MymtgoBetaShortInstall == """#);
    assert_eq!(block[cmp + 2], "StrCpy $MymtgoBetaSameFolder 2");
    assert_eq!(
        block[cmp + 3],
        "${ElseIf} $MymtgoBetaShort != $MymtgoBetaShortInstall"
    );
    assert_eq!(block[cmp + 4], "StrCpy $MymtgoBetaSameFolder 0");
    // The uninstaller runs only for 0, the cleanup only for 1, nothing for 2.
    let different = position(&block, "${If} $MymtgoBetaSameFolder == 0");
    let run = position(&block, "ExecWait");
    let same = position(&block, "${ElseIf} $MymtgoBetaSameFolder == 1");
    let key = position(
        &block,
        &format!(r#"DeleteRegKey HKCU "{BETA_UNINSTALL_KEY}""#),
    );
    assert!(assume < a && a < b && b < cmp && cmp < different);
    assert!(different < run && run < same && same < key);
    assert!(!block.iter().any(|line| line == "${Else}"));
}

#[test]
fn the_beta_exe_name_is_a_plain_exe_that_is_not_the_uninstaller() {
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    let read = position(&block, "ReadRegStr $MymtgoBetaExe");
    for bad in ["\\", "*", "?", "/", ":"] {
        let line = position(&block, &format!(r#"$MymtgoBetaChar == "{bad}""#));
        assert!(read < line, "{bad}");
    }
    let tail = position(&block, "StrCpy $MymtgoBetaChar $MymtgoBetaExe 4 -4");
    assert_eq!(block[tail + 1], r#"${If} $MymtgoBetaChar != ".exe""#);
    assert_eq!(
        block[tail + 2],
        r#"${OrIf} $MymtgoBetaExe == "uninstall.exe""#
    );
    assert_eq!(
        block[tail + 3],
        r#"StrCpy $MymtgoBetaExe "${MAINBINARYNAME}.exe""#
    );
    assert!(read < tail && tail < position(&block, "ExecWait"));
}

#[test]
fn a_beta_switched_off_in_task_manager_stays_off() {
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    // Read first (before the beta's uninstaller runs), through reg.exe.
    let query = position(&block, "nsExec::ExecToStack");
    assert!(block[query].contains(APPROVED_KEY), "{}", block[query]);
    assert!(block[query].contains(r#"/v "MyMTGO Beta""#));
    let run = position(&block, "ExecWait");
    assert!(query < run);
    // Task Manager's off state is the low bit of the first byte.
    let low_bit = position(&block, r#"IntOp $MymtgoBetaChar "0x$MymtgoBetaChar" & 1"#);
    let off = position(&block, "StrCpy $MymtgoBetaApproved 0");
    assert!(query < low_bit && low_bit < off && off < run);
    // The new Run value is written only when the beta was on.
    let only_if = position(&block, "${If} $MymtgoBetaAutostart == 1");
    let approved = position(&block, "${If} $MymtgoBetaApproved == 1");
    let write = position(&block, "WriteRegStr HKCU");
    assert!(only_if < approved && approved < write);
    assert!(block[write].contains(RUN_KEY), "{}", block[write]);
}

#[test]
fn the_beta_leftovers_are_removed() {
    let post = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTINSTALL"));
    let guard = position(&post, INERT_GUARD);
    let block = guarded_block(&post, guard);
    let write = position(&block, "WriteRegStr HKCU");
    let approved = position(
        &block,
        &format!(r#"DeleteRegValue HKCU "{APPROVED_KEY}" "{BETA_NAME}""#),
    );
    let folder_key = position(
        &block,
        r#"DeleteRegKey HKCU "Software\${MANUFACTURER}\MyMTGO Beta""#,
    );
    assert!(write < approved && approved < folder_key);
}

#[test]
fn the_uninstaller_removes_only_the_endstep_registry_keys() {
    // Endstep support: app/src/endstep/host.rs writes these three keys on
    // every start; a full uninstall removes exactly them.
    let lines = code_lines(&macro_body(&hooks(), "NSIS_HOOK_POSTUNINSTALL"));
    let deletes: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("Delete"))
        .collect();
    assert_eq!(
        deletes,
        vec![
            r#"DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.mymtgo.endstep""#,
            r#"DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.mymtgo.endstep""#,
            r#"DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.mymtgo.endstep""#,
        ]
    );
    assert_eq!(lines[0], "${If} $UpdateMode <> 1");
}
