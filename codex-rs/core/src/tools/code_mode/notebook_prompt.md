Run JavaScript or TypeScript in a persistent Deno notebook. Send raw source, not JSON or Markdown fences.

- Bindings and imports survive cells and context rollover. Checkpoints restore retained state by value, never by replaying cells. New threads inherit durable project state, not another thread's private live bindings.
- Retained functions restore from source. Keep helpers self-contained or use retained globals. Closures and live handles are not durable.
- Deno APIs, Web APIs and imports are available. Code runs with full machine access. Never treat this runtime as a sandbox.
- Await all async work. Deno timers and direct I/O are not cell-scoped; do not leave them running across cells. Only one cell may run at a time. Use `wait` after a yielded cell before starting another.
- Cancellation terminates the kernel. Use the top-level `notebook` tool to inspect or recover retained state.
- Optional first line: `// @exec: {"yield_time_ms": 10000, "max_output_tokens": 1000}`. Default yield timeout: {{ default_exec_yield_time_ms }} ms.
- `tools` contains the enabled nested tools. Example: `text(await tools.exec_command({cmd: "pwd"}))`. Tool names use normalized JavaScript identifiers. `ALL_TOOLS` contains their names and descriptions.
- `text(value)` emits text. `image(value)` emits an image. `generatedImage(value)` emits an image-generation result. Bare expression values are not a substitute for explicit output.
- `store(key, value)` and `load(key)` retain serializable values within this kernel.
- Inside exec, `tools.notebook` supports status without query, list, and diagnostics. All other notebook actions use the top-level tool after exec returns.
- `await notify(value)` sends an immediate notification. `await yield_control()` yields output while execution continues.
- `exit()` ends the cell successfully. `audio()` is not supported.
