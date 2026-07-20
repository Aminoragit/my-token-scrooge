#!/usr/bin/env node
import { existsSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const platform = `${process.platform}-${process.arch}`;
const executable = process.platform === "win32" ? "mts.exe" : "mts";
const candidates = [];

if (process.env.MTS_BINARY) {
  candidates.push(resolve(process.env.MTS_BINARY));
} else {
  try {
    const packageJson = require.resolve(`@my-token-scrooge/${platform}/package.json`);
    candidates.push(resolve(dirname(packageJson), "bin", executable));
  } catch {}
  candidates.push(resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "target", "release", executable));
  candidates.push(resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "target", "debug", executable));
}

const binary = candidates.find((path) => existsSync(path) && statSync(path).isFile());
if (!binary) {
  console.error(`MTS_INSTALL_ERROR: no trusted binary found for ${platform}. Reinstall my-token-scrooge or set MTS_BINARY.`);
  process.exit(1);
}
const metadata = statSync(binary);
if (process.platform !== "win32" && (metadata.mode & 0o002) !== 0) {
  console.error("MTS_INSTALL_ERROR: refusing a world-writable binary.");
  process.exit(1);
}
if (typeof process.getuid === "function" && metadata.uid !== 0 && metadata.uid !== process.getuid()) {
  console.error("MTS_INSTALL_ERROR: refusing a binary owned by another user.");
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: "inherit", windowsHide: false });
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => child.kill(signal));
}
child.on("error", (error) => {
  console.error(`MTS_LAUNCH_ERROR: ${error.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 1);
});
