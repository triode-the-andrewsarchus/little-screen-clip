//! Exercise internal worker dispatch through the actual file-manager executable.
use std::process::Command;

#[test]
fn studio_workers_dispatch_before_browser_startup_from_another_directory() {
    let mut blank_rgba = Vec::from(32_u32.to_le_bytes());
    blank_rgba.extend_from_slice(&32_u32.to_le_bytes());
    blank_rgba.extend_from_slice(&[255; 32 * 32 * 4]);
    for (worker, input_bytes, expected) in [
        (
            "--internal-conversion-decode-worker",
            b"invalid image".as_slice(),
            [0].as_slice(),
        ),
        ("--internal-barcode-worker", blank_rgba.as_slice(), b"[]"),
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
        assert_eq!(std::fs::read(output).unwrap(), expected, "{worker}");
    }
}
