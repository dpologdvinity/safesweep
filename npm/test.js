// Tests for the npm launcher (npm/safesweep/bin/safesweep.js).
//
// Run with:  node --test npm/test.js     (Node 18 or newer; CI runs it on every pull request)
//
// The launcher is tested against a fake platform package: a small shell script in the place the
// real native binary would be. That checks what the launcher is responsible for (finding the
// binary, passing arguments, output and exit codes through, forwarding signals, failing clearly)
// without needing a real build.
"use strict";
const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");

// The platform package name suffix for the machine the tests run on.
const arch = { x64: "x64", arm64: "arm64" }[process.arch];

// A project dir holding the launcher and a fake `safesweep-linux-<arch>` whose binary is `body`.
function project(body) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "launcher-"));
  const pkg = path.join(dir, "node_modules", `safesweep-linux-${arch}`, "bin");
  fs.mkdirSync(pkg, { recursive: true });
  fs.writeFileSync(path.join(pkg, "safesweep"), `#!/bin/sh\n${body}\n`, { mode: 0o755 });
  const bin = path.join(dir, "node_modules", "safesweep", "bin");
  fs.mkdirSync(bin, { recursive: true });
  fs.copyFileSync(path.join(__dirname, "safesweep", "bin", "safesweep.js"), path.join(bin, "safesweep.js"));
  return { dir, launcher: path.join(bin, "safesweep.js") };
}

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
const until = async (cond, ms = 3000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (cond()) return true;
    await new Promise((r) => setTimeout(r, 25));
  }
  return cond();
};

test("passes arguments, stdout and the exit code through", () => {
  const { launcher } = project('echo "args: $*"; exit 7');
  const r = spawnSync(process.execPath, [launcher, "scan", "--json"], { encoding: "utf8" });
  assert.strictEqual(r.stdout.trim(), "args: scan --json");
  assert.strictEqual(r.status, 7);
});

for (const sig of ["SIGTERM", "SIGINT", "SIGHUP"]) {
  test(`${sig} to the launcher stops the native process too`, async () => {
    const { dir, launcher } = project(`echo $$ > "${"${PIDFILE}"}"; exec sleep 60`);
    const pidfile = path.join(dir, "pid");
    const child = spawn(process.execPath, [launcher], { env: { ...process.env, PIDFILE: pidfile }, stdio: "ignore" });
    assert.ok(await until(() => fs.existsSync(pidfile) && fs.readFileSync(pidfile, "utf8").trim()), "child started");
    const native = Number(fs.readFileSync(pidfile, "utf8"));
    const exited = new Promise((res) => child.on("exit", (code, signal) => res({ code, signal })));
    child.kill(sig);
    const { code, signal } = await exited;
    assert.ok(await until(() => !alive(native)), `native process ${native} still running after ${sig}`);
    // like a shell: killed by the signal, or 128+n
    assert.ok(signal === sig || code === 128 + os.constants.signals[sig], `exit was code=${code} signal=${signal}`);
  });
}

test("a missing platform package is a clear error", () => {
  const { dir, launcher } = project("true");
  fs.rmSync(path.join(dir, "node_modules", `safesweep-linux-${arch}`), { recursive: true });
  const r = spawnSync(process.execPath, [launcher], { encoding: "utf8" });
  assert.strictEqual(r.status, 1);
  assert.match(r.stderr, /is not installed/);
});
