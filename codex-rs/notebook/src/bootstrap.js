// This closure holds the bridge credential and the session store, not user bindings.
((endpoint, credential) => {
  const values = new Map();
  const stringify = (value) => typeof value === "string" ? value : JSON.stringify(value) ?? String(value);
  const unsupported = (name) => { throw new Error(name + " is unsupported by native Deno notebook sessions"); };
  let current;
  const begin = (cellId, definitions) => {
    const context = { cellId, queue: Promise.resolve(), error: undefined };
    current = context;
    const rpc = async (body) => {
      const response = await fetch(endpoint, {
        method: "POST",
        headers: { "Authorization": "Bearer " + credential, "Content-Type": "application/json" },
        body: JSON.stringify({ ...body, cell_id: cellId }),
      });
      const result = await response.json();
      if (!response.ok || !result.ok) throw new Error(result.error ?? "notebook bridge failed");
      return result.value;
    };
    const enqueue = (body) => {
      context.queue = context.queue.then(() => rpc(body)).catch((error) => { context.error ??= error; });
      return context.queue;
    };
    const output = (item) => enqueue({ op: "output", item });
    const tools = Object.create(null);
    for (const definition of definitions) {
      const method = (input) => rpc({ op: "tool", name: definition.name, input });
      Object.defineProperties(method, {
        description: { value: definition.description },
        usage: { value: definition.input_schema ?? "No input schema supplied" },
      });
      tools[definition.name] = method;
    }
    Object.assign(globalThis, {
      tools: Object.freeze(tools),
      ALL_TOOLS: Object.freeze(definitions.map(({ name, description }) => Object.freeze({ name, description }))),
      text: (value) => { output({ type: "input_text", text: stringify(value) }); },
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
        if (detail != null && !["auto", "low", "high", "original"].includes(detail)) throw new Error("Invalid image detail");
        output({ type: "input_image", image_url: imageUrl, detail: detail ?? "high" });
      },
      generatedImage: (result) => {
        if (!result || typeof result.image_url !== "string") throw new Error("generatedImage requires { image_url, output_hint? }");
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
      exit: () => unsupported("exit"),
      audio: () => unsupported("audio"),
    });
  };
  const flush = async (cellId) => {
    // A syntax error prevents begin() from running. Never flush the preceding cell.
    if (current?.cellId !== cellId) return;
    await current.queue;
    if (current.error) throw current.error;
  };
  Object.defineProperty(globalThis, "__codexNotebook", { value: Object.freeze({ begin, flush }) });
})(__ENDPOINT__, __CREDENTIAL__);
