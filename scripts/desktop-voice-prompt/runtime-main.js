/* codex-user-voice-prompt-v1 */
(() => {
  const { ipcMain } = require("electron");
  const fs = require("node:fs");
  const { TextDecoder } = require("node:util");
  const promptPath = __CODEX_VOICE_PROMPT_PATH__;
  ipcMain.on("codex-user-voice-prompt:read", (event) => {
    let descriptor;
    try {
      const frame = event.senderFrame;
      const url = new URL(frame.url);
      if (
        frame !== event.sender.mainFrame ||
        url.protocol !== "app:" ||
        url.host !== "-"
      ) {
        throw new Error("Voice prompt access denied for this frame");
      }
      // A configured FIFO must fail visibly, not block Electron's main thread.
      descriptor = fs.openSync(
        promptPath,
        fs.constants.O_RDONLY | fs.constants.O_NONBLOCK,
      );
      const stat = fs.fstatSync(descriptor);
      if (!stat.isFile() || stat.size > 65536) {
        throw new Error("Voice prompt must be a regular UTF-8 file of at most 64 KiB");
      }
      const bytes = Buffer.alloc(65537);
      const length = fs.readSync(descriptor, bytes, 0, bytes.length, 0);
      if (length > 65536) throw new Error("Voice prompt exceeds 64 KiB");
      const text = new TextDecoder("utf-8", { fatal: true }).decode(
        bytes.subarray(0, length),
      );
      if (!text.trim()) throw new Error("Voice prompt is empty");
      event.returnValue = { ok: true, text };
    } catch (error) {
      event.returnValue = { ok: false, error: error.message };
    } finally {
      if (descriptor !== undefined) fs.closeSync(descriptor);
    }
  });
})();
