#!/usr/bin/env python3
"""Reapply desktop personality after a package update, without failing the update."""

import argparse
import os
import pwd
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

from asar import Asar, UnsupportedBundle
from patch import replacements, validate_prompt_path, verify_bundle


def validate_personality(user: str, personality: Path) -> Path:
    account = pwd.getpwnam(user)
    if account.pw_uid == 0:
        raise ValueError("select the desktop user, not root")
    personality = validate_prompt_path(personality)
    protected = Path(account.pw_dir) / ".pi/agent/REALTIME-SYSTEM-PROMPT.md"
    if personality.resolve() == protected.resolve():
        raise ValueError("the Pi-owned realtime prompt is outside this hook's scope")
    if not personality.is_file():
        raise ValueError(f"personality file is missing or not regular: {personality}")
    return personality


def reapply(archive: Path, personality: Path) -> str:
    metadata = archive.lstat()
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError("the installed ASAR must be a regular file, not a symlink")
    bundle = Asar(archive)
    version, layout = verify_bundle(bundle)
    # Staging shares the target filesystem. No backup or unpacked file is changed.
    with tempfile.TemporaryDirectory(
        prefix=".codex-personality-", dir=archive.parent
    ) as directory:
        output = Path(directory) / "app.asar"
        bundle.write(output, replacements(bundle, layout, personality))
        os.chown(output, metadata.st_uid, metadata.st_gid)
        os.chmod(output, stat.S_IMODE(metadata.st_mode))
        # Refuse a concurrently replaced source rather than install a stale bundle.
        bundle.read(layout.main)
        if archive.is_symlink():
            raise ValueError("the installed ASAR became a symlink during patching")
        os.replace(output, archive)
    return version


def report(user: str, message: str, *, warning: bool) -> None:
    print(f"Codex personality: {message}", file=sys.stderr if warning else sys.stdout)
    try:
        result = subprocess.run(
            [
                "/usr/bin/logger",
                "--tag",
                "codex-desktop-personality",
                "--priority",
                "user.warning" if warning else "user.notice",
                "--",
                message,
            ],
            capture_output=True,
            timeout=5,
            check=False,
        )
        if result.returncode:
            print("Codex personality: journal logging unavailable", file=sys.stderr)
    except (OSError, subprocess.TimeoutExpired):
        print("Codex personality: journal logging unavailable", file=sys.stderr)
    try:
        account = pwd.getpwnam(user)
        runtime = Path(f"/run/user/{account.pw_uid}")
        if not (runtime / "bus").is_socket():
            print("Codex personality: no desktop session to notify", file=sys.stderr)
            return
        result = subprocess.run(
            [
                "/usr/bin/runuser",
                "--user",
                user,
                "--",
                "/usr/bin/env",
                "-i",
                f"HOME={account.pw_dir}",
                f"XDG_RUNTIME_DIR={runtime}",
                f"DBUS_SESSION_BUS_ADDRESS=unix:path={runtime / 'bus'}",
                "/usr/bin/notify-send",
                "--app-name=Codex personality",
                "--urgency=critical" if warning else "--urgency=normal",
                "--expire-time=0" if warning else "--expire-time=10000",
                "Codex personality needs attention"
                if warning
                else "Codex personality reapplied",
                message,
            ],
            capture_output=True,
            timeout=5,
            check=False,
        )
        if result.returncode:
            print(
                "Codex personality: desktop notification unavailable", file=sys.stderr
            )
    except (KeyError, OSError, subprocess.TimeoutExpired):
        print("Codex personality: desktop notification unavailable", file=sys.stderr)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--user", required=True)
    parser.add_argument("--personality-file", required=True, type=Path)
    parser.add_argument(
        "--asar", type=Path, default=Path("/usr/lib/chatgpt/resources/app.asar")
    )
    args = parser.parse_args(argv)
    try:
        personality = validate_personality(args.user, args.personality_file)
        version = reapply(args.asar, personality)
    except UnsupportedBundle as error:
        report(
            args.user,
            f"Incompatible app: {error}. The app update completed; its archive was left unchanged. "
            "Personality was not reapplied. Update the patcher before retrying.",
            warning=True,
        )
    except Exception as error:
        # This is deliberately a fail-open package-manager boundary, not a fallback
        # inside the patcher. Preserve diagnostics while letting the update finish.
        report(
            args.user,
            f"Could not reapply personality: {error}. The app update completed. "
            "Check journalctl -t codex-desktop-personality before retrying.",
            warning=True,
        )
    else:
        report(
            args.user,
            f"Reapplied text and voice preferences to Codex {version}. Restart the app to use them.",
            warning=False,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
