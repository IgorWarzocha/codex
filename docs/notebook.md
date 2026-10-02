# Deno Notebook runtime

This fork adds an opt-in persistent JavaScript and TypeScript runtime to Codex's existing `exec` and `wait` tools. Rust controls Deno's Jupyter kernel through `jupyter-zmq-client`. No Python, JupyterLab or Node controller is required.

## Run

Build the fork's CLI from `codex-rs`. On Linux or macOS:

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

Notebook uses Deno from `PATH` when available. Otherwise, its first authorized startup downloads the pinned Deno 2.9.7 release from `denoland/deno` into `$CODEX_HOME/notebook/runtime`. The download and extracted executable must match the bundled SHA-256 hashes and sizes before installation. The cached runtime is reused without another download. Nothing is installed globally. Linux, macOS and Windows assets are provided for x86-64 and ARM64.

For persistent configuration:

```toml
[features.code_mode]
runtime = "notebook"
# Optional explicit executable. A missing override fails without downloading.
deno_program = "/absolute/path/to/deno"
# Optional saved profile, applied after restoring saved bindings.
notebook_profile = "daily"
# Heap limit in MiB. Default 4096, allowed range 256 through 65536.
notebook_max_heap_mib = 4096
# Print command output below its metadata. Default false uses JSON.
notebook_plain_command_output = true
```

Full-access permissions must still be selected separately. Omit `runtime` or set it to `"v8"` to retain upstream behavior.

Omit `deno_program` to allow automatic installation. Set it to `"deno"` to require an existing executable on `PATH`, including for offline environments. Network and verification failures are reported rather than falling back to an unverified runtime. Ephemeral threads can share the managed runtime cache without persisting Notebook state.

Keep the same Cargo profile and build flags between runs to reuse compiled dependencies. Incremental compilation is disabled for development and test builds to limit disk usage. Use `CARGO_BUILD_JOBS` to tune concurrency for your machine. Building tests for a previously unbuilt dependency feature set can compile additional artifacts even when the CLI is already built.

The workspace has a distinct prerelease version. Keep a real version rather than upstream's `0.0.0` development placeholder: Codex sends its compiled version during model discovery and requests. In live validation, `0.0.0` rejected GPT-6 Luna while the versioned fork accepted it with the same account.

## Retained state

- A separate persistent Deno kernel belongs to each running Codex thread.
- JavaScript and TypeScript bindings survive cells and live context rollover. Resuming the same thread restores its last saved serializable state.
- New threads restore durable project bindings, including explicitly pinned helpers. Existing live threads remain private and do not receive another thread's changes.
- Checkpoints and named profiles restore values and function definitions. They never replay cells. Functions must be self-contained or use retained global dependencies. Open handles, imports, promises and other unsupported values are reported as skipped.
- Nested tools use Codex's existing dispatcher and tool approvals.
- Cells can yield and continue through `wait`. Only one cell runs per kernel at a time.
- Ordinary JavaScript errors retain the kernel but do not checkpoint the failed cell. Cancellation invalidates live state, and the next execution restores the last completed checkpoint. Terminal kernel failures trigger a bounded recovery attempt. Recovery reports its result and never replays failed work or reverses external side effects. Manual restart and reset remain available if recovery fails.

The agent's `notebook` tool manages status, checkpoints, profiles and pins. Ask it to inspect retained bindings, pin a reusable helper, save a named profile, or prune unpinned temporary state. Pinned functions can run after startup or nested tool results. These hooks run with the same full host access as notebook cells.

To choose a default profile, first save one with `notebook`'s `save` action, then set `features.code_mode.notebook_profile` to its name. Startup and restart attempt the profile after restoring project and private bindings, before startup hooks. Any name collision leaves the entire profile unapplied and preserves restored bindings. Status reports whether the profile loaded. Unavailable profiles produce a notice, while failures applying a profile abort startup. Profile bindings remain thread-private unless pinned. Reset restores durable project state without applying the profile. Ephemeral threads may read a profile without writing state. Explicit `notebook load` also rejects collisions.

`text(await tools.exec_command(...))` and `text(await tools.write_stdin(...))` omit routine timing and chunk metadata. Set `notebook_plain_command_output = true` to show command output on separate lines instead of inside JSON. Exit codes, running session IDs and truncation information remain visible. The returned JavaScript object is unchanged, so code can still inspect every field.

Normal shutdown checkpoints an idle notebook and runs `Symbol.dispose` and `Symbol.asyncDispose` cleanup with a bounded wait. An interrupted or unavailable kernel is terminated without running cleanup. Permission revocation also skips user cleanup code.

Historical cells are saved as bounded `.ipynb` journals. Diagnostics checks that history with Deno's language server, separately from the current kernel's health. Journals are not a recovery script.

Startup context and notebook status list exact-version npm imports found in successful project cells. The agent must ask before using an unlisted package. This inventory is guidance, not a package sandbox or proof of prior approval. Imports are not restored as live modules. Recreate them explicitly or in a pinned startup helper.

State lives under `$CODEX_HOME/notebook`, outside the working tree. These private files contain code and serialized values, not encrypted data. Project state is shared by directories within the same Git repository. Session checkpoints remain thread-private. `--ephemeral` keeps checkpoints in memory and disables profile writes and journals.

## Remote notes and history

Codex backend authentication enables remote notes and history independently of token-budget settings. Agents can call them through `exec`, including independent calls in `Promise.all`. Await dependent writes to the same note path.

The default `context_strategy = "notes"` uses this remote storage for continuity
across context resets. Sessions without supported backend authentication must
explicitly select `context_strategy = "compaction"`. See [configuration](config.md#context-continuity).

These are not local files or notebook bindings. Encrypted results are delivered directly to the model. JavaScript receives `{ delivered_to_model: true, call_id }`, not decrypted contents. The call ID matches the model's result, so parallel receipts can be associated with their requests. API-key and other-provider sessions do not expose this backend capability.

## Boundaries

This is a native Codex implementation of the Pi Notebook workflow, not a Pi extension host. Pi custom extensions and ChatGPT desktop plugin packaging are not included. Native notebook code has full host access even though nested Codex tools retain their own approval checks.

On Unix, the controller owns the kernel's process group. On Windows, it assigns the suspended kernel to a non-breakaway Job Object before allowing it to run. Failure to establish ownership rejects startup. Kernel shutdown terminates owned descendants. The Windows path has not yet been exercised on a Windows machine.

Retained functions do not preserve lexical closures. Recreate live connections and imported dependencies in a pinned startup function. A failed startup hook blocks execution until the hook is repaired or unpinned. When the kernel has not started, unpin and reset operate on saved state without running startup hooks. Profile loading rejects name collisions instead of overwriting live bindings.

Checkpoint and journal budgets follow the configured kernel heap: one eighth of the heap, clamped between 8 MiB and 256 MiB. The default 4096 MiB heap gives a 256 MiB persistence budget. Values that cannot fit are reported as skipped. Releasing lexical bindings may require rebuilding the kernel from retained values, so runtime-only handles must be recreated.

## Implementation

`codex-rs/notebook-kernel` owns the Deno process and Jupyter protocol. `codex-rs/notebook` implements Codex's `CodeModeSessionProvider`, output collection and authenticated local tool bridge. Core selects that provider and supplies the Notebook tool contract.

Local Rust and real-Deno tests do not call a model. Live model validation uses `gpt-6-luna` with `model_reasoning_effort="low"`.
