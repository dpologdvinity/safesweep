#!/usr/bin/env node
// Runs the native safesweep binary from the platform package npm/pnpm/yarn/bun installed
// alongside this one (only the package matching this machine's os/cpu gets installed).
"use strict";
const { spawn } = require("node:child_process");

const arch = { x64: "x64", arm64: "arm64" }[process.arch];
if (process.platform !== "linux" || !arch) {
  console.error(`safesweep: only Linux (or WSL) on x64 and arm64 is supported, not ${process.platform}-${process.arch}`);
  process.exit(1);
}
const pkg = `safesweep-linux-${arch}`;
let bin;
try {
  bin = require.resolve(`${pkg}/bin/safesweep`);
} catch {
  console.error(`safesweep: ${pkg} is not installed; reinstall without --no-optional / --omit=optional`);
  process.exit(1);
}
// Async spawn, not spawnSync: a signal sent to this launcher (`timeout`, Ctrl-C, a service stop)
// must reach the native process, or it would keep deleting after its parent is gone.
const child = spawn(bin, process.argv.slice(2), { stdio: "inherit" });
const forward = (sig) => () => child.kill(sig);
const handlers = ["SIGTERM", "SIGINT", "SIGHUP"].map((sig) => [sig, forward(sig)]);
for (const [sig, h] of handlers) process.on(sig, h);
child.on("error", (e) => {
  console.error(`safesweep: ${e.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  for (const [sig, h] of handlers) process.removeListener(sig, h);
  if (signal) process.kill(process.pid, signal); // die by the same signal, like a shell would report
  process.exit(code ?? 1);
});
