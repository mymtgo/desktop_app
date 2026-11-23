use fixture_tools::TestDir;

#[test]
fn a_ui_error_reaches_the_log_with_its_source_route_and_stack() {
    let dir = TestDir::new("app-ui-errors");
    let logs = dir.join("logs");
    let guard = app::logging::init_with_filter(&logs, "info").expect("init logging");
    app::reporting::ui_error(&app::reporting::UiError {
        source: "vue: render function".to_string(),
        message: "Cannot read properties of undefined".to_string(),
        stack: Some("at MatchesTable.vue:12".to_string()),
        route: Some("/settings".to_string()),
    });
    drop(guard);

    let mut text = String::new();
    for entry in std::fs::read_dir(&logs).expect("log folder") {
        text.push_str(&std::fs::read_to_string(entry.expect("entry").path()).expect("read"));
    }
    assert!(text.contains("mymtgo::ui"), "{text}");
    assert!(
        text.contains("Cannot read properties of undefined"),
        "{text}"
    );
    assert!(text.contains("vue: render function"), "{text}");
    assert!(text.contains("/settings"), "{text}");
    assert!(text.contains("MatchesTable.vue:12"), "{text}");
}
