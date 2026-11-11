//! Exercise the real executable and descriptor protocol, including exec-time inheritance.
use std::{
    fs::File,
    io::Write,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::Command,
};

fn command(input: &File, mode: &str, formats: &str) -> Command {
    let descriptor = input.as_raw_fd();
    let mut command = Command::new(env!("CARGO_BIN_EXE_clip-media-worker"));
    command.args([mode, formats, &format!("/proc/self/fd/{descriptor}")]);
    // SAFETY: only the child descriptor table changes. fcntl is async-signal-safe,
    // and the parent test retains the live File through command completion.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(descriptor, libc::F_SETFD, 0) < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command
}

#[test]
fn executable_probes_and_decodes_a_retained_descriptor_from_eof() {
    let mut input = tempfile::tempfile().unwrap();
    input
        .write_all(include_bytes!("fixtures/silence.aac"))
        .unwrap();
    let probe = command(&input, "probe", "aac").output().unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(value["streams"][0]["codec_name"], "aac");
    let decoded = command(&input, "decode-audio", "aac").output().unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert!(!decoded.stdout.is_empty());
    assert_eq!(decoded.stdout.len() % 8, 0);
    assert!(
        decoded
            .stdout
            .chunks_exact(4)
            .all(|sample| f32::from_le_bytes(sample.try_into().unwrap()).is_finite())
    );
}

#[test]
fn executable_denies_secondary_files_even_when_they_exist() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("audio.aac"),
        include_bytes!("fixtures/silence.aac"),
    )
    .unwrap();
    let mut playlist = tempfile::tempfile().unwrap();
    playlist
        .write_all(b"ffconcat version 1.0\nfile audio.aac\n")
        .unwrap();
    let result = command(&playlist, "probe", "concat")
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("whitelist"));
}
