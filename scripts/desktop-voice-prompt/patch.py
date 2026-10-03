#!/usr/bin/env python3
"""Opt-in Codex desktop voice prompt patcher. Never installs or edits its input."""

import argparse
import hashlib
import json
import sys
from pathlib import Path

from asar import Asar, UnsupportedBundle


SUPPORTED_SHA256 = "529af3396e94c0e20b8a62b1d5862a565e89e1af2407b590e9b5ab1f1017af0d"
VERSION = "26.930.21537"
MAIN = ".vite/build/main-C3nRcJ3D.js"
PRELOAD = ".vite/build/preload.js"
PARAMETERS = "webview/assets/voice-session-parameters-c40a620b8266.js"
ANCHOR = (
    "bidi_system_prompt_override:w,structured_system_prompt_personality_override:void 0"
)
REPLACEMENT = (
    "bidi_system_prompt_override:b===`wingman`?globalThis.codexUserVoicePrompt.read():w,"
    "structured_system_prompt_personality_override:void 0"
)
MARKER = b"codex-user-voice-prompt-v1"
HERE = Path(__file__).resolve().parent


def digest(path: Path) -> str:
    with path.open("rb") as source:
        result = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def verify_bundle(bundle: Asar) -> None:
    try:
        package = json.loads(bundle.read("package.json"))
    except (ValueError, UnicodeError) as error:
        raise UnsupportedBundle("invalid package.json") from error
    if not isinstance(package, dict) or (
        package.get("name"),
        package.get("productName"),
        package.get("version"),
    ) != ("openai-codex-electron", "Codex", VERSION):
        raise UnsupportedBundle(f"only Codex desktop {VERSION} is supported")
    if digest(bundle.path) != SUPPORTED_SHA256:
        raise UnsupportedBundle("ASAR SHA256 is not the verified pristine bundle")
    main, preload, parameters = (
        bundle.read(name) for name in (MAIN, PRELOAD, PARAMETERS)
    )
    if any(MARKER in source for source in (main, preload, parameters)):
        raise UnsupportedBundle("bundle is already patched")
    if not main.startswith(b"const e=require("):
        raise UnsupportedBundle("unsupported main-process module")
    if b"contextBridge.exposeInMainWorld(`electronBridge`,z)" not in preload:
        raise UnsupportedBundle("unsupported sandboxed preload bridge")
    if parameters.count(ANCHOR.encode()) != 1 or (
        b"function mi(e,{ambientParameters:t,explicitParameters:n,liveParameters:r,systemHints:i})"
        not in parameters
    ):
        raise UnsupportedBundle("unsupported native voice parameter builder")


def validate_prompt_path(path: Path) -> Path:
    if not path.is_absolute():
        raise ValueError("--prompt-file must be an absolute path on the app's machine")
    # This task explicitly excludes Igor's Pi-owned prompt, including symlink aliases.
    protected = Path.home() / ".pi/agent/REALTIME-SYSTEM-PROMPT.md"
    if path.resolve() == protected.resolve():
        raise ValueError(
            "the Pi-owned REALTIME-SYSTEM-PROMPT.md is outside this script's scope"
        )
    return path


def replacements(bundle: Asar, prompt: Path) -> dict[str, bytes]:
    main = (
        (HERE / "runtime-main.js")
        .read_text()
        .replace(
            "__CODEX_VOICE_PROMPT_PATH__", json.dumps(str(prompt), ensure_ascii=True)
        )
    )
    preload = (HERE / "runtime-preload.js").read_bytes()
    parameters = bundle.read(PARAMETERS)
    if parameters.count(ANCHOR.encode()) != 1:
        raise UnsupportedBundle("voice prompt anchor must occur exactly once")
    return {
        MAIN: main.encode() + bundle.read(MAIN),
        PRELOAD: preload + bundle.read(PRELOAD),
        PARAMETERS: parameters.replace(ANCHOR.encode(), REPLACEMENT.encode()),
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--asar", required=True, type=Path, help="pristine app.asar to read"
    )
    parser.add_argument(
        "--check", action="store_true", help="verify support without writing"
    )
    parser.add_argument(
        "--prompt-file", type=Path, help="absolute user-owned prompt path"
    )
    parser.add_argument("--output", type=Path, help="new ASAR path, never overwritten")
    args = parser.parse_args(argv)
    if args.check and (args.prompt_file or args.output):
        parser.error("--check cannot be combined with --prompt-file or --output")
    if not args.check and (args.prompt_file is None or args.output is None):
        parser.error("patching requires --prompt-file and --output")
    try:
        bundle = Asar(args.asar)
        verify_bundle(bundle)
        if args.check:
            print(f"Supported: Codex {VERSION}, native wingman voice parameter builder")
            return 0
        prompt = validate_prompt_path(args.prompt_file)
        if args.output.resolve() == args.asar.resolve():
            raise ValueError(
                "output must differ from input; in-place patching is not supported"
            )
        bundle.write(args.output, replacements(bundle, prompt))
        print(f"Created {args.output}. Input unchanged. Not installed.")
        print(f"Voice reads {prompt} on parameter preparation, including prewarm.")
        print("Keep the matching app.asar.unpacked directory when installing manually.")
        return 0
    except (OSError, ValueError, RecursionError) as error:
        print(f"Cannot patch: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
