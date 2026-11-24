//! Explicit, bounded simulation of a small, self-contained SPICE dialect.

mod deck;

use super::SpiceRawDocument;
use crate::{
    eda::check_cancelled,
    file_source::{FileSource, ReadCheck, read_held_snapshot},
};
use std::sync::atomic::AtomicBool;

const MAX_DECK: usize = 16 * 1024;

/// Simulates a guarded local deck. Only the admitted, privately held bytes reach the solver.
pub fn simulate(source: &FileSource, cancel: &AtomicBool) -> Result<SpiceRawDocument, String> {
    let (mut file, _) = ReadCheck::default().open(source.path(), Some(source), cancel)?;
    let (bytes, snapshot) = read_held_snapshot(&mut file, MAX_DECK)
        .map_err(|error| format!("Could not pin the simulation deck: {error}"))?;
    if &snapshot != source {
        return Err("The deck changed. Reopen it before running simulation.".into());
    }
    let analyses = deck::admit(&bytes)?;
    snapshot.check()?;
    check_cancelled(cancel, "Simulation cancelled")?;
    let raw = platform::run(bytes, cancel)?;
    let raw = SpiceRawDocument::parse(&raw, cancel)
        .map_err(|error| format!("Solver RAW output: {error}"))?;
    analyses.verify(&raw)?;
    Ok(raw)
}

/// Probes the actual confinement and runtime without loading a deck or invoking the solver.
pub fn check_support(cancel: &AtomicBool) -> Result<(), String> {
    platform::probe(cancel)
}

#[cfg(target_os = "linux")]
mod platform {
    use crate::process::{CommandOutput, CommandRequest};
    use rustix::process::{Resource, geteuid};
    use std::{os::unix::fs::MetadataExt, path::Path, sync::atomic::AtomicBool, time::Duration};

    const BWRAP: &str = "/usr/bin/bwrap";
    const NGSPICE: &str = "/usr/bin/ngspice";
    const CAT: &str = "/usr/bin/cat";
    const SHELL: &str = "/usr/bin/sh";
    const OUTPUT: usize = 8 * 1024 * 1024;

    // These commands and paths are fixed by the application. Deck bytes are supplied as
    // an independent input and never become command arguments, shell syntax, or environment.
    const RUN: &str = "if /solver -n -b -o /tmp/solver.log -r /tmp/result.raw /deck.cir >/dev/null && test -s /tmp/result.raw; then exec /print /tmp/result.raw; fi; /print /tmp/solver.log >&2; exit 1";
    const PROBE: &str = "test ! -e /etc/passwd && test ! -e /proc/self && test ! -e /run/user && test -w /tmp && test -c /dev/null && test -x /solver && test -x /print && read -r token </probe && test \"$token\" = ready && test ! -w /probe";

    fn trusted_executable(path: &str) -> Result<(), String> {
        let metadata = std::fs::metadata(path).map_err(|error| format!("{path}: {error}"))?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.mode() & 0o6022 != 0
            || metadata.mode() & 0o111 == 0
        {
            return Err(format!(
                "{path} must be a root-owned executable without set-ID or writable group/other bits"
            ));
        }
        Ok(())
    }

    fn request(timeout: Duration) -> Result<CommandRequest, String> {
        if geteuid().is_root() {
            return Err("Simulation requires an unprivileged Files session".into());
        }
        for path in [BWRAP, NGSPICE, CAT, SHELL] {
            trusted_executable(path)?;
        }
        let mut request = CommandRequest::new(BWRAP)
            .clear_environment()
            .current_dir("/")
            .own_process_group()
            .die_with_parent()
            .timeout(timeout)
            .max_output_bytes(OUTPUT)
            .resource_limit(Resource::Cpu, 5)
            .resource_limit(Resource::As, 768 * 1024 * 1024)
            .resource_limit(Resource::Fsize, OUTPUT as u64)
            .resource_limit(Resource::Core, 0)
            .args([
                "--unshare-user",
                "--unshare-pid",
                "--unshare-net",
                "--unshare-ipc",
                "--unshare-uts",
                "--disable-userns",
                "--die-with-parent",
                "--cap-drop",
                "ALL",
                "--clearenv",
                "--setenv",
                "HOME",
                "/tmp",
                "--setenv",
                "LC_ALL",
                "C",
                "--setenv",
                "OMP_NUM_THREADS",
                "1",
                "--ro-bind",
                NGSPICE,
                "/solver",
                "--ro-bind",
                CAT,
                "/print",
                "--ro-bind",
                SHELL,
                "/shell",
                "--dev",
                "/dev",
                "--size",
                "16777216",
                "--tmpfs",
                "/tmp",
                "--chdir",
                "/tmp",
            ]);
        for path in ["/usr/lib", "/usr/lib64", "/lib", "/lib64"] {
            if Path::new(path).is_dir() {
                request = request.args(["--ro-bind", path, path]);
            }
        }
        Ok(request)
    }

