"""Require same-build public voice resources and exercise the installed runtime."""

import concurrent.futures
import json
import os
import re
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path
from typing import BinaryIO

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "third_party/voice"))

from assemble_package import assemble
from package_runtime import runtime_files
from runtime import digest


def voice_target(target: str) -> str:
    return target.removesuffix("-musl") + "-gnu" if target.endswith("-musl") else target


def extract_archive(archive: Path, output: Path) -> None:
    """Extract only regular package files. Do not execute downloaded artifacts here."""
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as source:
            for entry in source.infolist():
                path = Path(entry.filename)
                mode = entry.external_attr >> 16
                if path.is_absolute() or ".." in path.parts or "\\" in entry.filename or mode & 0o170000 == 0o120000:
                    raise ValueError("Unsafe release archive entry")
            source.extractall(output)
    else:
        with tarfile.open(archive, "r:gz") as source:
            if any(not (entry.isfile() or entry.isdir()) for entry in source.getmembers()):
                raise ValueError("Release archives must contain only regular files and directories")
            source.extractall(output, filter="data")


def add_voice(package: Path, target: str, version: str, commit: str) -> Path:
    native_target = voice_target(target)
    archive = REPO_ROOT / "lean-voice-artifact" / f"lean-voice-{native_target}.tar.gz"
    staged = REPO_ROOT / "lean-voice-input" / native_target
    staged.mkdir(parents=True)
    extract_archive(archive, staged)
    suffix = ".exe" if target.endswith("-windows-msvc") else ""
    output = REPO_ROOT / "lean-voice-package" / target
    output.parent.mkdir(exist_ok=True)
    assemble(package, staged / f"codex-voice-host{suffix}", native_target, commit, output,
             runtime=staged / "runtime", release_version=version)
    return output


def validate_voice(package: Path, target: str, version: str, commit: str) -> None:
    voice = package / "codex-resources/voice"
    files = runtime_files(voice.resolve(strict=True), voice_target(target), public_release=True)
    receipt = json.loads((voice / "runtime.json").read_text(encoding="utf-8"))
    manifest = json.loads((voice / "manifest.json").read_text(encoding="utf-8"))
    if (receipt["sourceCommit"] != commit or manifest["schemaVersion"] != 1
            or manifest["buildCommit"] != commit or manifest["appVersion"] != version
            or manifest["appTarget"] != target or manifest["voiceTarget"] != voice_target(target)):
        raise ValueError("Voice runtime and helper must match the app's release commit, version and target")
    suffix = ".exe" if target.endswith("-windows-msvc") else ""
    helper = f"codex-resources/voice/bin/codex-voice-host{suffix}"
    required = {f"bin/codex{suffix}", helper, *(f"codex-resources/voice/{name}" for name in files)}
    required.update(f"codex-resources/voice/{name}" for name in ("NOTICE.md", "sources.json", "licenses/LGPL-2.1.txt"))
    if suffix:
        required.update(("codex-resources/voice/bin/vcruntime140.dll", "codex-resources/voice/windows-crt.json",
                         "codex-resources/voice/bin/gstreamer-1.0-0.dll"))
    elif target.endswith("-musl"):
        required.add("codex-resources/voice/lib/libgstreamer-1.0.so.0")
    else:
        required.add("codex-resources/voice/lib/libgstreamer-1.0.0.dylib")
    hashes = manifest["sha256"]
    if not required.issubset(hashes):
        raise ValueError("Voice manifest omits required app, helper or runtime files")
    for name, expected in hashes.items():
        path = package / name
        if (Path(name).is_absolute() or ".." in Path(name).parts or path.is_symlink()
                or not path.is_file() or not path.resolve().is_relative_to(package.resolve())
                or not re.fullmatch(r"[0-9a-f]{64}", expected) or digest(path) != expected):
            raise ValueError(f"Voice package digest mismatch: {name}")
    actual = {path.relative_to(package).as_posix() for path in voice.rglob("*") if path.is_file()}
    expected = {name for name in hashes if name.startswith("codex-resources/voice/")}
    if actual != expected | {"codex-resources/voice/manifest.json"}:
        raise ValueError("Voice package has unlisted runtime files")


def read_frame(stream: BinaryIO) -> dict:
    def read_exact(length: int) -> bytes:
        chunks = bytearray()
        while len(chunks) < length:
            chunk = stream.read(length - len(chunks))
            if not chunk:
                raise ValueError("Voice helper closed before replying")
            chunks.extend(chunk)
        return bytes(chunks)

    length = struct.unpack(">I", read_exact(4))[0]
    # Keep aligned with codex-realtime-webrtc's MAX_FRAME_BYTES.
    if not 0 < length <= 128 * 1024:
        raise ValueError("Invalid voice helper frame length")
    return json.loads(read_exact(length))


def smoke_voice(package: Path, target: str, commit: str) -> None:
    """Handshake and load all packaged GStreamer plugins without opening devices."""
    suffix = ".exe" if target.endswith("-windows-msvc") else ""
    helper = package / "codex-resources/voice/bin" / f"codex-voice-host{suffix}"
    result = subprocess.run([str(helper), "--build-commit"], check=True, capture_output=True, text=True, timeout=30)
    if result.stdout.strip() != commit:
        raise ValueError("Voice executable was compiled from a different commit")
    env = {**os.environ, "GST_PLUGIN_PATH": "", "GST_PLUGIN_PATH_1_0": "",
           "GST_PLUGIN_SYSTEM_PATH": "", "GST_PLUGIN_SYSTEM_PATH_1_0": "",
           "GST_REGISTRY": "NUL" if suffix else "/dev/null",
           "GST_REGISTRY_UPDATE": "no", "GST_REGISTRY_FORK": "no"}
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen([str(helper)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=errors, env=env)
        reader = concurrent.futures.ThreadPoolExecutor(max_workers=1)
        try:
            assert process.stdin is not None and process.stdout is not None
            exchanges = (
                ({"type": "hello", "protocol": 1, "buildCommit": commit}, {"type": "ready"}),
                ({"type": "initializeRuntime"}, {"type": "runtimeReady"}),
                ({"type": "close"}, {"type": "closed"}),
            )
            for request, expected in exchanges:
                payload = json.dumps(request).encode()
                process.stdin.write(struct.pack(">I", len(payload)) + payload)
                process.stdin.flush()
                response = reader.submit(read_frame, process.stdout).result(timeout=30)
                if response != expected:
                    raise ValueError(f"Unexpected voice runtime reply: {response!r}")
            process.stdin.close()
            if process.wait(timeout=10) != 0:
                raise ValueError("Voice helper did not shut down cleanly")
        except BaseException:
            errors.seek(0)
            print(errors.read(8192).decode(errors="replace"), file=sys.stderr)
            raise
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
            reader.shutdown(wait=True, cancel_futures=True)
            if process.stdin is not None:
                process.stdin.close()
            if process.stdout is not None:
                process.stdout.close()
