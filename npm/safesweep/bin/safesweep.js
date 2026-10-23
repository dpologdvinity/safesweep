#!/usr/bin/env node
// Runs the native safesweep binary from the platform package npm/pnpm/yarn/bun installed
// alongside this one (only the package matching this machine's os/cpu gets installed).
"use strict";
const { spawnSync } = require("node:child_process");

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
const r = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (r.error) {
  console.error(`safesweep: ${r.error.message}`);
  process.exit(1);
}
if (r.signal) process.kill(process.pid, r.signal);
process.exit(r.status ?? 1);
