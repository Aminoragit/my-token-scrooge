import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

test("launcher reports a stable installation error without a binary", () => {
  const result = spawnSync(process.execPath, [resolve("npm/bin/mts.js")], {
    encoding: "utf8",
    env: { ...process.env, MTS_BINARY: resolve("missing-mts-binary") }
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /MTS_INSTALL_ERROR/);
});

test("launcher forwards arguments to an explicit binary", (context) => {
  const fixtureDirectory = mkdtempSync(join(tmpdir(), "mts-launcher-"));
  const fixtureBinary = join(fixtureDirectory, process.platform === "win32" ? "node.exe" : "node");
  copyFileSync(process.execPath, fixtureBinary);
  if (process.platform !== "win32") chmodSync(fixtureBinary, 0o755);
  context.after(() => rmSync(fixtureDirectory, { recursive: true, force: true }));

  const result = spawnSync(process.execPath, [resolve("npm/bin/mts.js"), "--version"], {
    encoding: "utf8",
    env: { ...process.env, MTS_BINARY: fixtureBinary }
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^v\d+/);
});
