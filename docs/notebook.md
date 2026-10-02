# Deno Notebook runtime

This fork adds an opt-in persistent JavaScript and TypeScript runtime to Codex's existing `exec` and `wait` tools. Rust controls Deno's Jupyter kernel through `jupyter-zmq-client`. No Python, JupyterLab or Node controller is required.

## Run

On Linux or macOS, install Deno and build the fork's CLI from `codex-rs`:

```sh
CARGO_PROFILE_DEV_DEBUG=0 cargo build -p codex-cli --bin codex
./target/debug/codex \
  -c 'features.code_mode.runtime="notebook"' \
  -m gpt-6-luna -c 'model_reasoning_effort="low"' \
  --sandbox danger-full-access
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

## Current scope

- A separate persistent Deno kernel belongs to each running Codex thread.
- JavaScript and TypeScript bindings survive cells and context rollover while the process remains alive.
- Nested tools use Codex's existing dispatcher and tool approvals.
- Cells can yield and continue through `wait`. Only one cell runs per kernel at a time.
- Ordinary JavaScript errors retain the kernel. Cancellation and terminal kernel failures invalidate its state rather than silently starting an empty replacement.

Resuming after process exit and forking a thread start fresh kernels. Disk checkpoints, project pins, profiles and Pi extension compatibility are not implemented. Native notebook code has full host access even though nested Codex tools retain their own approval checks. Windows startup is rejected until the controller can own and terminate the kernel's subprocess tree.

## Implementation

`codex-rs/notebook-kernel` owns the Deno process and Jupyter protocol. `codex-rs/notebook` implements Codex's `CodeModeSessionProvider`, output collection and authenticated local tool bridge. Core selects that provider and supplies the Notebook tool contract.

Local Rust and real-Deno tests do not call a model. Live model validation uses `gpt-6-luna` with `model_reasoning_effort="low"`.
