import { readFile, readdir } from "node:fs/promises";
import { extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const included = new Set([".rs", ".js", ".mjs", ".ts", ".md", ".json", ".toml", ".txt", ".yml", ".yaml", ".ps1", ".sh"]);
const excluded = new Set(["target", "node_modules", ".git"]);
const excludedPrefixes = ["benchmarks/results/"];
const allowedUnicode = new Set(["scripts/english-gate.test.mjs"]);
const nonEnglishScript = /[\u0370-\u052f\u0590-\u08ff\u0900-\u1fff\u2c00-\ud7ff\uf900-\ufaff]/u;

export async function scan(root) {
  const failures = [];
  async function visit(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (excluded.has(entry.name)) continue;
      const path = join(directory, entry.name);
      const name = relative(root, path).replaceAll("\\", "/");
      if (entry.isDirectory()) {
        if (!excludedPrefixes.some((prefix) => `${name}/`.startsWith(prefix))) await visit(path);
      }
      else if (included.has(extname(entry.name))) {
        if (!allowedUnicode.has(name) && nonEnglishScript.test(await readFile(path, "utf8"))) failures.push(name);
      }
    }
  }
  await visit(root);
  return failures;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const failures = await scan(process.cwd());
  if (failures.length) {
    console.error(`English-only gate failed:\n${failures.join("\n")}`);
    process.exit(1);
  }
  console.log("English-only gate passed.");
}
