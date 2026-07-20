import assert from "node:assert/strict";
import { lstat, readFile, readdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const registry = await readFile(join(root, "..", "crates", "mts-harnesses", "src", "lib.rs"), "utf8");
const expectedIds = [...registry.matchAll(/target!\(\s*"([^"]+)"/g)].map((match) => match[1]).sort();
const manifestDir = join(root, "manifests");
const manifests = (await readdir(manifestDir)).filter((name) => name.endsWith(".toml")).sort();
const fixtureNames = ["detection", "version", "install", "allow", "block-full", "block-partial", "malformed", "timeout", "unknown-version", "uninstall", "doctor"];
const requiredKeys = ["schema_version", "id", "display_name", "vendor", "execution_surface", "families", "commands", "markers", "installation_form", "policy_dir", "minimum_mts_version", "owner", "source", "default_mode", "capability_grade", "verification_status", "planned_capability", "install_dry_run", "install", "uninstall", "deployment", "owned_files", "fixture_dir", "doctor_template", "version_args", "help_args", "contract", "read", "write", "edit", "shell", "rewrite_input", "replace_output", "remote_executor", "live_verified", "unknown_version_mode", "mcp_is_enforcement_boundary", "unknown_internal_tools", "policy_files_are_physical_copies"];
const value = (text, key) => text.match(new RegExp(`^${key}\\s*=\\s*(.+)$`, "m"))?.[1];
const unquote = (text) => JSON.parse(text);
const ascii = (buffer, label) => assert(!buffer.some((byte) => byte > 0x7f), `${label} is not ASCII.`);

assert.equal(expectedIds.length, 64, "The Rust registry must contain 64 targets.");
assert.equal(new Set(expectedIds).size, 64, "The Rust registry IDs must be unique.");
assert.equal(manifests.length, 64, "Expected exactly 64 adapter manifests.");
assert.deepEqual(manifests.map((name) => name.slice(0, -5)), expectedIds, "Manifest names must exactly match registry IDs.");

for (const name of manifests) {
  const path = join(manifestDir, name);
  const buffer = await readFile(path);
  ascii(buffer, path);
  const text = buffer.toString("ascii");
  for (const key of requiredKeys) assert(value(text, key) !== undefined, `${name} is missing ${key}.`);
  const id = unquote(value(text, "id"));
  assert.equal(name, `${id}.toml`);
  assert.equal(unquote(value(text, "default_mode")), "SHADOW");
  assert.equal(unquote(value(text, "capability_grade")), "UNVERIFIED");
  assert.equal(unquote(value(text, "verification_status")), "UNVERIFIED");
  for (const operation of ["read", "write", "edit", "shell"]) assert.equal(unquote(value(text, operation)), "UNVERIFIED");
  assert.equal(value(text, "live_verified"), "false");
  assert.equal(unquote(value(text, "unknown_version_mode")), "SHADOW");
  assert.equal(unquote(value(text, "deployment")), "COPY");
  assert.equal(value(text, "policy_files_are_physical_copies"), "true");
  assert(!/^verified_(at|date)\s*=/m.test(text), `${name} must not invent verification dates.`);

  const fixtureDir = join(root, "fixtures", "contracts", id, "current");
  const files = (await readdir(fixtureDir)).filter((file) => file.endsWith(".json")).sort();
  assert.deepEqual(files, fixtureNames.map((fixture) => `${fixture}.json`).sort(), `${id} fixture set is incomplete.`);
  for (const fixture of fixtureNames) {
    const fixturePath = join(fixtureDir, `${fixture}.json`);
    const fixtureBuffer = await readFile(fixturePath);
    ascii(fixtureBuffer, fixturePath);
    const data = JSON.parse(fixtureBuffer.toString("ascii"));
    assert.equal(data.target_id, id);
    assert.equal(data.case, fixture);
    assert.equal(data.synthetic, true);
    assert.equal(data.live_verified, false);
    assert(data.input && data.expected, `${fixturePath} must contain input and expected data.`);
  }
  const unknown = JSON.parse(await readFile(join(fixtureDir, "unknown-version.json"), "utf8"));
  assert.deepEqual(unknown.expected, { capability_grade: "UNVERIFIED", mode: "SHADOW" });
  const install = JSON.parse(await readFile(join(fixtureDir, "install.json"), "utf8"));
  assert.equal(install.input.dry_run, true);
  assert.equal(install.expected.deployment, "COPY");
  const full = JSON.parse(await readFile(join(fixtureDir, "block-full.json"), "utf8"));
  const partial = JSON.parse(await readFile(join(fixtureDir, "block-partial.json"), "utf8"));
  assert.equal(full.expected.original_operation_executes, false);
  assert.equal(partial.expected.bounded_output, true);
}

async function rejectLinks(path) {
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = join(path, entry.name);
    assert(!(await lstat(child)).isSymbolicLink(), `${child} must not be a symbolic link.`);
    if (entry.isDirectory()) await rejectLinks(child);
  }
}
await rejectLinks(root);

console.log("Validated 64 unique UNVERIFIED/SHADOW adapter manifests, 704 ASCII fixtures, and physical-copy destinations.");
