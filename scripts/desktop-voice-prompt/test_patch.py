import contextlib
import hashlib
import io
import json
import os
import struct
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch as mock_patch

import patch
from asar import Asar, UnsupportedBundle


def fixture(path: Path, files: dict[str, bytes]) -> None:
    header = {"files": {}}
    payload = b""
    for name, content in files.items():
        directory = header
        parts = name.split("/")
        for part in parts[:-1]:
            directory = directory["files"].setdefault(part, {"files": {}})
        directory["files"][parts[-1]] = {
            "offset": str(len(payload)),
            "size": len(content),
            "integrity": {
                "algorithm": "SHA256",
                "hash": hashlib.sha256(content).hexdigest(),
                "blockSize": 8,
                "blocks": [
                    hashlib.sha256(content[start : start + 8]).hexdigest()
                    for start in range(0, len(content), 8)
                ],
            },
        }
        payload += content
    header["files"]["alias"] = {"link": "untouched"}
    header["files"]["native.node"] = {"size": 99, "unpacked": True, "executable": True}
    header["files"]["deduplicated"] = dict(header["files"]["untouched"])
    write_archive(path, header, payload)


def write_archive(path, header, payload):
    encoded = json.dumps(header).encode()
    padded = encoded + b"\0" * (-len(encoded) % 4)
    path.write_bytes(
        struct.pack("<4I", 4, 8 + len(padded), 4 + len(padded), len(encoded))
        + padded
        + payload
    )


class PatcherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.source = self.directory / "original.asar"
        self.output = self.directory / "patched.asar"
        self.files = {
            "package.json": json.dumps(
                {
                    "name": "openai-codex-electron",
                    "productName": "Codex",
                    "version": patch.VERSION,
                }
            ).encode(),
            patch.MAIN: b"const e=require('original');",
            patch.PRELOAD: b"e.contextBridge.exposeInMainWorld(`electronBridge`,z);",
            patch.PARAMETERS: (
                "function mi(e,{ambientParameters:t,explicitParameters:n,liveParameters:r,systemHints:i})"
                "{return {" + patch.ANCHOR + "}}"
            ).encode(),
            "untouched": b"native transport and tools remain byte-identical",
        }
        fixture(self.source, self.files)

    def run_cli(self, *arguments):
        with (
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            return patch.main(["--asar", str(self.source), *arguments])

    def trust_fixture(self):
        # Synthetic archives exercise our decisions, not external compatibility.
        return mock_patch.object(patch, "SUPPORTED_SHA256", patch.digest(self.source))

    def test_archive_round_trip_preserves_original_entries_and_rehashes_changed_blocks(
        self,
    ):
        original = Asar(self.source)
        content = b"replacement with multiple integrity blocks"
        original.write(self.output, {patch.MAIN: content})
        result = Asar(self.output)
        self.assertEqual(result.read(patch.MAIN), content)
        for name, entry in original.entries.items():
            if name == patch.MAIN:
                continue
            self.assertEqual(result.entries[name], entry)
            if "offset" in entry:
                self.assertEqual(result.read(name), original.read(name))
        integrity = result.entries[patch.MAIN]["integrity"]
        self.assertEqual(integrity["hash"], hashlib.sha256(content).hexdigest())
        self.assertEqual(
            integrity["blocks"],
            [
                hashlib.sha256(content[i : i + 8]).hexdigest()
                for i in range(0, len(content), 8)
            ],
        )

    def test_patch_reads_no_prompt_and_leaves_source_and_native_renderer_code_intact(
        self,
    ):
        before = self.source.read_bytes()
        prompt = self.directory / 'not-yet-created-"${literal}`.md'
        with self.trust_fixture():
            self.assertEqual(
                self.run_cli(
                    "--prompt-file", str(prompt), "--output", str(self.output)
                ),
                0,
            )
        self.assertEqual(self.source.read_bytes(), before)
        result = Asar(self.output)
        self.assertEqual(
            result.read(patch.PARAMETERS),
            self.files[patch.PARAMETERS].replace(
                patch.ANCHOR.encode(), patch.REPLACEMENT.encode()
            ),
        )
        self.assertTrue(result.read(patch.MAIN).endswith(self.files[patch.MAIN]))
        self.assertTrue(result.read(patch.PRELOAD).endswith(self.files[patch.PRELOAD]))
        self.assertIn(json.dumps(str(prompt)).encode(), result.read(patch.MAIN))
        self.assertEqual(result.read("untouched"), self.files["untouched"])

    def test_check_is_read_only(self):
        before = self.source.read_bytes()
        with self.trust_fixture():
            self.assertEqual(self.run_cli("--check"), 0)
        self.assertEqual(self.source.read_bytes(), before)
        self.assertFalse(self.output.exists())

    def test_rejects_unknown_identity_hash_and_duplicate_anchor_before_output(self):
        for fault in ("version", "hash", "anchor"):
            with self.subTest(fault=fault):
                files = dict(self.files)
                if fault == "version":
                    files["package.json"] = files["package.json"].replace(
                        patch.VERSION.encode(), b"99.0"
                    )
                elif fault == "anchor":
                    files[patch.PARAMETERS] *= 2
                fixture(self.source, files)
                with self.trust_fixture():
                    if fault == "hash":
                        with mock_patch.object(patch, "SUPPORTED_SHA256", "0" * 64):
                            self.assertEqual(self.run_cli("--check"), 1)
                    else:
                        self.assertEqual(self.run_cli("--check"), 1)
                self.assertFalse(self.output.exists())

    def test_rejects_in_place_existing_output_and_symlink_without_clobber(self):
        with self.trust_fixture():
            self.assertEqual(
                self.run_cli(
                    "--prompt-file", "/tmp/custom.md", "--output", str(self.source)
                ),
                1,
            )
            self.output.write_bytes(b"keep me")
            self.assertEqual(
                self.run_cli(
                    "--prompt-file", "/tmp/custom.md", "--output", str(self.output)
                ),
                1,
            )
            self.assertEqual(self.output.read_bytes(), b"keep me")
            self.output.unlink()
            self.output.symlink_to(self.directory / "absent")
            self.assertEqual(
                self.run_cli(
                    "--prompt-file", "/tmp/custom.md", "--output", str(self.output)
                ),
                1,
            )
            self.assertTrue(self.output.is_symlink())
            self.assertFalse((self.directory / "absent").exists())
        self.assertEqual(
            sorted(p.name for p in self.directory.iterdir()),
            ["original.asar", "patched.asar"],
        )

    def test_protected_prompt_and_alias_and_relative_paths_are_rejected(self):
        protected = Path.home() / ".pi/agent/REALTIME-SYSTEM-PROMPT.md"
        alias = self.directory / "alias.md"
        alias.symlink_to(protected)
        for prompt in (protected, protected.resolve(), alias, Path("relative.md")):
            with self.subTest(prompt=prompt), self.assertRaises(ValueError):
                patch.validate_prompt_path(prompt)

    def test_rejects_truncation_out_of_bounds_and_partial_overlap(self):
        self.source.write_bytes(b"bad")
        with self.assertRaises(UnsupportedBundle):
            Asar(self.source)
        for entry in ({"size": 2, "offset": "1000"}, {"size": 3, "offset": "1"}):
            write_archive(
                self.source,
                {"files": {"a": {"size": 3, "offset": "0"}, "b": entry}},
                b"abcd",
            )
            with self.assertRaises(UnsupportedBundle):
                Asar(self.source)

    def test_input_replacement_after_parsing_is_rejected_before_publication(self):
        original = Asar(self.source)
        alternate = self.directory / "alternate.asar"
        fixture(alternate, self.files)
        alternate.replace(self.source)
        with self.assertRaisesRegex(UnsupportedBundle, "changed during patching"):
            original.write(self.output, {patch.MAIN: b"replacement"})
        self.assertFalse(self.output.exists())

    def test_runtime_loader_with_real_files_and_native_payload_expression(self):
        subprocess.run(
            ["node", "--test", str(patch.HERE / "test_runtime.cjs")],
            check=True,
            env={**os.environ, "CODEX_VOICE_EXPRESSION": patch.REPLACEMENT},
        )


if __name__ == "__main__":
    unittest.main()
