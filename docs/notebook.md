# Deno Notebook runtime

This fork adds an opt-in persistent JavaScript and TypeScript runtime to Codex's existing `exec` and `wait` tools. Rust controls Deno's Jupyter kernel through `jupyter-zmq-client`. No Python, JupyterLab or Node controller is required.

## Run

Install Deno and build the fork's CLI from `codex-rs`. On Linux or macOS:

```sh
CARGO_PROFILE_DEV_DEBUG=0 cargo build -p codex-cli --bin codex
./target/debug/codex \
  -c 'features.code_mode.runtime="notebook"' \
  -m gpt-6-luna -c 'model_reasoning_effort="low"' \
  --sandbox danger-full-access
```

On Windows, use PowerShell:

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = "0"
cargo build -p codex-cli --bin codex
.\target\debug\codex.exe -c 'features.code_mode.runtime="notebook"' -m gpt-6-luna -c 'model_reasoning_effort="low"' --sandbox danger-full-access
```

Use this only in a trusted local working directory. Deno Jupyter always has full filesystem, network and subprocess access. Notebook refuses restricted permissions, managed network proxies and remote or multiple execution environments. The fork does not change your global Codex configuration.

For persistent configuration:

```toml
[features.code_mode]
runtime = "notebook"
# Optional executable override. Otherwise use deno from PATH.
deno_program = "/absolute/path/to/deno"
```

Full-access permissions must still be selected separately. Omit `runtime` or set it to `"v8"` to retain upstream behavior.

Keep the same Cargo profile and build flags between runs to reuse incremental artifacts. Use `CARGO_BUILD_JOBS` to tune concurrency for your machine. Building tests for a previously unbuilt dependency feature set can compile additional artifacts even when the CLI is already built.

The workspace has a distinct prerelease version. Keep a real version rather than upstream's `0.0.0` development placeholder: Codex sends its compiled version during model discovery and requests. In live validation, `0.0.0` rejected GPT-6 Luna while the versioned fork accepted it with the same account.

## Retained state

- A separate persistent Deno kernel belongs to each running Codex thread.
- JavaScript and TypeScript bindings survive cells and live context rollover. Resuming the same thread restores its last saved serializable state.
- New threads restore durable project bindings, including explicitly pinned helpers. Existing live threads remain private and do not receive another thread's changes.
- Checkpoints and named profiles restore values and function definitions. They never replay cells. Functions must be self-contained or use retained global dependencies. Open handles, imports, promises and other unsupported values are reported as skipped.
- Nested tools use Codex's existing dispatcher and tool approvals.
- Cells can yield and continue through `wait`. Only one cell runs per kernel at a time.
- Ordinary JavaScript errors retain the kernel. Cancellation and terminal kernel failures invalidate its in-memory state. Ask the agent to restart the notebook from its checkpoint, or reset it to durable project state. Neither operation replays failed work or reverses external side effects.

The agent's `notebook` tool manages status, checkpoints, profiles and pins. Ask it to inspect retained bindings, pin a reusable helper, save a named profile, or prune unpinned temporary state. Pinned functions can run after startup or nested tool results. These hooks run with the same full host access as notebook cells.

Historical cells are saved as bounded `.ipynb` journals. Diagnostics checks that history with Deno's language server, separately from the current kernel's health. Journals are not a recovery script.

Startup context and notebook status list exact-version npm imports found in successful project cells. The agent must ask before using an unlisted package. This inventory is guidance, not a package sandbox or proof of prior approval. Imports are not restored as live modules. Recreate them explicitly or in a pinned startup helper.

State lives under `$CODEX_HOME/notebook`, outside the working tree. These private files contain code and serialized values, not encrypted data. Project state is shared by directories within the same Git repository. Session checkpoints remain thread-private. `--ephemeral` keeps checkpoints in memory and disables disk profiles and journals.

## Boundaries

This is a native Codex implementation of the Pi Notebook workflow, not a Pi extension host. Pi custom extensions and ChatGPT desktop plugin packaging are not included. Native notebook code has full host access even though nested Codex tools retain their own approval checks.

On Unix, the controller owns the kernel's process group. On Windows, it assigns the suspended kernel to a non-breakaway Job Object before allowing it to run. Failure to establish ownership rejects startup. Kernel shutdown terminates owned descendants. The Windows path has not yet been exercised on a Windows machine.

Retained functions do not preserve lexical closures. Recreate live connections and imported dependencies in a pinned startup function. A failed startup hook blocks execution until the hook is repaired or unpinned. Profile loading rejects name collisions instead of overwriting live bindings.

Checkpoint and journal budgets follow the configured kernel heap: one eighth of the heap, clamped between 8 MiB and 256 MiB. The default 512 MiB heap gives a 64 MiB persistence budget. Values that cannot fit are reported as skipped. Releasing lexical bindings may require rebuilding the kernel from retained values, so runtime-only handles must be recreated.

## Implementation

`codex-rs/notebook-kernel` owns the Deno process and Jupyter protocol. `codex-rs/notebook` implements Codex's `CodeModeSessionProvider`, output collection and authenticated local tool bridge. Core selects that provider and supplies the Notebook tool contract.

Local Rust and real-Deno tests do not call a model. Live model validation uses `gpt-6-luna` with `model_reasoning_effort="low"`.
