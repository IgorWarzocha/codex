// Injected once into the Deno isolate. The host owns discovery and disk transactions.
// Names come from Jupyter completion minus the post-bootstrap baseline, never cell parsing.
await (async () => {
  const { serialize, deserialize, getHeapStatistics } = await import("node:v8");
  const pins = new Map();
  let projectNames = new Set();
  const identifier = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
  const nameCheck = (name) => {
    if (typeof name !== "string" || !identifier.test(name) || name.startsWith("__codex")) {
      throw new TypeError("unsupported notebook binding name: " + String(name));
    }
    return name;
  };
  const read = (name) => (0, eval)(nameCheck(name));
  const reason = (error) => String(error instanceof Error ? error.message : error).slice(0, 240);
  const descriptorValue = (value, key) => {
    for (let current = value; current !== null; current = Object.getPrototypeOf(current)) {
      const descriptor = Object.getOwnPropertyDescriptor(current, key);
      if (descriptor) return descriptor.value;
    }
  };
  const metadata = (value) => {
    const result = {};
    if (value === null || !["object", "function"].includes(typeof value)) return result;
    for (const [key, maxBytes, maxLines] of [["description", 256, 1], ["usage", 512, 4]]) {
      const text = Object.getOwnPropertyDescriptor(value, key)?.value;
      if (typeof text !== "string" || !text || new TextEncoder().encode(text).length > maxBytes) continue;
      if (text.split("\n").length > maxLines || /[\x00-\x09\x0b-\x1f\x7f]/.test(text)) continue;
      result[key] = text;
    }
    return result;
  };
  const applyMetadata = (value, entry) => {
    if (value === null || !["object", "function"].includes(typeof value)) return;
    for (const [key, text] of Object.entries(metadata(entry))) {
      Object.defineProperty(value, key, { value: text, writable: true, configurable: true, enumerable: true });
    }
  };
  const inspect = (name) => {
    try {
      const value = read(name);
      let kind = "value";
      let disposable;
      if (value !== null && ["object", "function"].includes(typeof value)) {
        if (typeof descriptorValue(value, Symbol.asyncDispose) === "function") disposable = "async";
        else if (typeof descriptorValue(value, Symbol.dispose) === "function") disposable = "sync";
      }
      if (disposable || value instanceof Promise || value instanceof WeakMap || value instanceof WeakSet) {
        kind = "runtime-only";
      } else if (typeof value === "function") {
        kind = Function.prototype.toString.call(value).includes("[native code]") ? "runtime-only" : "definition";
      } else if (value && descriptorValue(value, Symbol.toStringTag) === "Module") kind = "runtime-only";
      return {
        name,
        type: typeof value,
        kind,
        globalProperty: Object.hasOwn(globalThis, name),
        ...(disposable ? { disposable } : {}),
        ...metadata(value),
        ...pins.get(name),
      };
    } catch {
      return { name, type: "unavailable", kind: "runtime-only", globalProperty: false, ...pins.get(name) };
    }
  };
  const status = (names) => {
    const usage = Deno.memoryUsage();
    return {
      memory: {
        heapUsedBytes: usage.heapUsed,
        heapTotalBytes: usage.heapTotal,
        heapLimitBytes: getHeapStatistics().heap_size_limit,
        rssBytes: usage.rss,
        externalBytes: usage.external,
      },
      bindings: [...new Set(names)].sort().map(inspect),
    };
  };
  const encode = (bytes) => {
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 32768) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + 32768));
    }
    return btoa(binary);
  };
  const capture = (
    names,
    maxBytes = Math.min(
      256 * 1024 * 1024,
      Math.max(8 * 1024 * 1024, Math.floor(getHeapStatistics().heap_size_limit / 8)),
    ),
  ) => {
    if (!Number.isSafeInteger(maxBytes) || maxBytes < 0 || maxBytes > 256 * 1024 * 1024) {
      throw new TypeError("invalid notebook checkpoint budget");
    }
    const candidates = [...new Set(names)].sort();
    if (candidates.length > 10000) throw new Error("Notebook checkpoint exceeds 10000 top-level values");
    const entries = [];
    const skipped = [];
    let total = 0;
    for (const name of candidates) {
      try {
        const value = read(name);
        let captured = value;
        let kind = "value";
        if (typeof value === "function") {
          captured = Function.prototype.toString.call(value);
          if (captured.includes("[native code]")) throw new Error("native or bound function");
          if (typeof (0, eval)("(" + captured + ")") !== "function") {
            throw new Error("function source did not reanimate");
          }
          kind = "function";
        }
        if (value instanceof Promise) throw new Error("promise");
        if (value instanceof WeakMap || value instanceof WeakSet) throw new Error("weak collection");
        if (value !== null && ["object", "function"].includes(typeof value)) {
          if (
            typeof descriptorValue(value, Symbol.dispose) === "function" ||
            typeof descriptorValue(value, Symbol.asyncDispose) === "function"
          ) throw new Error("disposable runtime handle");
          if (descriptorValue(value, Symbol.toStringTag) === "Module") throw new Error("module namespace");
        }
        const bytes = serialize(captured);
        if (bytes.length > maxBytes) throw new Error("exceeds per-variable checkpoint cap");
        if (total + bytes.length > maxBytes) throw new Error("exceeds total checkpoint cap");
        entries.push({ name, kind, data: encode(bytes), length: bytes.length, ...metadata(value), ...pins.get(name) });
        total += bytes.length;
      } catch (error) {
        skipped.push({ name, reason: reason(error) });
      }
    }
    return { deno: Deno.version.deno, v8: Deno.version.v8, entries, skipped };
  };
  const configurePins = (entries) => {
    const next = new Map();
    for (const entry of entries) {
      const name = nameCheck(entry.name);
      if (entry.pinned !== undefined && typeof entry.pinned !== "boolean") throw new TypeError("invalid pin metadata");
      if (entry.hook !== undefined && ![false, "startup", "tool_result"].includes(entry.hook)) {
        throw new TypeError("invalid notebook hook");
      }
      if (!entry.pinned) {
        if (entry.hook) throw new TypeError("notebook hooks must be pinned");
        continue;
      }
      if (entry.hook && typeof read(name) !== "function") {
        throw new TypeError("notebook hook must be a function: " + name);
      }
      next.set(name, { pinned: true, ...(entry.hook ? { hook: entry.hook } : {}) });
    }
    pins.clear();
    for (const [name, pin] of next) pins.set(name, pin);
  };
  const restore = async (snapshot, { startup = false } = {}) => {
    if (!snapshot || !Array.isArray(snapshot.entries)) throw new TypeError("invalid notebook snapshot");
    const seen = new Set();
    const prepared = snapshot.entries.map((entry) => {
      const name = nameCheck(entry.name);
      if (seen.has(name)) throw new Error("duplicate notebook snapshot binding: " + name);
      seen.add(name);
      if (!["value", "function"].includes(entry.kind) || typeof entry.data !== "string") {
        throw new TypeError("invalid notebook snapshot entry");
      }
      if (entry.pinned !== undefined && typeof entry.pinned !== "boolean") {
        throw new TypeError("invalid pin snapshot metadata");
      }
      const bytes = Uint8Array.from(atob(entry.data), (char) => char.charCodeAt(0));
      if (entry.length !== bytes.length) throw new Error("notebook snapshot length mismatch: " + name);
      const captured = deserialize(bytes);
      if (entry.kind === "function" && typeof captured !== "string") {
        throw new TypeError("invalid notebook function snapshot");
      }
      if (
        entry.hook && (!entry.pinned || !["startup", "tool_result"].includes(entry.hook) || entry.kind !== "function")
      ) throw new TypeError("invalid notebook hook snapshot");
      const descriptor = Object.getOwnPropertyDescriptor(globalThis, name);
      if (descriptor && !descriptor.configurable) throw new Error("notebook binding is not configurable: " + name);
      return { entry, captured, descriptor };
    });
    // Values and function definitions are installed by value. No original cells run.
    // Function source cannot capture a closure. Helpers must be self-contained or use globals.
    // Restore values first, as class static initialization can depend on retained globals.
    const ordered = [
      ...prepared.filter(({ entry }) => entry.kind === "value"),
      ...prepared.filter(({ entry }) => entry.kind === "function"),
    ];
    try {
      for (const { entry, captured } of ordered) {
        const value = entry.kind === "function" ? (0, eval)("(" + captured + ")") : captured;
        if (entry.kind === "function" && typeof value !== "function") {
          throw new TypeError("invalid notebook function snapshot");
        }
        applyMetadata(value, entry);
        Object.defineProperty(globalThis, entry.name, { value, writable: true, configurable: true, enumerable: true });
      }
    } catch (error) {
      for (const { entry, descriptor } of prepared) {
        if (descriptor) Object.defineProperty(globalThis, entry.name, descriptor);
        else delete globalThis[entry.name];
      }
      throw error;
    }
    for (const { entry } of prepared) {
      if (entry.pinned) pins.set(entry.name, { pinned: true, ...(entry.hook ? { hook: entry.hook } : {}) });
      else pins.delete(entry.name);
    }
    if (startup) await globalThis.__codexNotebook.runStartupHooks();
    return prepared.map(({ entry }) => entry.name);
  };
  const disposal = async (names, remove) => {
    const selected = [...new Set(names)];
    for (const name of selected) {
      nameCheck(name);
      if (remove && pins.has(name)) {
        throw new Error("Pinned notebook bindings cannot be released: " + name + "; unpin them first");
      }
    }
    // Lexical bindings cannot be deleted. The host must capture survivors then restart.
    if (remove && selected.some((name) => !Object.hasOwn(globalThis, name))) {
      return { released: [], disposed: [], failures: [], restartRequired: true };
    }
    const seen = new WeakSet();
    const released = [];
    const disposed = [];
    const failures = [];
    for (const name of selected) {
      try {
        const value = read(name);
        if (value !== null && ["object", "function"].includes(typeof value) && !seen.has(value)) {
          seen.add(value);
          if (typeof value[Symbol.asyncDispose] === "function") {
            await value[Symbol.asyncDispose]();
            disposed.push(name);
          } else if (typeof value[Symbol.dispose] === "function") {
            value[Symbol.dispose]();
            disposed.push(name);
          }
        }
        if (remove) {
          if (!delete globalThis[name]) throw new Error("binding is not configurable");
          projectNames.delete(name);
          released.push(name);
        }
      } catch (error) {
        failures.push({ name, reason: reason(error) });
      }
    }
    return { released, disposed, failures, restartRequired: false };
  };
  Object.defineProperty(globalThis, "__codexNotebookState", {
    value: Object.freeze({
      status,
      capture,
      restore,
      configurePins,
      release: (names) => disposal(names, true),
      dispose: (names) => disposal(names, false),
      projectBindings: () => [...projectNames].sort(),
      syncProjectBindings: (names) => {
        projectNames = new Set(names.map(nameCheck));
      },
      // Invalid identifiers remain visible as skipped captures, not failed cell completion.
      promote: (names) => {
        for (const name of names) if (typeof name === "string" && !name.startsWith("__codex")) projectNames.add(name);
      },
      hooks: (type) => [...pins].filter(([, pin]) => pin.hook === type).map(([name]) => name).sort(),
      read,
    }),
  });
})();
