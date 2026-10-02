Run JavaScript/TypeScript in a persistent Deno notebook. Source only, no JSON or fences. Deno, Web APIs and imports have full machine access, not a sandbox.
Bindings/imports survive cells and context rollover. Checkpoints restore values without replay; functions restore from source, not closures or live handles. New threads inherit durable project state, not private live bindings.
Await async work. Timers and direct I/O can outlive cells. Only one cell runs at a time: wait on yielded cells before another exec. Cancellation terminates the kernel; the next exec restores its checkpoint without replaying cells.
Optional // @exec: {"yield_time_ms": 10000, "max_output_tokens": 1000}; defaults {{ default_exec_yield_time_ms }} ms/10000 tokens.
Call await tools.<normalized_name>(args). Help: tools.<name>.description and .usage, or filter ALL_TOOLS by name.
Model-only tool results bypass JS; calls return delivery receipts.
text(value), image(value), generatedImage(value) emit output; bare values are discarded. store(key,value)/load(key) retain serializable values in this kernel. await notify(value) emits immediately; await yield_control() yields while work continues; exit() finishes successfully. No audio().
Inside exec, tools.notebook({action}) supports status without query, list, diagnostics. Other notebook actions use the top-level tool after exec returns.
