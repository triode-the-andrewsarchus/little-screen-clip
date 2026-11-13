//! Build the pinned GoAT engine without module downloads or a runtime install.
use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    process::Command,
};

fn main() {
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest")).join("../..");
    for path in [
        "scripts/goat-renderer",
        "vendor/goat",
        "vendor/tdewolff-parse",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    println!("cargo:rerun-if-env-changed=LITTLE_CLIP_GO");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo target OS");
    let go_os = match target_os.as_str() {
        "macos" => "darwin",
        "linux" | "windows" | "freebsd" | "openbsd" | "netbsd" => &target_os,
        other => panic!("Bundled GoAT does not support the {other} target"),
    };
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo target architecture");
    let go_arch = match arch.as_str() {
        "x86_64" => "amd64",
        "x86" => "386",
        "aarch64" => "arm64",
        "arm" | "riscv64" => &arch,
        other => panic!("Bundled GoAT does not support the {other} architecture"),
    };
    let suffix = if target_os == "windows" { ".exe" } else { "" };
    let binary = out.join(format!("clip-goat{suffix}"));
    let result = Command::new(env::var_os("LITTLE_CLIP_GO").unwrap_or_else(|| "go".into()))
        .current_dir(root.join("scripts/goat-renderer"))
        .args([
            "build",
            "-mod=readonly",
            "-trimpath",
            "-buildvcs=false",
            "-p",
            "1",
            "-ldflags=-s -w",
            "-o",
        ])
        .arg(&binary)
        .arg(".")
        .env("GOOS", go_os)
        .env("GOARCH", go_arch)
        .env("CGO_ENABLED", "0")
        .env("GOPROXY", "off")
        .env("GOSUMDB", "off")
        .env("GOTOOLCHAIN", "local")
        .env("GOMAXPROCS", "2")
        .output()
        .expect("Building native GoAT requires Go 1.25+ (or set LITTLE_CLIP_GO)");
    assert!(
        result.status.success(),
        "Bundled GoAT build failed (Go 1.25+ required):\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    // Versioned sidecars avoid replacing an executable in use and let older
    // running applications keep their exact helper during a rebuild. Cargo's
    // retained build directory is also a development/test fallback.
    let bytes = fs::read(&binary).expect("Compiled GoAT helper");
    let mut hash = DefaultHasher::new();
    bytes.hash(&mut hash);
    let name = format!("clip-goat-{:016x}{suffix}", hash.finish());
    let profile = out.ancestors().nth(3).expect("Cargo profile directory");
    let bundled = profile.join(&name);
    if !bundled.exists() {
        let staging = profile.join(format!("{name}.tmp-{}", std::process::id()));
        fs::copy(&binary, &staging).expect("Stage bundled GoAT helper");
        fs::rename(staging, bundled).expect("Publish bundled GoAT helper");
    }
    println!("cargo:rustc-env=LITTLE_CLIP_GOAT_HELPER_NAME={name}");
    println!(
        "cargo:rustc-env=LITTLE_CLIP_GOAT_HELPER_PATH={}",
        binary.display()
    );
}
