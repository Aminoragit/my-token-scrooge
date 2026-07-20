import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { scan } from "./english-gate.mjs";

test("English-only gate reports project-owned non-English text", async () => {
  const root = await mkdtemp(join(tmpdir(), "mts-language-"));
  await mkdir(join(root, "src"));
  await writeFile(join(root, "src", "message.rs"), "const MESSAGE: &str = \"실패\";\n");
  assert.deepEqual(await scan(root), ["src/message.rs"]);
});

test("English-only gate preserves raw benchmark evidence", async () => {
  const root = await mkdtemp(join(tmpdir(), "mts-language-"));
  await mkdir(join(root, "benchmarks", "results", "run"), { recursive: true });
  await writeFile(join(root, "benchmarks", "results", "run", "stderr.txt"), "파일 오류\n");
  assert.deepEqual(await scan(root), []);
});
