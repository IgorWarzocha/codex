// Focused real-Deno REPL tests. The REPL and Jupyter share Deno's persistent lexical scope.
// Run: deno test --allow-read --allow-run codex-rs/notebook/src/kernel_test.js
import assert from "node:assert/strict";

const stateSource = await Deno.readTextFile(new URL("./kernel-state.js", import.meta.url));
const bootstrapSource = await Deno.readTextFile(new URL("./bootstrap.js", import.meta.url));
const inject = (source) => `await import(${JSON.stringify("data:text/javascript;base64," + btoa(source))});`;

async function repl(cells, maxHeapMiB) {
  const child = new Deno.Command(Deno.execPath(), {
    args: ["repl", "--quiet", "--allow-all", ...(maxHeapMiB ? ["--v8-flags=--max-old-space-size=" + maxHeapMiB] : [])],
    stdin: "piped",
    stdout: "piped",
    stderr: "piped",
    signal: AbortSignal.timeout(20_000),
  }).spawn();
  const output = child.output();
  const writer = child.stdin.getWriter();
  await writer.write(new TextEncoder().encode(
    [
      inject(stateSource),
      'globalThis.assert = (await import("node:assert/strict")).default;',
      ...cells,
      'console.log("__PASS__");',
    ].join("\n") + "\n",
  ));
  await writer.close();
  const result = await output;
  const stdout = new TextDecoder().decode(result.stdout);
  const stderr = new TextDecoder().decode(result.stderr);
  assert.equal(result.code, 0, stderr);
  assert.ok(stdout.includes("__PASS__"), stdout + stderr);
  assert.doesNotMatch(stdout + stderr, /Uncaught|parse error:/, stdout + stderr);
  return stdout.split("\n").filter((line) => line.startsWith("__RESULT__")).map((line) => JSON.parse(line.slice(10)));
}

Deno.test("capture lexical bindings and restore serialized values and function metadata without replay", async () => {
  const [snapshot] = await repl([
    "globalThis.originalSideEffect = 1;",
    'const lexical = { n: 42n, map: new Map([["key", new Set([3])]]), bytes: new Uint16Array([17, 65535]), date: new Date(1234) }; lexical.self = lexical;',
    "globalThis.dependency = 9;",
    "const helper = (n) => n + dependency;",
    "class StaticHelper { static value = dependency; }",
    'Object.defineProperties(helper, {description:{value:"Add the retained dependency"},usage:{value:"helper(4)"}});',
    "const promise = Promise.resolve(1); const weak = new WeakMap(); const native = Math.max; const handle = {[Symbol.dispose]() {}};",
    '__codexNotebookState.configurePins([{name:"helper",pinned:true}]);',
    'assert.equal(__codexNotebookState.status(["lexical"]).bindings[0].globalProperty, false);',
    'const captured = __codexNotebookState.capture(["lexical","helper","dependency","StaticHelper","promise","weak","native","handle"], 1048576);',
    'assert.deepEqual(captured.skipped.map(x=>x.name), ["handle","native","promise","weak"]);',
    'assert.equal(__codexNotebookState.capture(["lexical"],1).skipped.length, 1);',
    'console.log("__RESULT__" + JSON.stringify(captured));',
  ]);
  await repl([
    `await __codexNotebookState.restore(${JSON.stringify(snapshot)});`,
    'assert.equal(typeof originalSideEffect,"undefined");',
    'assert.equal(lexical.n,42n); assert.equal(lexical.self,lexical); assert.ok(lexical.map.get("key").has(3));',
    "assert.ok(lexical.bytes instanceof Uint16Array); assert.equal(lexical.bytes[1],65535); assert.equal(lexical.date.getTime(),1234);",
    'assert.equal(helper(4),13); assert.equal(helper.description,"Add the retained dependency"); assert.equal(helper.usage,"helper(4)");',
    "assert.equal(StaticHelper.value,9);",
    'assert.equal(__codexNotebookState.capture(["helper"]).entries[0].pinned,true);',
    `await assert.rejects(__codexNotebookState.restore(${
      JSON.stringify({ ...snapshot, entries: snapshot.entries.map((entry) => ({ ...entry, length: 0 })) })
    }), /length mismatch/);`,
  ]);
});

Deno.test("pins protect release, lexical release requires restart, disposal is awaited and deduplicated", async () => {
  await repl([
    "const lexical = 1;",
    'globalThis.calls = []; globalThis.resource = {[Symbol.asyncDispose]: async () => { await Promise.resolve(); calls.push("disposed"); }}; globalThis.alias = resource;',
    'globalThis.hook = () => {}; __codexNotebookState.configurePins([{name:"hook",pinned:true,hook:"tool_result"}]);',
    'assert.throws(()=>__codexNotebookState.configurePins([{name:"hook",pinned:true,hook:"bad"}]),/invalid notebook hook/); assert.deepEqual(__codexNotebookState.hooks("tool_result"),["hook"]);',
    'await assert.rejects(__codexNotebookState.release(["hook"]),/Pinned/);',
    'assert.equal((await __codexNotebookState.release(["lexical","resource"])).restartRequired,true); assert.equal(calls.length,0);',
    'const released = await __codexNotebookState.release(["resource","alias"]); assert.deepEqual(released.released,["resource","alias"]); assert.deepEqual(calls,["disposed"]);',
    'globalThis.bad = {[Symbol.dispose]() { throw new Error("close failed"); }}; const failure = await __codexNotebookState.release(["bad"]); assert.equal(failure.failures[0].reason,"close failed"); assert.ok("bad" in globalThis);',
    '__codexNotebookState.configurePins([]); assert.deepEqual((await __codexNotebookState.release(["hook"])).released,["hook"]);',
    "assert.ok(__codexNotebookState.status([]).memory.heapLimitBytes > 0);",
  ]);
});

