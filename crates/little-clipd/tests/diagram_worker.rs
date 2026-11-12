//! Verify native rendering dispatches before daemon or hardware startup.
#[test]
fn diagram_worker_renders_and_reports_errors_without_host_startup() {
    for executable in [
        env!("CARGO_BIN_EXE_little-clipd"),
        env!("CARGO_BIN_EXE_test_screen"),
    ] {
        little_clip_render::diagram::test_support::assert_worker(executable);
    }
}
