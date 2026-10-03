"""Offline input invalidation, immutable artifact integrity and original provenance."""

import json
import os
import shutil
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import lean_voice_cache as cache
import lean_voice as voice
from runtime import PLUGINS, digest, required_library_paths

TARGET = "x86_64-unknown-linux-gnu"
COMMIT = "a" * 40
IMAGE = {"ImageOS": "ubuntu24", "ImageVersion": "20261001.1"}
TOOLS = {"python": "3.12.9"}


class FingerprintTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name in cache.FILES:
            self.write(name, "pinned input\n")
        for name in cache.TREES:
            self.write(name + "/input", "recipe\n")
        self.write("codex-rs/Cargo.toml", '''[workspace]
members = ["voice-host", "shared", "cli"]
[workspace.package]
version = "1.0.0-lean.1"
[workspace.dependencies]
protocol = { path = "shared" }
''')
        self.write("codex-rs/voice-host/Cargo.toml", '''[package]
name = "voice"
[dependencies]
protocol = { workspace = true }
[target.'cfg(windows)'.build-dependencies]
windows = { path = "../windows" }
''')
        self.write("codex-rs/shared/Cargo.toml", '[package]\nname = "protocol"\n')
        self.write("codex-rs/windows/Cargo.toml", '[package]\nname = "windows"\n')
        self.write("codex-rs/cli/Cargo.toml", '[package]\nname = "cli"\n')
        self.write("codex-rs/shared/src/lib.rs", "shared protocol\n")
        self.write("codex-rs/voice-host/src/main.rs", "voice helper\n")
        self.original = cache.source_fingerprint(self.root, TARGET)

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def test_cli_skills_and_build_outputs_do_not_change_native_identity(self):
        for name in ("codex-rs/cli/src/main.rs", "codex-rs/skills/src/lib.rs",
                     "codex-rs/voice-host/target/native.o", "third_party/voice/__pycache__/runtime.pyc"):
            self.write(name, "unrelated edit")
        self.assertEqual(cache.source_fingerprint(self.root, TARGET), self.original)

    def test_protocol_transitive_platform_build_deps_and_tool_pins_invalidate(self):
        for name in ("codex-rs/shared/src/lib.rs", "codex-rs/windows/build.rs",
                     "codex-rs/cli/Cargo.toml", "codex-rs/Cargo.lock", "MODULE.bazel.lock",
                     "third_party/voice/input", "patches/input", "bazel/input",
                     ".github/workflows/lean-release.yml", ".github/actions/setup-msvc-env/input"):
            path = self.root / name
            previous = path.read_bytes() if path.exists() else None
            with self.subTest(name=name):
                self.write(name, (previous.decode() if previous else "") + "\n# meaningful change\n")
                self.assertNotEqual(cache.source_fingerprint(self.root, TARGET), self.original)
                if previous is None:
                    path.unlink()
                else:
                    path.write_bytes(previous)

    def test_new_transitive_dependency_and_workspace_version_are_meaningful(self):
        path = self.root / "codex-rs/shared/Cargo.toml"
        original = path.read_text()
        self.write("codex-rs/leaf/Cargo.toml", '[package]\nname = "leaf"\n')
        path.write_text(original + '[dependencies]\nleaf = { path = "../leaf" }\n')
        before = cache.source_fingerprint(self.root, TARGET)
        self.write("codex-rs/leaf/src/lib.rs", "new dependency implementation")
        self.assertNotEqual(cache.source_fingerprint(self.root, TARGET), before)
        path.write_text(original)
        workspace = self.root / "codex-rs/Cargo.toml"
        workspace.write_text(workspace.read_text().replace("1.0.0-lean.1", "1.0.0-lean.2"))
        self.assertNotEqual(cache.source_fingerprint(self.root, TARGET), self.original)

    def test_platform_and_native_runner_image_change_the_exact_key(self):
        self.assertNotEqual(cache.source_fingerprint(self.root, "aarch64-apple-darwin"), self.original)
        original = cache.input_fingerprint(self.original, IMAGE, TOOLS)
        for field in IMAGE:
            image = {**IMAGE, field: IMAGE[field] + "-changed"}
            self.assertNotEqual(cache.input_fingerprint(self.original, image, TOOLS), original)
        self.assertNotEqual(cache.input_fingerprint(self.original, IMAGE, {"python": "3.12.10"}), original)
        with patch.dict(os.environ, {"ImageOS": "", "ImageVersion": ""}):
            with self.assertRaisesRegex(ValueError, "required"):
                cache.image_identity()

    def test_record_order_is_portable_between_windows_and_linux_checkouts(self):
        # PureWindowsPath orders paths case-insensitively. POSIX paths do not.
        # Include BUILD.bazel and bazel so native and publisher hashes cannot diverge.
        records = [[path.relative_to(self.root).as_posix(), digest(path)]
                   for path in self.root.rglob("*") if path.is_file()]
        records.sort(key=lambda record: record[0])
        expected = cache.json_digest({"schemaVersion": 1, "target": TARGET, "inputs": records})
        self.assertEqual(cache.source_fingerprint(self.root, TARGET), expected)

    def test_checkout_proof_rejects_self_attested_input_changes(self):
        proof = {"schemaVersion": 1, "sourceCommit": COMMIT, "target": TARGET,
                 "sourceFingerprint": self.original, "runnerImage": IMAGE,
                 "toolVersions": TOOLS,
                 "inputFingerprint": cache.input_fingerprint(self.original, IMAGE, TOOLS),
                 "archiveSha256": "0" * 64, "helperSha256": "1" * 64}
        self.assertEqual(cache.validate_provenance(proof, TARGET, root=self.root), COMMIT)
        self.write("codex-rs/shared/src/lib.rs", "changed protocol")
        with self.assertRaisesRegex(ValueError, "provenance"):
            cache.validate_provenance(proof, TARGET, root=self.root)
        proof["sourceFingerprint"] = "2" * 64
        proof["inputFingerprint"] = cache.input_fingerprint("2" * 64, IMAGE, TOOLS)
        with self.assertRaisesRegex(ValueError, "provenance"):
            cache.validate_provenance(proof, TARGET, root=self.root)


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.staged = self.root / "staged"
        self.runtime = self.staged / "runtime"
        self.runtime.mkdir(parents=True)
        self.helper = self.staged / "codex-voice-host"
        self.helper.write_bytes(b"offline fixture, not native")
        self.helper.chmod(0o755)
        plugins = [f"lib/gstreamer-1.0/libgst{name}.so" for name in PLUGINS]
        libraries = []
        for name in sorted({*plugins, *required_library_paths(TARGET)}):
            path = self.runtime / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"runtime fixture")
            libraries.append({"path": name, "sha256": digest(path)})
        self.receipt = self.runtime / "runtime.json"
        self.receipt.write_text(json.dumps({
            "schemaVersion": 1, "target": TARGET, "sourceCommit": COMMIT,
            "sourceManifestSha256": digest(cache.ROOT / "third_party/voice/sources.json"),
            "developmentOnly": False, "distribution": "publicRelease",
            "libraries": libraries, "plugins": plugins,
        }))
        self.directory = self.root / "cache"
        self.directory.mkdir()
        self.archive = self.directory / f"lean-voice-{TARGET}.tar.gz"
        self.proof_path = self.directory / f"lean-voice-{TARGET}.provenance.json"
        self.write_archive()
        with patch.dict(os.environ, IMAGE):
            cache.seal_artifact(self.directory, self.staged, TARGET, COMMIT)
        self.proof = json.loads(self.proof_path.read_text())

    def write_archive(self):
        with tarfile.open(self.archive, "w:gz") as archive:
            for name in ("runtime", "codex-voice-host"):
                archive.add(self.staged / name, arcname=name)

    def verify(self):
        output = self.root / "extracted"
        shutil.rmtree(output, ignore_errors=True)
        with patch.dict(os.environ, IMAGE):
            return voice.verify_artifact(self.directory, TARGET, output)

    def test_verified_hit_preserves_original_commit_and_exact_archive_bytes(self):
        original = self.archive.read_bytes()
        self.assertEqual(self.verify()["sourceCommit"], COMMIT)
        self.assertEqual(self.archive.read_bytes(), original)
        with patch.dict(os.environ, {**IMAGE, "ImageVersion": "different"}):
            with self.assertRaisesRegex(ValueError, "runner image"):
                voice.verify_artifact(self.directory, TARGET, self.root / "different-image")

    def test_archive_digest_is_checked_before_extraction(self):
        self.archive.write_bytes(self.archive.read_bytes() + b"tamper")
        with self.assertRaisesRegex(ValueError, "archive digest"):
            self.verify()
        self.assertFalse((self.root / "extracted").exists())

    def test_recomputed_archive_digest_cannot_hide_helper_or_runtime_tampering(self):
        for path in (self.helper, self.runtime / required_library_paths(TARGET)[0]):
            original = path.read_bytes()
            with self.subTest(path=path):
                path.write_bytes(original + b"tamper")
                self.write_archive()
                proof = {**self.proof, "archiveSha256": digest(self.archive)}
                self.proof_path.write_text(json.dumps(proof))
                with self.assertRaisesRegex(ValueError, "digest"):
                    self.verify()
                path.write_bytes(original)

    def test_runtime_original_commit_and_inventory_cannot_be_relabelled(self):
        receipt = json.loads(self.receipt.read_text())
        receipt["sourceCommit"] = "b" * 40
        self.receipt.write_text(json.dumps(receipt))
        with self.assertRaisesRegex(ValueError, "original helper build"):
            cache.validate_payload(self.helper, self.runtime, self.proof, TARGET)
        receipt["sourceCommit"] = COMMIT
        self.receipt.write_text(json.dumps(receipt))
        (self.runtime / "unexpected").write_bytes(b"unlisted")
        proof = {**self.proof, "runtimeSha256": cache.inventory(self.runtime)}
        with self.assertRaisesRegex(ValueError, "unlisted"):
            cache.validate_payload(self.helper, self.runtime, proof, TARGET)

    def test_failed_native_smoke_never_exports_voice_stamp(self):
        env_file = self.root / "env"
        with patch.dict(os.environ, {**IMAGE, "GITHUB_ENV": str(env_file)}), \
                patch("sys.argv", ["cache", "verify", "--target", TARGET,
                                   "--directory", str(self.directory), "--export-env"]), \
                patch.object(voice, "native_smoke", side_effect=ValueError("wrong helper stamp")):
            with self.assertRaisesRegex(ValueError, "wrong helper stamp"):
                voice.main()
        self.assertFalse(env_file.exists())

    def test_verified_export_uses_original_voice_commit_not_current_app_commit(self):
        env_file = self.root / "env"
        with patch.dict(os.environ, {**IMAGE, "GITHUB_ENV": str(env_file),
                                     "STABLE_GIT_COMMIT": "b" * 40}), \
                patch("sys.argv", ["voice", "verify", "--target", TARGET,
                                   "--directory", str(self.directory), "--export-env"]), \
                patch.object(voice, "native_smoke") as smoke:
            voice.main()
        self.assertEqual(env_file.read_text(), f"CODEX_VOICE_BUILD_COMMIT={COMMIT}\n")
        self.assertEqual(smoke.call_args.args[1:], (TARGET, COMMIT))


if __name__ == "__main__":
    unittest.main()
