# Desktop voice prompt patcher

An opt-in patch for **Codex desktop 26.930.21537**. Python 3.10 or newer is required. No packages are installed. This is separate from the fork's native CLI voice configuration.

The script writes a **new ASAR**. It never installs the patch, overwrites an existing output, or changes its input. It supports only the verified pristine bundle with SHA256 `529af3396e94c0e20b8a62b1d5862a565e89e1af2407b590e9b5ab1f1017af0d`. Different versions and modified bundles are rejected. The `/usr/lib/chatgpt` directory name does not identify the app: the archive's `package.json` identifies it as Codex.

## Prepare a patched archive

Run these commands on the machine with that app version, from this repository:

```sh
python3 scripts/desktop-voice-prompt/patch.py \
  --asar /usr/lib/chatgpt/resources/app.asar --check
```

Create your own UTF-8 prompt at an absolute path, for example `~/.config/codex/desktop-voice.md`. The script does not create or read that file. The app reads it at runtime. Do not use the Pi-owned `~/.pi/agent/REALTIME-SYSTEM-PROMPT.md`, which is deliberately excluded.

```sh
python3 scripts/desktop-voice-prompt/patch.py \
  --asar /usr/lib/chatgpt/resources/app.asar \
  --prompt-file "$HOME/.config/codex/desktop-voice.md" \
  --output "$HOME/app.voice-prompt.asar"
```

The prompt path is embedded, not the prompt contents. If preparing an archive for another machine, supply that machine's absolute prompt path. The runtime file must be nonempty, valid UTF-8, and at most 64 KiB. Missing, unreadable, invalid, or oversized files cause an error instead of silently using the default prompt.

## Installation and removal

Installation is a separate, deliberate action. Fully quit the desktop app first. Preserve its pristine `app.asar` outside the installation directory for removal. Replace only `resources/app.asar` with the generated archive, using the installation's normal owner and mode, usually `root:root` and `0644` on Linux. Keep the matching `app.asar.unpacked` directory unchanged. The script does not need root.

Restart the app after installing. Edit your prompt, then stop and restart voice to use the new text. The file is reread during native parameter preparation, including prewarm. An existing call is not hot-updated. The native prewarm cache key includes the override, so changed text does not select a call prepared with the old prompt.

To remove the patch, quit the app and restore the pristine archive or reinstall the app package. App updates replace the patch. Do not apply this script to an already-patched archive. A new app version needs a newly verified patch point, not a bypass of the version or hash check. If the app enforces external ASAR integrity and rejects the result, restore the original. This script does not disable integrity protections or patch the executable.

## What changes

The verified renderer path is `start-voice` → the wingman session builder in `voice-session-parameters-c40a620b8266.js` → native voice status and WebRTC session requests. The parameter builder supplies `bidi_system_prompt_override`, which is normally unset. This patch fills that field only for `wingman` voice.

A narrow preload bridge asks Electron's main process to read the fixed prompt path. Only the app's top-level `app://-` frames can use the loader. The renderer cannot supply another file path. Standard and advanced voice remain unchanged.

Native request routing, authentication, SDP, WebRTC, `system_hints`, `client_tools`, and handoff handlers remain unchanged. This is a **full system-prompt override**, not an append to the server's hidden default. Your instructions can influence whether the model uses available tools. Include the delegation behavior you want in your own prompt. No private Pi prompt or hidden server prompt is copied.

Validation covers archive preservation, runtime file loading, failure handling, and the actual bundle's parameter builder. Live backend acceptance, spoken behavior, and tool execution have not been tested. No installed app was patched during development.

## Tests

Python 3.10 or newer and Node.js 22 or newer are required for the tests. They use temporary files, without network access or an installed app.

```sh
python3 -m unittest discover -s scripts/desktop-voice-prompt -p 'test_*.py' -v
```
