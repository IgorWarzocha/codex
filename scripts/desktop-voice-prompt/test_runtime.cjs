const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const vm = require("node:vm");
const { test } = require("node:test");
const { TextDecoder } = require("node:util");

test("file-backed prompt bridge reloads, fails closed, and changes only wingman instructions", () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "codex-voice-test-"));
  const prompt = path.join(directory, "voice.md");
  try {
    let handler;
    let bridge;
    const frame = { url: "app://-/index.html" };
    const sender = { mainFrame: frame };
    const electron = {
      ipcMain: {
        on: (channel, callback) => {
          assert.equal(channel, "codex-user-voice-prompt:read");
          handler = callback;
        },
      },
      ipcRenderer: {
        sendSync: (channel) => {
          assert.equal(channel, "codex-user-voice-prompt:read");
          const event = { sender, senderFrame: frame };
          handler(event);
          return event.returnValue;
        },
      },
      contextBridge: {
        exposeInMainWorld: (name, value) => {
          assert.equal(name, "codexUserVoicePrompt");
          bridge = value;
        },
      },
    };
    const imports = (name) => {
      if (name === "electron") return electron;
      if (name === "node:fs") return fs;
      if (name === "node:util") return { TextDecoder };
      throw new Error(`Unexpected import: ${name}`);
    };
    const main = fs
      .readFileSync(path.join(__dirname, "runtime-main.js"), "utf8")
      .replace("__CODEX_VOICE_PROMPT_PATH__", JSON.stringify(prompt));
    vm.runInNewContext(main, { require: imports, URL, Buffer });
    vm.runInNewContext(
      fs.readFileSync(path.join(__dirname, "runtime-preload.js"), "utf8"),
      { require: imports },
    );
    fs.writeFileSync(prompt, "Speak plainly. Use native handoffs.\n");
    assert.equal(bridge.read(), "Speak plainly. Use native handoffs.\n");
    fs.writeFileSync(prompt, "Updated instructions: café ☕\n");
    assert.equal(bridge.read(), "Updated instructions: café ☕\n");
    fs.writeFileSync(prompt, "x".repeat(65536));
    assert.equal(bridge.read().length, 65536);
    for (const invalid of [Buffer.from([0xff]), "  \n", "x".repeat(65537)]) {
      fs.writeFileSync(prompt, invalid);
      assert.throws(() => bridge.read(), /User voice prompt:/);
    }
    fs.unlinkSync(prompt);
    assert.throws(() => bridge.read(), /ENOENT/);
    fs.mkdirSync(prompt);
    assert.throws(() => bridge.read(), /regular UTF-8 file/);
    fs.rmdirSync(prompt);
    fs.writeFileSync(prompt, "custom prompt");
    for (const badFrame of [
      { url: "https://example.com/" },
      { url: "app://fs/@fs/private" },
      { url: "app://-/index.html" },
      { url: "not a URL" },
    ]) {
      const event = { sender, senderFrame: badFrame };
      handler(event);
      assert.equal(event.returnValue.ok, false);
      assert.equal(event.returnValue.text, undefined);
    }
    const expression = process.env.CODEX_VOICE_EXPRESSION;
    assert.ok(expression, "run through the Python test suite to use the production replacement");
    const nativeTools = [{ name: "native-tool", parameters: {} }];
    const nativeHints = ["native-hint"];
    const evaluate = (mode) =>
      vm.runInNewContext(`({client_tools,system_hints,${expression}})`, {
        b: mode,
        w: undefined,
        client_tools: nativeTools,
        system_hints: nativeHints,
        codexUserVoicePrompt: bridge,
      });
    const wingman = evaluate("wingman");
    assert.equal(wingman.bidi_system_prompt_override, "custom prompt");
    assert.equal(wingman.client_tools, nativeTools);
    assert.equal(wingman.system_hints, nativeHints);
    fs.unlinkSync(prompt);
    assert.throws(() => evaluate("wingman"), /ENOENT/);
    assert.equal(evaluate("standard").bidi_system_prompt_override, undefined);
    assert.equal(evaluate("advanced").bidi_system_prompt_override, undefined);
    // A stale IPC contract must not silently restore the default prompt.
    electron.ipcRenderer.sendSync = () => undefined;
    assert.throws(() => bridge.read(), /loader unavailable/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