    pub(super) fn probe(cancel: &AtomicBool) -> Result<(), String> {
        let output = request(Duration::from_secs(3))?
            .args(["--ro-bind-data", "0", "/probe", "--", "/shell", "-c", PROBE])
            .stdin_bytes(b"ready\n".to_vec())
            .max_output_bytes(4096)
            .run_cancellable(cancel)
            .map_err(|error| format!("Sandbox check failed: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(diagnostic(
                "Required sandbox could not be established",
                &output,
            ))
        }
    }

    pub(super) fn run(bytes: Vec<u8>, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        let output = request(Duration::from_secs(10))?
            .args([
                "--ro-bind-data",
                "0",
                "/deck.cir",
                "--",
                "/shell",
                "-c",
                RUN,
            ])
            .stdin_bytes(bytes)
            .run_cancellable(cancel)
            .map_err(|error| format!("Sandboxed simulation failed: {error}"))?;
        if !output.status.success() {
            return Err(diagnostic("Sandboxed simulation failed", &output));
        }
        if output.stdout_truncated || output.stdout.len() >= OUTPUT {
            return Err("Solver output reached the 8 MiB limit".into());
        }
        Ok(output.stdout)
    }

    fn diagnostic(summary: &str, output: &CommandOutput) -> String {
        let message: String = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(500)
            .collect();
        if message.is_empty() {
            format!("{summary} ({})", output.status)
        } else {
            format!("{summary}: {message}")
        }
    }

    #[test]
    fn sandbox_cancellation_reaps_a_running_worker() {
        use crate::process::CommandError;
        use std::{sync::atomic::Ordering, time::Instant};

        let cancel = AtomicBool::new(false);
        if let Err(reason) = probe(&cancel) {
            eprintln!("sandbox cancellation check unavailable: {reason}");
            return;
        }
        let started = Instant::now();
        let result = request(Duration::from_secs(10))
            .unwrap()
            .args(["--", "/shell", "-c", "while :; do :; done"])
            .run_cancellable_with_progress(&cancel, || {
                if started.elapsed() >= Duration::from_millis(100) {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
        assert!(matches!(result, Err(CommandError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::sync::atomic::AtomicBool;

    pub(super) fn probe(_: &AtomicBool) -> Result<(), String> {
        Err("Simulation requires Linux with Bubblewrap and ngspice".into())
    }

    pub(super) fn run(_: Vec<u8>, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        probe(cancel).map(|_| Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eda::spice::RawAnalysisKind;

    const RC: &[u8] = include_bytes!("../../../tests/fixtures/eda/trusted_rc.cir");

    #[test]
    fn simulation_rejects_dependencies_commands_and_unbounded_analyses() {
        let text = std::str::from_utf8(RC).unwrap();
        assert!(deck::admit(RC).is_ok());
        for unsupported in [
            ".include local.cir",
            ".lib models.lib",
            ".model MOS NMOS",
            ".control",
            ".subckt macro a b",
            ".save v(out)",
            ".param x=1",
            "X1 in 0 macro",
            "V2 out 0 {gain}",
            "I2 out 0 PWL(FILE=wave.txt)",
            "+ DC 1",
            "R3 a b -1k",
            "r1 a b 2k",
            ".tran 1n 600",
            ".tran 1n 1",
            ".tran 1m 4m UIC",
            "V2 a b 1; .include local.cir",
            "V2 a b 1\r.op",
        ] {
            let deck = if unsupported.starts_with(".tran") {
                text.replace(".tran 5u 4m", unsupported)
            } else {
                text.replace(".end", &format!("{unsupported}\n.end"))
            };
            assert!(deck::admit(deck.as_bytes()).is_err(), "{unsupported}");
        }
    }

    #[test]
    fn simulation_rejects_a_replaced_deck_and_a_precancelled_request() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("original.cir");
        let replacement = temp.path().join("replacement.cir");
        std::fs::write(&path, RC).unwrap();
        let source = FileSource::capture(&path).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(simulate(&source, &cancel).is_err());
        std::fs::write(&replacement, RC).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert!(simulate(&source, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn sandboxed_simulation_reads_the_known_rc_result() {
        let cancel = AtomicBool::new(false);
        if let Err(reason) = check_support(&cancel) {
            eprintln!("sandboxed RC check unavailable: {reason}");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("case.cir");
        std::fs::write(&path, RC).unwrap();
        let doc = simulate(&FileSource::capture(&path).unwrap(), &cancel).unwrap();
        let trace = |kind: RawAnalysisKind| {
            doc.plots
                .iter()
                .find(|plot| plot.kind == kind)
                .unwrap()
                .traces
                .iter()
                .find(|trace| trace.name.eq_ignore_ascii_case("v(out)"))
                .unwrap()
        };
        let op = trace(RawAnalysisKind::DcOperatingPoint).samples()[0];
        assert!((op.value - 1.0).abs() < 1e-9);
        let transient = trace(RawAnalysisKind::Transient).samples().last().unwrap();
        assert!((transient.x - 0.004).abs() < 1e-12);
        assert!((transient.value - 0.999_664_224).abs() < 2e-6);
    }
}
