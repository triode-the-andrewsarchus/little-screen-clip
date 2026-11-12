//! Exercise internal worker dispatch through the actual file-manager executable.
use std::process::Command;

#[test]
fn studio_workers_dispatch_before_browser_startup_from_another_directory() {
    let mut blank_rgba = Vec::from(32_u32.to_le_bytes());
    blank_rgba.extend_from_slice(&32_u32.to_le_bytes());
    blank_rgba.extend_from_slice(&[255; 32 * 32 * 4]);
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(32, 32, image::Rgba([255; 4]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    for (worker, input_bytes, expected) in [
        (
            "--internal-conversion-decode-worker",
            b"invalid image".as_slice(),
            [0].as_slice(),
        ),
        ("--internal-barcode-worker", blank_rgba.as_slice(), b"[]"),
        (
            "--internal-png-optimize-worker",
            png.as_slice(),
            png.as_slice(),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input");
        let output = directory.path().join("output");
        std::fs::write(&input, input_bytes).unwrap();
        let status = Command::new(env!("CARGO_BIN_EXE_clip-file-manager"))
            .current_dir(directory.path())
            .arg(worker)
            .arg(input)
            .arg(&output)
            .status()
            .unwrap();
        assert!(status.success(), "{worker}: {status}");
        let actual = std::fs::read(output).unwrap();
        if worker == "--internal-png-optimize-worker" {
            assert!(actual.len() <= expected.len());
            assert_eq!(
                image::load_from_memory(&actual).unwrap().to_rgba8(),
                image::load_from_memory(expected).unwrap().to_rgba8()
            );
        } else {
            assert_eq!(actual, expected, "{worker}");
        }
    }
}

#[test]
fn browser_workers_reject_bad_arguments_before_opening_a_window() {
    for (flag, extra_argument, expected_error) in [
        (
            "--internal-file-search-reader",
            true,
            "Invalid internal worker arguments",
        ),
        (
            "--internal-file-extraction-plan",
            true,
            "Invalid internal worker arguments",
        ),
        (
            "--internal-system-console-snapshot",
            false,
            "Invalid internal worker arguments",
        ),
        (
            "--internal-system-processes",
            false,
            "Invalid internal worker arguments",
        ),
        (
            "--internal-system-inspector",
            false,
            "Invalid internal worker arguments",
        ),
        (
            "--internal-fuse-binwalk-daemon",
            false,
            "Usage: --internal-fuse-binwalk-daemon",
        ),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clip-file-manager"));
        command.arg(flag);
        if extra_argument {
            command.arg("unexpected");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{flag}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected_error),
            "{flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn extracted_store_restores_through_the_application_reader_with_inherited_handles() {
    use clip_search::store::TestFixture;
    use little_clip_core::{config::FileSearchLimits, process::CommandRequest};
    use std::{fs::File, sync::atomic::AtomicBool, time::Duration};

    fn reader(
        handles: [&File; 3],
        max_bytes: u64,
        max_vectors: u64,
        page_size: u32,
        cancel: &AtomicBool,
    ) -> Result<u64, String> {
        let mut request = CommandRequest::new(env!("CARGO_BIN_EXE_clip-file-manager"));
        let mut fds = [0; 3];
        for (index, file) in handles.into_iter().enumerate() {
            let (next, fd) = request.inherit_handle(file).map_err(|e| e.to_string())?;
            request = next;
            fds[index] = fd;
        }
        let reply = request
            .arg("--internal-file-search-reader")
            .stdin_bytes(
                serde_json::to_vec(&serde_json::json!({"Restore": {
                    "fds": fds, "max_bytes": max_bytes,
                    "max_vectors": max_vectors, "page_size": page_size,
                }}))
                .unwrap(),
            )
            .own_process_group()
            .die_with_parent()
            .timeout(Duration::from_secs(30))
            .run_cancellable(cancel)
            .map_err(|e| e.to_string())?;
        if !reply.status.success() || reply.stdout_truncated {
            return Err(reply.stderr_lossy());
        }
        let json: serde_json::Value =
            serde_json::from_slice(&reply.stdout).map_err(|e| e.to_string())?;
        json["Ok"]["Restored"]
            .as_u64()
            .ok_or_else(|| json.to_string())
    }

    let mut fixture = TestFixture::new();
    let output = tempfile::tempdir().unwrap();
    let backup = output.path().join("backup.sqlite");
    let cancel = AtomicBool::new(false);
    fixture.store.export(&backup, &cancel).unwrap();
    let backup_bytes = std::fs::read(&backup).unwrap();
    fixture
        .store
        .connection
        .execute("UPDATE segments SET text='changed after backup'", [])
        .unwrap();
    let limits = FileSearchLimits {
        max_mib: 16,
        max_vectors: 10,
    };
    let prepared = fixture
        .store
        .prepare_restore(&backup, &limits, &cancel, reader)
        .unwrap();
    fixture
        .store
        .apply_restore(prepared, &[fixture.root.path.clone()], &cancel)
        .unwrap();
    let text: String = fixture
        .store
        .connection
        .query_row("SELECT text FROM segments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(text, "original content");
    let vectors: i64 = fixture
        .store
        .connection
        .query_row(
            "SELECT count(*) FROM segments WHERE length(vector)=1536",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(vectors, 1);
    assert_eq!(std::fs::read(backup).unwrap(), backup_bytes);
}

#[test]
fn extraction_worker_round_trips_data_only_plans_through_the_application_entrypoint() {
    use little_clip_core::process::CommandRequest;
    use serde_json::{Value, json};
    let call = |request: Value| {
        let output = CommandRequest::new(env!("CARGO_BIN_EXE_clip-file-manager"))
            .arg("--internal-file-extraction-plan")
            .stdin_bytes(serde_json::to_vec(&request).unwrap())
            .die_with_parent()
            .own_process_group()
            .max_output_bytes(64 * 1024)
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        assert!(!output.stdout_truncated);
        serde_json::from_slice::<Result<Value, String>>(&output.stdout)
            .unwrap()
            .unwrap()
    };
    let source = "---\ninvoice:\n  total: 42\n---\nAn owned invoice fixture.";
    let observed = call(json!({"operation":"inspect", "source":source}));
    assert_eq!(observed["kind"], "header");
    assert_eq!(observed["header"]["format"], "YAML");
    let plan = json!({"operation":"frontmatter_field", "schema":observed["header"]["schema"], "path":["invoice", "total"]});
    let extracted = call(json!({"operation":"execute", "source":source, "plan":plan}));
    assert_eq!(extracted["kind"], "field");
    assert_eq!(extracted["value"]["value"], "42");
    assert_eq!(extracted["value"]["path"], json!(["invoice", "total"]));
}
