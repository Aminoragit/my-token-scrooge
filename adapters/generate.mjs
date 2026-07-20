import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const source = await readFile(join(root, "..", "crates", "mts-harnesses", "src", "lib.rs"), "utf8");
const pattern = /target!\(\s*"([^"]+)",\s*"([^"]+)",\s*"([^"]+)",\s*\[([^\]]+)\],\s*"([^"]+)",\s*([A-Z]+),\s*\[([^\]]*)\],\s*\[([^\]]*)\]\s*\)/g;
const familyNames = {
  NativeHook: "NATIVE_HOOK",
  NativePlugin: "NATIVE_PLUGIN",
  SdkMiddleware: "SDK_MIDDLEWARE",
  AcpProxy: "ACP_PROXY",
  ProcessWrapper: "PROCESS_WRAPPER",
  SandboxWorkspace: "SANDBOX_WORKSPACE",
  IdeCompanion: "IDE_COMPANION",
  RepoBootstrap: "REPO_BOOTSTRAP",
  TelemetryProxy: "TELEMETRY_PROXY",
  CustomCommand: "CUSTOM_COMMAND",
};
const plannedCapabilities = {
  S: "STRICT",
  G: "STRONG",
  P: "PARTIAL",
  A: "ADVISORY",
  GP: "STRONG or PARTIAL",
  PA: "PARTIAL or ADVISORY",
  AP: "ADVISORY or PARTIAL",
};
const quoted = (value) => JSON.stringify(value);
const stringList = (value) => [...value.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
const tomlList = (values) => `[${values.map(quoted).join(", ")}]`;
const targets = [...source.matchAll(pattern)].map((match) => ({
  id: match[1],
  displayName: match[2],
  executionSurface: match[3],
  families: match[4].split(",").map((value) => familyNames[value.trim()]),
  installationForm: match[5],
  plannedCapability: plannedCapabilities[match[6]],
  commands: stringList(match[7]),
  markers: stringList(match[8]),
}));

if (targets.length !== 64 || targets.some((target) => !target.families.every(Boolean) || !target.plannedCapability)) {
  throw new Error(`Registry parse failed: expected 64 complete targets, found ${targets.length}.`);
}

await Promise.all([rm(join(root, "manifests"), { recursive: true, force: true }), rm(join(root, "fixtures"), { recursive: true, force: true })]);
await Promise.all([mkdir(join(root, "manifests"), { recursive: true }), mkdir(join(root, "fixtures", "contracts"), { recursive: true })]);

const fixtureCases = (target) => ({
  detection: {
    input: { commands: target.commands, markers: target.markers },
    expected: { status: "UNVERIFIED", modifies_files: false },
  },
  version: {
    input: { version_output: "unknown" },
    expected: { capability_grade: "UNVERIFIED", mode: "SHADOW" },
  },
  install: {
    input: { dry_run: true },
    expected: { action: "PREVIEW", default_mode: "SHADOW", deployment: "COPY" },
  },
  allow: {
    input: { operation: "read", resource: "README.md" },
    expected: { decision: "ALLOW", original_operation_executes: true },
  },
  "block-full": {
    input: { operation: "read", resource: "node_modules/package/index.js" },
    expected: { decision: "FULL_BLOCK", original_operation_executes: false },
  },
  "block-partial": {
    input: { operation: "read", resource: "logs/build.log" },
    expected: { decision: "PARTIAL_BLOCK", original_operation_executes: false, bounded_output: true },
  },
  malformed: {
    input: { payload: "{" },
    expected: { decision: "SAFE_ERROR", original_operation_executes: false },
  },
  timeout: {
    input: { elapsed_ms: 30001 },
    expected: { decision: "SAFE_ERROR", original_operation_executes: false },
  },
  "unknown-version": {
    input: { detected_version: "unknown" },
    expected: { capability_grade: "UNVERIFIED", mode: "SHADOW" },
  },
  uninstall: {
    input: { owned_files_only: true },
    expected: { removes_owned_files: true, preserves_unowned_files: true },
  },
  doctor: {
    input: {},
    expected: {
      detection: "UNVERIFIED",
      version: "unknown",
      integration: "UNVERIFIED",
      policy_files: "not installed",
      recommended_mode: "SHADOW",
    },
  },
});

await Promise.all(targets.flatMap(async (target) => {
  const fixtureDir = join(root, "fixtures", "contracts", target.id, "current");
  await mkdir(fixtureDir, { recursive: true });
  const manifest = [
    "# Generated from crates/mts-harnesses/src/lib.rs by adapters/generate.mjs.",
    "schema_version = 1",
    `id = ${quoted(target.id)}`,
    `display_name = ${quoted(target.displayName)}`,
    'vendor = "unknown"',
    `execution_surface = ${quoted(target.executionSurface)}`,
    `families = ${tomlList(target.families)}`,
    `commands = ${tomlList(target.commands)}`,
    `markers = ${tomlList(target.markers)}`,
    `installation_form = ${quoted(target.installationForm)}`,
    `policy_dir = ${quoted(`~/.mts/harnesses/${target.id}`)}`,
    'minimum_mts_version = "0.1.0"',
    'owner = "MTS adapter maintainers"',
    'source = "crates/mts-harnesses/src/lib.rs"',
    'default_mode = "SHADOW"',
    'capability_grade = "UNVERIFIED"',
    'verification_status = "UNVERIFIED"',
    `planned_capability = ${quoted(target.plannedCapability)}`,
    "install_dry_run = true",
    "install = true",
    "uninstall = true",
    'deployment = "COPY"',
    'owned_files = ["block-full.txt", "block-partial.txt", "adapter.json", "install-manifest.json"]',
    `fixture_dir = ${quoted(`fixtures/contracts/${target.id}/current`)}`,
    'doctor_template = "Detection: {detection}\\nVersion: {version}\\nIntegration: {integration}\\nPolicy files: {policy_files}\\nRecommended mode: SHADOW"',
    "",
    "[probe]",
    'version_args = ["--version"]',
    'help_args = ["--help"]',
    `contract = ${quoted(`fixtures/contracts/${target.id}/current`)}`,
    "",
    "[capabilities]",
    'read = "UNVERIFIED"',
    'write = "UNVERIFIED"',
    'edit = "UNVERIFIED"',
    'shell = "UNVERIFIED"',
    "rewrite_input = false",
    "replace_output = false",
    "remote_executor = false",
    "",
    "[honesty]",
    "live_verified = false",
    'unknown_version_mode = "SHADOW"',
    "mcp_is_enforcement_boundary = false",
    "unknown_internal_tools = true",
    "policy_files_are_physical_copies = true",
    "",
  ].join("\n");
  const writes = [writeFile(join(root, "manifests", `${target.id}.toml`), manifest, "ascii")];
  for (const [name, body] of Object.entries(fixtureCases(target))) {
    writes.push(writeFile(join(fixtureDir, `${name}.json`), `${JSON.stringify({ target_id: target.id, case: name, synthetic: true, live_verified: false, ...body }, null, 2)}\n`, "ascii"));
  }
  return Promise.all(writes);
}));

console.log(`Generated ${targets.length} adapter manifests and ${targets.length * 11} contract fixtures.`);