Deno.test("checkpoint budgets skip oversized values and continue after aggregate overflow", async () => {
  await repl([
    "globalThis.aLarge = new Uint8Array(64); globalThis.bLarge = new Uint8Array(64); globalThis.cTiny = 1;",
    'const largeBytes = __codexNotebookState.capture(["aLarge"], 1024).entries[0].length; const tinyBytes = __codexNotebookState.capture(["cTiny"], 1024).entries[0].length;',
    'assert.equal(__codexNotebookState.capture(["aLarge"], largeBytes - 1).skipped[0].reason, "exceeds per-variable checkpoint cap");',
    'const bounded = __codexNotebookState.capture(["aLarge", "bLarge", "cTiny"], largeBytes + tinyBytes); assert.deepEqual(bounded.entries.map(x => x.name), ["aLarge", "cTiny"]); assert.equal(bounded.skipped[0].reason, "exceeds total checkpoint cap");',
    "assert.throws(() => __codexNotebookState.capture([], 256 * 1024 * 1024 + 1), /invalid notebook checkpoint budget/);",
    'const heap = (await import("node:v8")).getHeapStatistics().heap_size_limit; const budget = Math.min(256 * 1024 * 1024, Math.max(8 * 1024 * 1024, Math.floor(heap / 8))); globalThis.oversized = new Uint8Array(budget + 1); assert.equal(__codexNotebookState.capture(["oversized"]).skipped[0].reason, "exceeds per-variable checkpoint cap");',
  ], 128);
});

Deno.test("tool hooks suppress only their async context, retain tool values, and drain unawaited hooks", async () => {
  const bootstrap = bootstrapSource.replace("__ENDPOINT__", '"http://127.0.0.1:" + server.addr.port').replace(
    "__CREDENTIAL__",
    '"test"',
  );
  await repl([
    'globalThis.requests = []; globalThis.server = Deno.serve({hostname:"127.0.0.1",port:0,onListen(){}}, async req => { const body = await req.json(); requests.push(body); if (body.input?.fail) return Response.json({ok:false,error:"failed"},{status:400}); return Response.json({ok:true,value: body.op === "tool" ? {name:body.name,input:body.input} : null}); });',
    inject(bootstrap),
    'assert.throws(()=>text("outside cell"),/outside an active exec/);',
    "globalThis.events = []; globalThis.observer = async event => { events.push(event); await new Promise(r=>setTimeout(r,30)); await tools.echo({nested:true}); };",
    '__codexNotebookState.configurePins([{name:"observer",pinned:true,hook:"tool_result"}]);',
    '__codexNotebook.begin("one",[{name:"echo",description:"Echo owned input",input_schema:{type:"object"}}]);',
    'assert.deepEqual(ALL_TOOLS[0].input_schema,{type:"object"}); assert.equal(tools.echo.description,"Echo owned input");',
    "const first = tools.echo({one:true}); await new Promise(r=>setTimeout(r,10)); const second = tools.echo({two:true});",
    "const results = await Promise.all([first,second]); assert.equal(results[0].input.one,true); assert.equal(events.length,2);",
    'await assert.rejects(tools.echo({fail:true}),/failed/); assert.equal(events[2].status,"error"); assert.equal(events[2].error,"failed");',
    'tools.echo({unawaited:true}); await __codexNotebook.flush("one"); assert.equal(events.length,4); assert.equal(requests.filter(x=>x.input?.nested).length,4);',
    'globalThis.newProjectGlobal = 4; globalThis["invalid-name"] = 1; __codexNotebook.completed("one"); assert.ok(__codexNotebookState.projectBindings().includes("newProjectGlobal")); assert.equal(__codexNotebookState.capture(["invalid-name"]).skipped.length,1);',
    'globalThis.starter = async event => { assert.equal(event.type,"startup"); globalThis.started = (globalThis.started ?? 0) + 1; }; __codexNotebookState.configurePins([{name:"starter",pinned:true,hook:"startup"}]); await __codexNotebook.runStartupHooks(); assert.equal(started,1);',
    '__codexNotebook.begin("two",[]); text("before exit"); assert.throws(()=>exit(), {name:"CodexNotebookExit",message:"__CodexNotebookExit__"}); await __codexNotebook.flush("two"); __codexNotebook.completed("two"); assert.equal(requests.at(-1).item.text,"before exit");',
    "await server.shutdown();",
  ]);
});
