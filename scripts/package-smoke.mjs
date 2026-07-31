#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const [platform, archiveDirectory] = process.argv.slice(2);
const platforms = new Set(["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win32-x64", "win32-arm64"]);
if (!platforms.has(platform) || !archiveDirectory) {
  console.error("Usage: node scripts/package-smoke.mjs <platform-arch> <archive-directory>");
  process.exit(2);
}

const rootPackage = JSON.parse(readFileSync("package.json", "utf8"));
const platformArchive = resolve(archiveDirectory, `my-token-scrooge-${platform}-${rootPackage.version}.tgz`);
assert.ok(existsSync(platformArchive), `Missing platform package: ${platformArchive}`);
const workspace = mkdtempSync(join(tmpdir(), `mts-package-smoke-${platform}-`));
const archives = join(workspace, "archives");
const install = join(workspace, "install");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

process.on("exit", () => rmSync(workspace, { recursive: true, force: true }));
mkdirSync(archives, { recursive: true });
mkdirSync(install, { recursive: true });
writeFileSync(join(install, "package.json"), "{\"private\":true}\n");

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8", windowsHide: true, shell: process.platform === "win32" });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return result;
}

run(npm, ["pack", ".", "--pack-destination", archives]);
const rootArchive = join(archives, `my-token-scrooge-${rootPackage.version}.tgz`);
assert.ok(existsSync(rootArchive), `Missing root package: ${rootArchive}`);

run(npm, ["install", "--prefix", install, platformArchive, "--ignore-scripts", "--no-audit", "--no-fund"]);
run(npm, ["install", "--prefix", install, rootArchive, "--omit=optional", "--ignore-scripts", "--no-audit", "--no-fund"]);

const launcher = join(install, "node_modules", "my-token-scrooge", "npm", "bin", "mts.js");
const launched = run(process.execPath, [launcher, "--version"]);
assert.match(launched.stdout, new RegExp(`^mts ${rootPackage.version}`));

run(npm, ["uninstall", "--prefix", install, "my-token-scrooge", `@my-token-scrooge/${platform}`, "--no-audit", "--no-fund"]);
assert.ok(!existsSync(join(install, "node_modules", "my-token-scrooge")));
assert.ok(!existsSync(join(install, "node_modules", "@my-token-scrooge", platform)));
console.log(`Package install, launch, and uninstall passed for ${platform}.`);
