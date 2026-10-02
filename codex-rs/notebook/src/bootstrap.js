// This closure holds the bridge credential and the session store, not user bindings.
await (async (endpoint, credential) => {
  const { AsyncLocalStorage } = await import("node:async_hooks");
  const hookScope = new AsyncLocalStorage();
  const state = globalThis.__codexNotebookState;
  const values = new Map();
  const commandResults = new WeakSet();
  const plainCommandOutput = __PLAIN_COMMAND_OUTPUT__;
  const stringify = (value) => {
    if (typeof value === "string") return value;
    try { return JSON.stringify(value) ?? String(value); }
    catch { return String(value); }
  };
  const formatCommandOutput = (value) => {
    const metadata = Object.fromEntries(Object.entries(value).filter(([key]) =>
      key !== "output" && key !== "chunk_id" && key !== "wall_time_seconds" &&
      (key !== "original_token_count" || value.truncated)
    ));
    return plainCommandOutput
      ? stringify(metadata) + "\nOutput:\n" + value.output
      : stringify({ output: value.output, ...metadata });
  };
  const unsupported = (name) => {
    throw new Error(name + " is unsupported by native Deno notebook sessions");
  };
  let current;
  const hookFailure = (context, name, error) => {
    if (current !== context || !context.cellId) return;
    globalThis.text(
      "Notebook tool_result hook " + name + " failed: " +
        String(error instanceof Error ? error.message : error).slice(0, 1000) + "; unpin or set hook:false to disable",
    );
  };
  const dispatchToolResult = async (context, event) => {
    for (const name of state?.hooks("tool_result") ?? []) {
      if (current !== context) return;
      try {
        await hookScope.run(context, () => state.read(name)(structuredClone(event)));
      } catch (error) {
        hookFailure(context, name, error);
      }
    }
  };
  const runStartupHooks = async () => {
    for (const name of state?.hooks("startup") ?? []) {
      try {
        await hookScope.run(current ?? {}, () => state.read(name)({ type: "startup" }));
      } catch (error) {
        throw new Error(
          "Notebook startup hook " + name + " failed: " + String(error instanceof Error ? error.message : error) +
            ". Unpin it to recover; external side effects were not rolled back",
          { cause: error },
        );
      }
    }
  };
  const begin = (cellId, definitions) => {
    const context = {
      cellId,
      queue: Promise.resolve(),
      error: undefined,
      tools: new Set(),
      globals: new Set(Object.getOwnPropertyNames(globalThis)),
    };
    current = context;
    const rpc = async (body) => {
      if (!cellId || current !== context) {
        throw new Error("notebook cell is unknown or closed; helper called outside an active exec cell");
      }
      const response = await fetch(endpoint, {
        method: "POST",
        headers: { "Authorization": "Bearer " + credential, "Content-Type": "application/json" },
        body: JSON.stringify({ ...body, cell_id: cellId }),
      });
      const result = await response.json();
      if (!response.ok || !result.ok) throw new Error(result.error ?? "notebook bridge failed");
      return result.value;
    };
    context.cancelTools = () => rpc({ op: "cancel_tools" });
    const enqueue = (body) => {
      if (!cellId || current !== context) {
        throw new Error("notebook cell is unknown or closed; helper called outside an active exec cell");
      }
      context.queue = context.queue.then(() => rpc(body)).catch((error) => {
        context.error ??= error;
      });
      return context.queue;
    };
    const output = (item) => enqueue({ op: "output", item });
    const tools = Object.create(null);
    for (const definition of definitions) {
      // Use the original identity, not the exposed alias or a result-shape guess.
      const commandTool = [null, undefined, "", "functions"].includes(definition.tool_name.namespace) &&
        ["exec_command", "write_stdin"].includes(definition.tool_name.name);
      const method = (input) => {
        const scope = hookScope.getStore();
        if (scope && scope !== context) throw new Error("Notebook hook tool called outside its originating exec cell");
        let observe = !scope && (state?.hooks("tool_result").length ?? 0) > 0;
        let eventInput;
        if (observe) {
          try {
            eventInput = structuredClone(input);
          } catch (error) {
            observe = false;
            hookFailure(context, "input snapshot for " + definition.name, error);
          }
        }
        const event = { type: "tool_result", toolName: definition.name, input: eventInput };
        const promise = rpc({ op: "tool", name: definition.name, input }).then(
          async (result) => {
            if (commandTool && result !== null && typeof result === "object") commandResults.add(result);
            if (observe) await dispatchToolResult(context, { ...event, status: "success", result });
            return result;
          },
          async (error) => {
            if (observe) {
              await dispatchToolResult(context, {
                ...event,
                status: "error",
                error: String(error instanceof Error ? error.message : error),
              });
            }
            throw error;
          },
        );
        context.tools.add(promise);
        void promise.then(() => context.tools.delete(promise), () => context.tools.delete(promise));
        return promise;
      };
      Object.defineProperties(method, {
        description: { value: definition.description },
        usage: { value: definition.input_schema ?? "No input schema supplied" },
      });
      tools[definition.name] = method;
    }
    Object.assign(globalThis, {
      tools: Object.freeze(tools),
      ALL_TOOLS: Object.freeze(definitions.map((definition) => Object.freeze({ ...definition }))),
      text: (value) => {
        output({
          type: "input_text",
          text: commandResults.has(value) && typeof value.output === "string" ? formatCommandOutput(value) : stringify(value),
        });
      },
      image: (value, detail) => {
        let imageUrl;
        if (typeof value === "string") imageUrl = value;
        else if (value?.type === "image" && typeof value.data === "string" && typeof value.mimeType === "string") {
          imageUrl = "data:" + value.mimeType + ";base64," + value.data;
          detail ??= value._meta?.["codex/imageDetail"];
        } else {
          imageUrl = value?.image_url;
          detail ??= value?.detail;
        }
        if (typeof imageUrl !== "string" || !imageUrl.startsWith("data:image/")) {
          throw new Error("image requires a data:image URL or an MCP image block");
        }
        if (detail != null && !["auto", "low", "high", "original"].includes(detail)) {
          throw new Error("Invalid image detail");
        }
        output({ type: "input_image", image_url: imageUrl, detail: detail ?? "high" });
      },
      generatedImage: (result) => {
        if (!result || typeof result.image_url !== "string") {
          throw new Error("generatedImage requires { image_url, output_hint? }");
        }
        globalThis.image(result.image_url);
        if (result.output_hint !== undefined) globalThis.text(result.output_hint);
      },
      store: (key, value) => {
        if (typeof key !== "string") throw new Error("store key must be a string");
        const json = JSON.stringify(value);
        if (json === undefined) throw new Error("store requires a JSON-serializable value");
        values.set(key, JSON.parse(json));
      },
      load: (key) => {
        if (typeof key !== "string") throw new Error("load key must be a string");
        return structuredClone(values.get(key));
      },
      notify: (value) => enqueue({ op: "notify", text: stringify(value) }),
      yield_control: () => enqueue({ op: "yield" }),
      exit: () => {
        const error = new Error("__CodexNotebookExit__");
        error.name = "CodexNotebookExit";
        throw error;
      },
      audio: () => unsupported("audio"),
    });
  };
  const flush = async (cellId) => {
    // A syntax error prevents begin() from running. Never flush the preceding cell.
    if (current?.cellId !== cellId) return;
    // Awaited calls and their hooks have already settled. Cancel abandoned calls
    // before joining them, otherwise an unawaited long-running tool blocks the cell.
    if (current.tools.size) await current.cancelTools();
    while (current.tools.size) await Promise.allSettled([...current.tools]);
    await current.queue;
    if (current.error) throw current.error;
  };
  const completed = (cellId) => {
    if (current?.cellId !== cellId) return;
    state?.promote(
      Object.getOwnPropertyNames(globalThis).filter((name) =>
        !current.globals.has(name) && !name.startsWith("__codex")
      ),
    );
    current = undefined;
  };
  // Establish helper names before the host takes its completion baseline.
  begin(null, []);
  Object.defineProperty(globalThis, "__codexNotebook", {
    value: Object.freeze({ begin, flush, completed, end: completed, runStartupHooks }),
  });
})(__ENDPOINT__, __CREDENTIAL__);
