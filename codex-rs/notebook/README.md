# Native Deno notebook sessions

`codex-notebook` implements Codex's `CodeModeSessionProvider` with one Deno Jupyter kernel per session. JavaScript top-level bindings and JSON values stored with `store` persist between cells. Separate sessions have separate kernels.

Deno Jupyter runs with unrestricted filesystem, network, and subprocess access. This provider is not a sandbox. The integrating caller must require explicit `danger-full-access` before exposing it. The kernel must run on the same local machine as the localhost tool bridge.

Construct `DenoNotebookSessionProvider::new(deno_program, cwd)` and use the existing Code Mode `exec`, `wait`, and delegate contracts. Deno must support `deno jupyter --kernel`. The deterministic runtime tests were exercised with Deno 2.9.6.

## Cell behavior

Only one cell can run at a time. Another `exec` is rejected until the running cell completes or is terminated. Foreground timeouts, preemption, and `await yield_control()` yield an observation without stopping execution. `wait` drains only output not previously observed, including output buffered after a yield and before completion.

Ordinary JavaScript exceptions preserve the kernel. Termination, a kernel failure, or the 24-hour execution deadline invalidates the session and discards its bindings. Create a new session to recover. Shutdown cancels callbacks and shuts down the bridge and kernel with bounded waits.

Pending output is capped at 4 MiB, with a visible truncation notice. The integrating caller applies each `exec` or `wait` token budget. This provider does not carry the initial `exec` budget into later observations.

## Globals

- `tools.*` invokes the actual cell's `CodeModeSessionDelegate` over an authenticated localhost bridge. Tool functions expose `description` and supplied `input_schema` as `usage`. `ALL_TOOLS` lists normalized names and descriptions.
- `text(value)` appends text. `image(value, detail?)` accepts a `data:image` URL, an image item, or an MCP image block. `generatedImage({ image_url, output_hint? })` forwards an image and its optional hint.
- `store(key, value)` saves a JSON-serializable value. `load(key)` returns a copy, or `undefined` for a missing key.
- `await notify(value)` calls the real delegate notification hook. `await yield_control()` requests an immediate foreground yield while execution continues.
- Native Deno APIs and timers are available. Console output and Jupyter image displays are forwarded. Bare expression results are discarded.

Await all asynchronous work within its cell. Native Deno timers and IO are not cell-scoped and may outlive evaluation. Captured tool methods retain their original cell identity and cannot call a later cell's delegate. In-flight delegate calls are cancelled when their cell closes.

There are no disk checkpoints, pins, profiles, notebook management tools, or restart-in-place. Non-default protocol resource limits, `exit()`, and `audio()` are explicitly unsupported. Jupyter display updates append new items instead of replacing earlier observations. `clear_output` and unsupported display MIME types produce explicit notices.

## Validation

From `codex-rs`, run `cargo test -p codex-notebook` for in-process contract tests. To include real Deno persistence, isolation, delegate, output, and cancellation tests, run:

```sh
DENO_PROGRAM=/usr/bin/deno cargo test -p codex-notebook -- --include-ignored
```

These tests do not call a model.
