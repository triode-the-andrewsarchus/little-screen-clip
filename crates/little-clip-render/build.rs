//! Build pinned local engines and publish immutable companion artifacts.
use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest")).join("../..");
    for path in [
        "scripts/goat-renderer",
        "vendor/goat",
        "vendor/tdewolff-parse",
        "vendor/ditaa",
        "vendor/umlet",
        "crates/little-clip-render/assets/fonts/graphviz/Cousine-Bold.ttf",
        "crates/little-clip-render/assets/fonts/graphviz/Cousine-Regular.ttf",
        "crates/little-clip-render/assets/fonts/graphviz/Tinos-Regular.ttf",
        "crates/little-clip-render/assets/fonts/graphviz/Arimo[wght].ttf",
        "crates/little-clip-render/assets/fonts/graphviz/OFL-Cousine.txt",
        "crates/little-clip-render/assets/fonts/graphviz/OFL-Tinos.txt",
        "crates/little-clip-render/assets/fonts/graphviz/OFL-Arimo.txt",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    println!("cargo:rerun-if-env-changed=LITTLE_CLIP_GO");
    println!("cargo:rerun-if-env-changed=LITTLE_CLIP_JAVAC");
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
    publish(&binary, &out, "clip-goat", "GOAT");
    for (family, directory) in [("DITAA", "ditaa"), ("UMLET", "umlet")] {
        let companion = out.join(format!("clip-{directory}.jar"));
        let result = Command::new("python3")
            .arg(root.join(format!("vendor/{directory}/build.py")))
            .arg(&companion)
            .output()
            .expect("Building diagram companions requires Python 3 and a local JDK 11+");
        assert!(
            result.status.success(),
            "Bundled {family} build failed (local JDK 11+ required; LITTLE_CLIP_JAVAC selects javac):\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        publish(&companion, &out, &format!("clip-{directory}"), family);
    }
}

fn publish(artifact: &Path, out: &Path, stem: &str, family: &str) {
    // Versioned sidecars avoid replacing an artifact in use and let older
    // running applications keep their exact helper during a rebuild. Cargo's
    // retained build directory is also a development/test fallback.
    let bytes = fs::read(artifact).expect("Built diagram companion");
    let mut hash = DefaultHasher::new();
    bytes.hash(&mut hash);
    let suffix = artifact
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let name = format!("{stem}-{:016x}{suffix}", hash.finish());
    let profile = out.ancestors().nth(3).expect("Cargo profile directory");
    let bundled = profile.join(&name);
    if !bundled.exists() {
        let staging = profile.join(format!("{name}.tmp-{}", std::process::id()));
        fs::copy(artifact, &staging).expect("Stage bundled diagram companion");
        fs::rename(staging, bundled).expect("Publish bundled diagram companion");
    }
    println!("cargo:rustc-env=LITTLE_CLIP_{family}_HELPER_NAME={name}");
    println!(
        "cargo:rustc-env=LITTLE_CLIP_{family}_HELPER_PATH={}",
        artifact.display()
    );
}
