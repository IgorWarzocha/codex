/* codex-user-voice-prompt-v1 */
(() => {
  const { contextBridge, ipcRenderer } = require("electron");
  contextBridge.exposeInMainWorld("codexUserVoicePrompt", {
    read: () => {
      const result = ipcRenderer.sendSync("codex-user-voice-prompt:read");
      if (!result || result.ok !== true || typeof result.text !== "string") {
        throw new Error(`User voice prompt: ${result?.error ?? "loader unavailable"}`);
      }
      return result.text;
    },
  });
})();
