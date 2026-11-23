use fixture_tools::TestDir;

#[test]
fn log_lines_and_command_timings_reach_the_log_folder() {
    let dir = TestDir::new("app-logging");
    let logs = dir.join("logs");
    let guard =
        app::logging::init_with_filter(&logs, "info,mymtgo::timing=debug").expect("init logging");
    tracing::info!("hello from the logging test");
    drop(app::logging::Timer::start("list_matches"));
    drop(guard); // writes out the buffered lines

    let mut text = String::new();
    for entry in std::fs::read_dir(&logs).expect("log folder") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("mymtgo.") && name.ends_with(".log"),
            "{name}"
        );
        text.push_str(&std::fs::read_to_string(&path).expect("read log"));
    }
    assert!(text.contains("hello from the logging test"), "{text}");
    assert!(text.contains("list_matches"), "{text}");
    assert!(text.contains("elapsed_us"), "{text}");
}
