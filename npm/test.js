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

// What the kernel says about a process, for failure messages ("S sleeping", "Z zombie", ...).
const procState = (pid) => {
  try {
    return fs.readFileSync(`/proc/${pid}/status`, "utf8").split("\n").filter((l) => /^(State|PPid|SigBlk|SigIgn|SigCgt):/.test(l)).join("; ");
  } catch {
    return "gone";
  }
};

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
// Polls `cond` until it is true or `ms` pass; returns its last value. The default is generous on
// purpose: a loaded machine can take seconds to start a process, and a wait that ends as soon
// as the condition holds costs nothing when everything is fast.
const until = async (cond, ms = 20000) => {
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
    // the fake binary records its own process id, then becomes a long `sleep`
    const { dir, launcher } = project(`echo $$ > "${"${PIDFILE}"}"; exec sleep 60`);
    const pidfile = path.join(dir, "pid");
    const child = spawn(process.execPath, [launcher], { env: { ...process.env, PIDFILE: pidfile }, stdio: "ignore" });
    let native;
    try {
      assert.ok(await until(() => fs.existsSync(pidfile) && fs.readFileSync(pidfile, "utf8").trim()), "child started");
      native = Number(fs.readFileSync(pidfile, "utf8"));
      const exited = new Promise((res) => child.on("exit", (code, signal) => res({ code, signal })));
      child.kill(sig);
      const { code, signal } = await exited;
      assert.ok(await until(() => !alive(native)), `native process ${native} still running after ${sig}: ${procState(native)}`);
      // like a shell: killed by the signal, or 128+n
      assert.ok(signal === sig || code === 128 + os.constants.signals[sig], `exit was code=${code} signal=${signal}`);
    } finally {
      // never leave a stray `sleep` behind, even when an assertion above failed
      child.kill("SIGKILL");
      if (native && alive(native)) process.kill(native, "SIGKILL");
    }
  });
}

test("a missing platform package is a clear error", () => {
  const { dir, launcher } = project("true");
  fs.rmSync(path.join(dir, "node_modules", `safesweep-linux-${arch}`), { recursive: true });
  const r = spawnSync(process.execPath, [launcher], { encoding: "utf8" });
  assert.strictEqual(r.status, 1);
  assert.match(r.stderr, /is not installed/);
});

// --- npm/publish.sh -------------------------------------------------------------------------
// Tested with a stand-in `npm` that records its calls, so nothing is ever published.

const { execFileSync } = require("node:child_process");

// Runs publish.sh with a fake `npm`. `published` lists package@version strings that `npm view`
// should report as existing. Returns the recorded `npm publish` calls, one per line.
function runPublish(published, dist) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "publish-"));
  const bin = path.join(dir, "bin");
  fs.mkdirSync(bin);
  const log = path.join(dir, "calls.log");
  for (const p of ["safesweep-linux-x64", "safesweep-linux-arm64", "safesweep"]) {
    fs.mkdirSync(path.join(dir, "out", p), { recursive: true });
  }
  fs.writeFileSync(
    path.join(bin, "npm"),
    `#!/bin/sh
if [ "$1" = view ]; then
  case " $PUBLISHED " in *" $2 "*) exit 0 ;; esac
  exit 1
fi
echo "$(basename "$PWD") $*" >> "$LOG"
`,
    { mode: 0o755 },
  );
  execFileSync(path.join(__dirname, "publish.sh"), ["v1.2.3", dist, path.join(dir, "out")], {
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, PUBLISHED: published.join(" "), LOG: log },
    stdio: "ignore",
  });
  return fs.existsSync(log) ? fs.readFileSync(log, "utf8").trim().split("\n") : [];
}

test("publish.sh publishes the platform packages first, then the launcher", () => {
  assert.deepStrictEqual(runPublish([], "latest"), [
    "safesweep-linux-x64 publish --provenance --access public --tag latest",
    "safesweep-linux-arm64 publish --provenance --access public --tag latest",
    "safesweep publish --provenance --access public --tag latest",
  ]);
});

test("publish.sh skips versions that are already on npm and uses the given dist-tag", () => {
  const calls = runPublish(["safesweep-linux-x64@1.2.3", "safesweep@1.2.3"], "next");
  assert.deepStrictEqual(calls, ["safesweep-linux-arm64 publish --provenance --access public --tag next"]);
});

test("publish.sh publishes nothing when everything is already published", () => {
  const all = ["safesweep-linux-x64@1.2.3", "safesweep-linux-arm64@1.2.3", "safesweep@1.2.3"];
  assert.deepStrictEqual(runPublish(all, "latest"), []);
});
