//! Verify native rendering dispatches before host UI/runtime startup.
#[test]
fn diagram_worker_renders_and_reports_errors_without_host_startup() {
    little_clip_render::diagram::test_support::assert_worker(env!("CARGO_BIN_EXE_clip-workbench"));
}
