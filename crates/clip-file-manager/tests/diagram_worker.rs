//! Verify native rendering dispatches before any host UI/runtime setup.
use little_clip_core::CommandRequest;
use std::time::Duration;

#[test]
fn diagram_worker_renders_and_reports_errors_without_host_startup() {
    little_clip_render::diagram::test_support::assert_worker(env!(
        "CARGO_BIN_EXE_clip-file-manager"
    ));
}

#[test]
fn diagram_license_notices_are_available_without_host_startup() {
    let output = CommandRequest::new(env!("CARGO_BIN_EXE_clip-file-manager"))
        .arg("--diagram-licenses")
        .timeout(Duration::from_secs(5))
        .max_output_bytes(512 * 1024)
        .own_process_group()
        .die_with_parent()
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr_lossy());
    assert!(!output.stdout_truncated);
    let notices = String::from_utf8(output.stdout).unwrap();
    assert!(notices.contains("Eclipse Public License - v 2.0"));
    assert!(notices.contains("Permission is hereby granted"));
    assert!(notices.contains("Abbas Monfared"));
    assert!(notices.contains("SIL OPEN FONT LICENSE"));
    assert!(notices.contains("CORE-MATH"));
    assert!(notices.contains("D2 and TALA"));
    assert!(notices.contains("Fuzzy Bubbles Project Authors"));
    assert!(notices.contains("Matheus Mota"));
    assert!(notices.contains("STIX Fonts Project Authors"));
    assert!(notices.contains("234fc35f116bceca2c34579879a6e6893112ccf8"));
    assert!(notices.contains("e0e75bcab2fa8e39dd7a09465b7f2e035eda82ef"));
}
