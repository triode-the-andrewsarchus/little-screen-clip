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
    assert!(notices.contains("2026 urbanisierung"));
    assert!(notices.contains("2011-2026 Aliaksei Chapyzhenka"));
    assert!(notices.contains("Rquickjs Contributors"));
    assert!(notices.contains("Excalidraw Render License Bundle"));
    assert!(notices.contains("29392d10d9c504ec22d67f37a4dd545a63d5b371"));
    assert!(notices.contains("D2 and TALA"));
    assert!(notices.contains("2020 Daniel Kallin"));
    assert!(notices.contains("2022 Apisit Ritreungroj"));
    assert!(notices.contains("2016 Bryce Lampe"));
    assert!(notices.contains("2015 Taco de Wolff"));
    assert!(notices.contains("The Go Authors"));
    assert!(notices.contains("Bytefield-SVG 1.11.0"));
    assert!(notices.contains("Erd compatibility grammar"));
    assert!(notices.contains("d18075d205583458b477cdcb7fe8bb41ec092cf9"));
    assert!(notices.contains("Eclipse Public License - v 1.0"));
    assert!(notices.contains("811552e58e93d1106cb634ddeec1f68db4e6371f9edc56cf1787b82202478e80"));
    assert!(notices.contains("2012-2014 Chris Pettitt"));
    assert!(notices.contains("Fuzzy Bubbles Project Authors"));
    assert!(notices.contains("Matheus Mota"));
    assert!(notices.contains("STIX Fonts Project Authors"));
    assert!(notices.contains("234fc35f116bceca2c34579879a6e6893112ccf8"));
    assert!(notices.contains("e0e75bcab2fa8e39dd7a09465b7f2e035eda82ef"));
}

#[cfg(unix)]
#[test]
fn relocated_application_uses_its_bundled_goat_companion() {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};
    let executable = Path::new(env!("CARGO_BIN_EXE_clip-file-manager"));
    let profile = executable.parent().unwrap();
    // A hard link keeps a debug application fixture from copying gigabytes.
    let deployment = tempfile::tempdir_in(profile).unwrap();
    let application = deployment.path().join("viewer");
    fs::hard_link(executable, &application).unwrap();
    let helpers: Vec<_> = fs::read_dir(profile)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("clip-goat-")
                && entry.path().is_file()
        })
        .map(|entry| entry.path())
        .collect();
    assert!(
        !helpers.is_empty(),
        "Cargo must publish the compiled companion"
    );
    let run = || {
        let output = CommandRequest::new(&application)
            .arg("--internal-diagram-worker")
            .stdin_bytes(
                serde_json::to_vec(
                    &serde_json::json!({"format":"goat","source":".----.\n| hi |\n'----'"}),
                )
                .unwrap(),
            )
            .timeout(Duration::from_secs(10))
            .max_output_bytes(8 * 1024 * 1024)
            .own_process_group()
            .die_with_parent()
            .run()
            .unwrap();
        assert!(output.status.success());
        assert!(!output.stdout_truncated);
        serde_json::from_slice::<Result<little_clip_render::diagram::Rendered, String>>(
            &output.stdout,
        )
        .unwrap()
    };
    // A marker proves sibling discovery won over the development-path fallback.
    for helper in &helpers {
        let path = deployment.path().join(helper.file_name().unwrap());
        fs::write(
            &path,
            "#!/bin/sh\nprintf 'owned GoAT deployment marker' >&2\nexit 7\n",
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(run().unwrap_err().contains("owned GoAT deployment marker"));
    for helper in &helpers {
        fs::copy(helper, deployment.path().join(helper.file_name().unwrap())).unwrap();
    }
    let rendered = run().unwrap();
    assert!(rendered.svg.contains("<path"));
    assert!(rendered.png.len() > 500);
}
