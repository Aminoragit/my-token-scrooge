import { existsSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const localBinary = resolve("target", "debug", process.platform === "win32" ? "mts.exe" : "mts");
const binary = process.env.MTS_BINARY || (existsSync(localBinary) ? localBinary : null);
const output = binary
  ? spawnSync(binary, ["harness", "list", "--format", "markdown"], { encoding: "utf8" })
  : spawnSync(
      process.env.CARGO || "cargo",
      ["run", "--quiet", "--bin", "mts", "--", "harness", "list", "--format", "markdown"],
      { encoding: "utf8" }
    );
if (output.status !== 0) {
  console.error(output.stderr);
  process.exit(output.status ?? 1);
}
const content = `# Generated harness support matrix\n\nRegistry entries are planned maxima, not live verification claims. Unknown versions remain UNVERIFIED and SHADOW.\n\n${output.stdout}`;
const path = "docs/support-matrix.md";
if (process.argv.includes("--check")) {
  const current = await readFile(path, "utf8").catch(() => "");
  if (current !== content) {
    console.error(`${path} is stale. Run node scripts/generate-support-matrix.mjs.`);
    process.exit(1);
  }
} else {
  await writeFile(path, content);
  console.log(`Generated ${path}.`);
}
