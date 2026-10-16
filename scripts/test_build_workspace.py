"""Exercise cleanup against real, offline Cargo builds in an isolated directory."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from build_workspace import artifact_hash, build_workspace, prune_artifacts


class WorkspaceCacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="clip-build-cache-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        environment = patch.dict(os.environ, {
            "CARGO_TARGET_DIR": str(self.root / "target"),
            "CARGO_BUILD_BUILD_DIR": str(self.root / "target"),
        })
        environment.start()
        self.addCleanup(environment.stop)
        os.environ.pop("CARGO_BUILD_TARGET", None)
        self.previous_cwd = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self.previous_cwd)
        for package in ("app", "dep"):
            (self.root / package / "src").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers=["app", "dep"]\nresolver="2"\n')
        (self.root / "app/Cargo.toml").write_text(
            '[package]\nname="cache-app"\nversion="0.1.0"\nedition="2024"\n'
            '[dependencies]\ncache-dep={path="../dep"}\n'
            '[build-dependencies]\ncache-dep={path="../dep", features=["build"]}\n')
        (self.root / "dep/Cargo.toml").write_text(
            '[package]\nname="cache-dep"\nversion="0.1.0"\nedition="2024"\n'
            '[features]\nbuild=[]\n')
        (self.root / "dep/src/lib.rs").write_text('pub fn value() -> u32 { 1 }\n')
        (self.root / "dep/build.rs").write_text('fn main() {}\n')
        (self.root / "app/src/main.rs").write_text(
            'fn main() { println!("{}", cache_dep::value()); }\n')
        (self.root / "app/build.rs").write_text(
            'fn main() { println!("cargo:rustc-env=VALUE={}", cache_dep::value()); }\n')

    def cargo(self, *args):
        result = subprocess.run(
            ["cargo", "build", "--offline", "--message-format=json-render-diagnostics", *args],
            capture_output=True, text=True, check=True,
        )
        return [json.loads(line) for line in result.stdout.splitlines()]

    def test_retains_fresh_graph_and_build_scripts_but_removes_obsolete_versions(self):
        self.cargo()  # Debug artifacts must survive release cleanup.
        debug = set((self.root / "target/debug/deps").iterdir())
        self.cargo("--release")
        old = set((self.root / "target/release/.fingerprint").iterdir())
        manifest = self.root / "dep/Cargo.toml"
        manifest.write_text(manifest.read_text().replace('version="0.1.0"', 'version="0.2.0"'))
        current = self.cargo("--release")
        self.assertTrue(any(not m["fresh"] for m in current if m["reason"] == "compiler-artifact"))
        helper = self.root / "target/tools/license.txt"
        helper.parent.mkdir(parents=True)
        helper.write_text("keep bundled helpers")
        self.assertEqual(build_workspace(), 0)  # Entire graph is already fresh.
        remaining = set((self.root / "target/release/.fingerprint").iterdir())
        self.assertTrue(old - remaining)
        self.assertTrue(all(p.exists() for p in debug))
        self.assertTrue(helper.exists())
        after = self.cargo("--release")
        artifacts = [m for m in after if m["reason"] == "compiler-artifact"]
        self.assertTrue(all(m["fresh"] for m in artifacts), after)
        self.assertTrue(any(m["reason"] == "build-script-executed" for m in after))
        self.assertGreaterEqual(sum(m["target"]["name"] == "cache_dep" for m in artifacts), 2)

    def test_failed_build_does_not_clean_any_artifacts(self):
        self.cargo("--release")
        obsolete = self.root / "target/release/deps/old-1234567890abcdef.rlib"
        obsolete.write_bytes(b"old cache")
        (self.root / "app/src/main.rs").write_text("invalid Rust")
        self.assertNotEqual(build_workspace(), 0)
        self.assertTrue(obsolete.exists())

    def test_incomplete_inventory_fails_before_removing_anything(self):
        self.cargo("--release")
        profile = self.root / "target/release"
        before = set((profile / ".fingerprint").iterdir())
        with self.assertRaises(ValueError):
            prune_artifacts({profile: {"0000000000000000"}})
        self.assertEqual(before, set((profile / ".fingerprint").iterdir()))

    def test_hash_parser_ignores_unhashed_files(self):
        self.assertEqual(artifact_hash(Path("libdep-0123456789abcdef.rlib")), "0123456789abcdef")
        self.assertIsNone(artifact_hash(Path("build-script-build")))


if __name__ == "__main__":
    unittest.main()
