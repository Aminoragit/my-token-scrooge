import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

test("launcher reports a stable installation error without a binary", () => {
  const result = spawnSync(process.execPath, [resolve("npm/bin/mts.js")], {
    encoding: "utf8",
    env: { ...process.env, MTS_BINARY: resolve("missing-mts-binary") }
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /MTS_INSTALL_ERROR/);
});

test("launcher forwards arguments to an explicit binary", () => {
  const result = spawnSync(process.execPath, [resolve("npm/bin/mts.js"), "--version"], {
    encoding: "utf8",
    env: { ...process.env, MTS_BINARY: process.execPath }
  });
  assert.equal(result.status, 0);
  assert.match(result.stdout, /^v\d+/);
});
