import { chmod, copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";

const [platform, source, destination = `npm/platform-packages/${process.argv[2]}`] = process.argv.slice(2);
if (!platform || !source) {
  console.error("Usage: node scripts/stage-platform-package.mjs <platform-arch> <binary> [destination]");
  process.exit(2);
}

const packageDirectory = resolve(destination);
const executable = platform.startsWith("win32-") ? "mts.exe" : "mts";
await mkdir(resolve(packageDirectory, "bin"), { recursive: true });
await copyFile(resolve(source), resolve(packageDirectory, "bin", executable));
await copyFile(resolve("LICENSE"), resolve(packageDirectory, "LICENSE"));
if (executable === "mts") await chmod(resolve(packageDirectory, "bin", executable), 0o755);
const rootPackage = JSON.parse(await readFile(resolve("package.json"), "utf8"));
await writeFile(resolve(packageDirectory, "package.json"), `${JSON.stringify({
  name: `@my-token-scrooge/${platform}`,
  version: rootPackage.version,
  description: `Native my-token-scrooge binary for ${platform}`,
  license: rootPackage.license,
  repository: rootPackage.repository,
  homepage: rootPackage.homepage,
  os: [platform.split("-")[0]],
  cpu: [platform.split("-")[1]],
  files: [`bin/${basename(executable)}`, "LICENSE"],
  publishConfig: { access: "public" }
}, null, 2)}\n`);
console.log(`Staged ${platform} in ${dirname(resolve(packageDirectory, "bin", executable))}.`);
