"""Test CRT receipt updates independently of the native PE inspection tools."""

import hashlib
import json
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import windows_crt


class WindowsCrtTests(unittest.TestCase):
    def test_staging_updates_a_read_only_bazel_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "runtime"
            (runtime / "bin").mkdir(parents=True)
            manifest_path = runtime / "runtime.json"
            target = "x86_64-pc-windows-msvc"
            manifest_path.write_text(json.dumps({
                "target": target, "developmentOnly": True, "libraries": [],
            }), encoding="utf-8")
            manifest_path.chmod(0o444)
            # The archive exercises the actual pinned extraction path. PE import
            # inspection is outside this permission and receipt regression.
            data = b"retail CRT fixture"
            digest = hashlib.sha256(data).hexdigest()
            archive = root / "crt.zip"
            member = "retail/vcruntime140.dll"
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr(member, data)
            (root / "windows-crt.json").write_text(json.dumps({target: {
                "url": archive.as_uri(),
                "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                "member": member,
                "dllSha256": digest,
            }}), encoding="utf-8")
            try:
                with (
                    patch.object(windows_crt, "__file__", str(root / "windows_crt.py")),
                    patch.object(windows_crt.subprocess, "run", return_value=
                                 subprocess.CompletedProcess([], 0, b"", b"")),
                ):
                    windows_crt.stage(runtime, target, root / "helper.exe")
                self.assertTrue(manifest_path.stat().st_mode & stat.S_IWUSR)
                self.assertEqual(json.loads(manifest_path.read_text(encoding="utf-8")), {
                    "target": target, "developmentOnly": True,
                    "libraries": [{"path": "bin/vcruntime140.dll", "sha256": digest}],
                })
                self.assertEqual((runtime / "bin/vcruntime140.dll").read_bytes(), data)
            finally:
                manifest_path.chmod(0o644)


if __name__ == "__main__":
    unittest.main()
