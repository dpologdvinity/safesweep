#!/usr/bin/env node
// Launcher for the `safesweep` npm package.
//
// The real program is a native binary, shipped in a platform package (`safesweep-linux-x64` or
// `safesweep-linux-arm64`). npm, pnpm, yarn and bun install only the one matching this machine,
// as an optional dependency of this package. This script finds that binary and runs it with the
// same arguments, standard streams, exit code and signals, so `safesweep` behaves the same
// whether it was installed from npm, cargo, apt or anywhere else.
//
// Only Linux (including WSL) on x64 and arm64 is supported; anything else exits with an
// explanation. Tested by npm/test.js.
"use strict";
const { spawn } = require("node:child_process");

// Node's `process.arch` names ("x64", "arm64") match the platform packages' suffixes.
const arch = { x64: "x64", arm64: "arm64" }[process.arch];
if (process.platform !== "linux" || !arch) {
  console.error(`safesweep: only Linux (or WSL) on x64 and arm64 is supported, not ${process.platform}-${process.arch}`);
  process.exit(1);
}
const pkg = `safesweep-linux-${arch}`;
// Resolve the binary through Node's module lookup, so it is found wherever the package manager
// put it (flat node_modules, pnpm's store, Yarn PnP).
let bin;
try {
  bin = require.resolve(`${pkg}/bin/safesweep`);
} catch {
  console.error(`safesweep: ${pkg} is not installed; reinstall without --no-optional / --omit=optional`);
  process.exit(1);
}
// Async spawn, not spawnSync: a signal sent to this launcher (`timeout`, Ctrl-C, a service stop)
// must reach the native process, or it would keep deleting after its parent is gone.
//
// The handlers are installed BEFORE the child is spawned. Installed afterwards, there is a small
// window in which the child is already running but a signal would still hit Node's default
// action and kill this launcher outright, leaving the native process orphaned and unstoppable.
// On a busy machine that window is large enough to hit (it was found by a flaky test). A signal
// that arrives before the child exists is remembered and delivered as soon as it does.
const signals = ["SIGTERM", "SIGINT", "SIGHUP"];
let child = null;
let early = null; // a signal that arrived before the child was spawned
const forward = (sig) => () => {
  if (child) child.kill(sig);
  else early = sig;
};
const handlers = signals.map((sig) => [sig, forward(sig)]);
for (const [sig, h] of handlers) process.on(sig, h);

child = spawn(bin, process.argv.slice(2), { stdio: "inherit" });
if (early) child.kill(early);
child.on("error", (e) => {
  console.error(`safesweep: ${e.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  for (const [sig, h] of handlers) process.removeListener(sig, h);
  if (signal) process.kill(process.pid, signal); // die by the same signal, like a shell would report
  process.exit(code ?? 1);
});
