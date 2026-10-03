# Desktop personality patcher

An opt-in ASAR patch that appends your communication preferences to **Codex desktop's native text and realtime voice instructions**. It preserves Codex's own prompts and tool handoffs. Python 3.10 or newer is required, without additional packages.

The script writes separate artifacts. It never installs them, changes its inputs, or overwrites existing outputs. Compatibility is checked against the specific native instruction code it edits, not the app version or whole-archive hash. Unrelated code changes and renamed bundle chunks are allowed. Unfamiliar instruction code, ambiguous owners, and already-patched archives are rejected.

## Prepare a patched archive

From this repository, check the installed archive:

```sh
python3 scripts/desktop-voice-prompt/patch.py \
  --asar /usr/lib/chatgpt/resources/app.asar --check
```

Create your own UTF-8 Markdown file at `$CODEX_HOME/codex_personality.md`, normally `~/.codex/codex_personality.md`. Write communication preferences, not copies of Codex's tool schemas or native instructions. The script never creates or reads your personality file. The app reads it at runtime.

```sh
python3 scripts/desktop-voice-prompt/patch.py \
  --asar /usr/lib/chatgpt/resources/app.asar \
  --output "$HOME/app.personality.asar"
```

To select another absolute path on the app's machine, add `--personality-file "$HOME/path/to/codex_personality.md"`. Only that path is embedded, not the contents. The desktop patch does not read `personality_file` from `config.toml`. Use the explicit flag if you want a non-default file.

A missing default file or an empty file adds nothing. An existing unreadable file, invalid UTF-8, or content over 64 KiB causes an error rather than silently ignoring preferences. An explicitly selected file must exist. The Pi-owned `~/.pi/agent/REALTIME-SYSTEM-PROMPT.md` and resolved aliases are deliberately excluded.

## macOS

Use the archive inside the app bundle. The current official DMG names the app ChatGPT.app, despite its Codex bundle identity. Supply its matching `Info.plist` so the script can generate updated ASAR integrity metadata:

```sh
python3 scripts/desktop-voice-prompt/patch.py \
  --asar "/Applications/ChatGPT.app/Contents/Resources/app.asar" \
  --info-plist "/Applications/ChatGPT.app/Contents/Info.plist" \
  --output "$HOME/app.personality.asar" \
  --output-info-plist "$HOME/Info.personality.plist"
```

The original plist must match the original ASAR. Only its `Resources/app.asar` integrity hash changes. No Electron integrity protection is disabled. Modifying a signed app invalidates its signature. Generating these artifacts does **not** re-sign the app. macOS installation requires a separate signing step on a Mac. A patched Mac app has not been launched during validation.

## Installation and removal

Installation is a separate, deliberate action. Fully quit the desktop app first. Preserve the unmodified app for removal, or be ready to reinstall its package. Install the generated archive with the installation's normal owner and mode, usually `root:root` and `0644` on Linux. Keep the matching `app.asar.unpacked` directory unchanged. On macOS, the generated plist and a valid signature are also required. This script does not need root.

Restart the app after installation. Start a new text conversation to get newly composed instructions. Stop and restart voice to pick up changes. The file is reread when desktop text developer instructions are composed and whenever a native realtime call is created, including replacement calls. Existing text instructions and ongoing voice calls are not hot-updated.

To remove the patch, quit the app and restore the unmodified app or reinstall its package. Updates replace the patch. Run `--check` against the new unpatched archive before generating another patch. If an integrity or signing check rejects the result, restore the original rather than disabling that protection.

## What changes

The main process appends a `<user_communication_preferences>` block after the result of `getProjectAwareDeveloperInstructions`. The block identifies the text as the user's preferred communication style, subordinate to native instructions, safety requirements, and tool handoffs.

For voice, a narrow preload bridge supplies the same block to the native `thread/realtime/start` prompt and the client-owned `/wham/realtime/calls` instructions. Both keep the native prompt as an unchanged prefix. Existing-call attachment keeps its native handoff request unchanged. No bundled consumer ChatGPT wingman override is modified.

The renderer can request only the configured file. Access is restricted to the app's top-level `app://-` frames. Native routing, authentication, SDP, WebRTC, tools, delegation, and handoff fields remain unchanged. No native prompt or tool schema is copied into this repository.

Offline validation covered Linux 26.930.21537 and 26.930.31730, plus the official macOS Apple Silicon DMG 26.930.31730. It compared all untouched archive entries and executed the actual native instruction owners before and after transformation with controlled boundary inputs. Live app launch, backend acceptance, spoken behavior, and tool execution remain untested. No installed app was patched.

## Tests

Python 3.10 or newer and Node.js 22 or newer are required. Tests use temporary files, without network access or an installed app.

```sh
python3 -m unittest discover -s scripts/desktop-voice-prompt -p 'test_*.py' -v
```
