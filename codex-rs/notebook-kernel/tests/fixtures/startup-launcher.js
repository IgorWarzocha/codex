// Test-only executable adapter. Bind a selected connection-file port before
// launching the real Deno kernel so its own protocol reports the collision.
const directory = Deno.env.get("CODEX_TEST_STARTUP_DIRECTORY");
const executable = Deno.env.get("CODEX_TEST_REAL_DENO");
const mode = Deno.env.get("CODEX_TEST_STARTUP_MODE");
const path = Deno.args.at(-1);
const info = JSON.parse(Deno.readTextFileSync(path));
const log = directory + "/attempts.jsonl";
let previous = [];
try {
  previous = Deno.readTextFileSync(log).trim().split("\n").filter(Boolean).map(JSON.parse);
} catch (error) {
  if (!(error instanceof Deno.errors.NotFound)) throw error;
}
const attempt = previous.length + 1;
let previousConnectionExists = false;
if (previous.length) {
  try {
    Deno.statSync(previous.at(-1).path);
    previousConnectionExists = true;
  } catch (error) {
    if (!(error instanceof Deno.errors.NotFound)) throw error;
  }
}
const stalled = ["cancel", "deadline"].includes(mode) && attempt > 1;
const collision = mode !== "ordinary" && !stalled && (mode === "persistent" || attempt === 1);
const reservation = collision ? Deno.listen({ hostname: info.ip, port: info.shell_port }) : undefined;
if (mode === "deadline" && attempt === 1) await new Promise((resolve) => setTimeout(resolve, 2000));
const args = mode === "ordinary"
  ? ["jupyter", "--not-a-real-kernel-option"]
  : stalled ? ["eval", "setInterval(() => {}, 1000)"] : Deno.args;
const child = new Deno.Command(executable, { args, stdout: "null", stderr: "inherit" }).spawn();
const record = {
  path, key: info.key, ports: [info.shell_port, info.iopub_port, info.stdin_port, info.control_port, info.hb_port],
  leader: Deno.pid, child: child.pid, previousConnectionExists,
};
const next = directory + "/attempts-next.jsonl";
Deno.writeTextFileSync(next, [...previous, record].map((entry) => JSON.stringify(entry)).join("\n") + "\n");
Deno.renameSync(next, log);
const status = await child.status;
reservation?.close();
Deno.exit(status.code);
