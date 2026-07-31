#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const executable = process.platform === "win32" ? "mts.exe" : "mts";
const candidates = [join(repo, "target", "release", executable), join(repo, "target", "debug", executable)];
const mts = process.env.MTS_BINARY || candidates.find(existsSync) || "mts";
const scenarios = [
  { id: "dependency-read", kind: "read", path: "node_modules/pkg/large.js", bytes: 1_048_576, expected: "deny", protected: true },
  { id: "python-cache-read", kind: "read", path: "__pycache__/fixture.pyc", bytes: 524_288, expected: "deny", protected: true },
  { id: "git-object-read", kind: "read", path: ".git/objects/aa/object", bytes: 524_288, expected: "deny", protected: true },
  { id: "dependency-edit", kind: "edit", path: "node_modules/pkg/edit.js", expected: "deny", protected: true },
  { id: "distribution-edit", kind: "edit", path: "dist/edit.js", expected: "deny", protected: true },
  { id: "source-read-control", kind: "read", path: "src/large.js", bytes: 65_536, expected: "deny", protected: false },
  { id: "source-edit-control", kind: "edit", path: "src/edit.js", expected: "allow", protected: false }
];

if (process.argv.includes("--validate")) {
  if (new Set(scenarios.map(({ id }) => id)).size !== scenarios.length) throw new Error("Duplicate scenario ID");
  console.log(`Valid hook evidence suite: ${scenarios.length} scenarios`);
  process.exit(0);
}

const runRoot = join(repo, "benchmarks", "results", `hook-evidence-${randomUUID()}`);
const baselineRoot = join(runRoot, "fixtures", "baseline");
const enforceRoot = join(runRoot, "fixtures", "enforce");
const env = { ...process.env, MTS_HOME: join(runRoot, "mts-home"), NO_COLOR: "1" };

function run(args, { cwd = repo, input } = {}) {
  const result = spawnSync(mts, args, { cwd, env, input, encoding: "utf8", windowsHide: true });
  if (result.error || result.status !== 0) {
    throw new Error(`${mts} ${args.join(" ")} failed: ${result.error?.message || result.stderr}`);
  }
  return result.stdout.trim();
}

function write(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, value);
}

function fixture(root, scenario) {
  const path = join(root, scenario.path);
  write(path, scenario.kind === "read" ? Buffer.alloc(scenario.bytes, 120) : "export const marker = 'ORIGINAL';\n");
}

function report() {
  return JSON.parse(run(["report", "export", "--format", "json"]));
}

function difference(after, before) {
  return Object.fromEntries(Object.keys(after).map((key) => [key, after[key] - before[key]]));
}

function hookPayload(scenario) {
  const command = scenario.kind === "read"
    ? process.platform === "win32" ? `Get-Content ${scenario.path.replaceAll("/", "\\")}` : `cat ${scenario.path}`
    : `*** Begin Patch\n*** Update File: ${scenario.path}\n@@\n-ORIGINAL\n+CHANGED\n*** End Patch`;
  return JSON.stringify({
    session_id: `hook-evidence-${scenario.id}`,
    hook_event_name: "PreToolUse",
    tool_name: scenario.kind === "read" ? "Bash" : "apply_patch",
    tool_input: { command }
  });
}

function mutate(path) {
  const original = readFileSync(path, "utf8");
  writeFileSync(path, original.replace("ORIGINAL", "CHANGED"));
  return readFileSync(path, "utf8").includes("CHANGED");
}

for (const scenario of scenarios) {
  fixture(baselineRoot, scenario);
  fixture(enforceRoot, scenario);
}

run(["setup", "--targets", "codex-cli", "--yes", "--codex-home", join(runRoot, "codex-home")]);
run(["mode", "warn"]);
run(["mode", "enforce"]);
run(["doctor"]);

const rows = [];
for (const scenario of scenarios) {
  const baselinePath = join(baselineRoot, scenario.path);
  const enforcePath = join(enforceRoot, scenario.path);
  const baselineBytes = scenario.kind === "read" ? readFileSync(baselinePath).length : 0;
  const baselineMutated = scenario.kind === "edit" ? mutate(baselinePath) : null;
  const before = report();
  const hook = JSON.parse(run(["hook", "codex-cli"], { cwd: enforceRoot, input: hookPayload(scenario) }));
  const decision = hook.hookSpecificOutput.permissionDecision;
  const reason = hook.hookSpecificOutput.permissionDecisionReason || "";
  const policyDecision = decision === "allow" ? "ALLOW"
    : reason.includes("MTS_POLICY_FULL_BLOCK") ? "FULL_BLOCK" : "PARTIAL_BLOCK";
  const context = hook.hookSpecificOutput.additionalContext || "";
  const substituteBytes = Buffer.byteLength(context);
  const enforcedContextBytes = scenario.kind === "read"
    ? decision === "allow" ? readFileSync(enforcePath).length : substituteBytes + Buffer.byteLength(reason)
    : 0;
  const enforceMutated = scenario.kind === "edit" && decision === "allow" ? mutate(enforcePath) : false;
  const metrics = difference(report(), before);
  if (decision !== scenario.expected) throw new Error(`${scenario.id}: expected ${scenario.expected}, got ${decision}`);
  if (decision === "deny" && !reason.includes("Do not retry or work around this block")) {
    throw new Error(`${scenario.id}: denial is missing no-retry guidance`);
  }
  if (policyDecision === "PARTIAL_BLOCK" && !context.startsWith("MTS guidance:")) {
    throw new Error(`${scenario.id}: bounded result is missing policy guidance`);
  }
  if (scenario.kind === "read" && decision === "deny" && metrics.avoided_output_bytes !== baselineBytes) {
    throw new Error(`${scenario.id}: avoided bytes do not match baseline output`);
  }
  if (scenario.kind === "edit" && (!baselineMutated || enforceMutated !== (decision === "allow"))) {
    throw new Error(`${scenario.id}: mutation outcome does not match hook decision`);
  }
  rows.push({
    ...scenario,
    baseline_output_bytes: baselineBytes,
    baseline_mutated: baselineMutated,
    decision,
    policy_decision: policyDecision,
    permission_reason: reason,
    guidance: context.split("\n\n", 1)[0],
    substitute_output_bytes: substituteBytes,
    enforced_context_bytes: enforcedContextBytes,
    enforce_mutated: enforceMutated,
    context_adjusted_estimated_tokens_saved: scenario.kind === "read"
      ? Math.floor(Math.max(0, baselineBytes - enforcedContextBytes) / 4) : 0,
    ...metrics
  });
}

const protectedReads = rows.filter((row) => row.kind === "read" && row.protected);
const protectedEdits = rows.filter((row) => row.kind === "edit" && row.protected);
const controls = rows.filter((row) => !row.protected);
const totals = {
  protected_read_baseline_bytes: protectedReads.reduce((sum, row) => sum + row.baseline_output_bytes, 0),
  protected_read_enforced_context_bytes: protectedReads.reduce((sum, row) => sum + row.enforced_context_bytes, 0),
  avoided_output_bytes: protectedReads.reduce((sum, row) => sum + row.avoided_output_bytes, 0),
  replacement_output_bytes: protectedReads.reduce((sum, row) => sum + row.replacement_output_bytes, 0),
  estimated_net_tokens_saved: protectedReads.reduce((sum, row) => sum + row.estimated_net_tokens_saved, 0),
  context_adjusted_estimated_tokens_saved: protectedReads.reduce((sum, row) => sum + row.context_adjusted_estimated_tokens_saved, 0),
  protected_edits_prevented: protectedEdits.filter(({ enforce_mutated }) => !enforce_mutated).length,
  protected_edits_tested: protectedEdits.length,
  functional_control_failures: controls.filter((row) => row.kind === "edit"
    ? row.decision !== "allow" : row.substitute_output_bytes < row.baseline_output_bytes).length,
  control_policy_denials: controls.filter(({ decision }) => decision === "deny").length,
  controls: controls.length
};
totals.output_reduction_percent = 100
  * (1 - totals.protected_read_enforced_context_bytes / totals.protected_read_baseline_bytes);

const version = run(["--version"]);
run(["uninstall", "--targets", "codex-cli"]);
const codexHooks = join(runRoot, "codex-home", "hooks.json");
if (existsSync(codexHooks) && readFileSync(codexHooks, "utf8").includes("mts hook codex-cli")) {
  throw new Error("Codex uninstall left the MTS hook installed");
}
const result = {
  schema_version: 1,
  created_at: new Date().toISOString(),
  method: "real Codex PreToolUse hook; deterministic-byte-range-v1 at 4 bytes/token, LOW confidence",
  platform: `${process.platform}-${process.arch}`,
  mts: version,
  rows,
  totals
};
write(join(runRoot, "report.json"), `${JSON.stringify(result, null, 2)}\n`);

const table = rows.map((row) => {
  const baseline = row.kind === "read" ? `${row.baseline_output_bytes} B` : row.baseline_mutated ? "mutated" : "unchanged";
  const enforced = row.kind === "read" ? `${row.enforced_context_bytes} B` : row.enforce_mutated ? "mutated" : "unchanged";
  return `| ${row.id} | ${baseline} | ${row.policy_decision} | ${enforced} | ${row.avoided_output_bytes} | ${row.context_adjusted_estimated_tokens_saved} |`;
}).join("\n");
const markdown = `# Real Codex hook waste-prevention evidence\n\n` +
  `Run: \`${relative(repo, runRoot).replaceAll("\\", "/")}\`  \n` +
  `Method: ${result.method}. Token values are estimates, not billed API-token measurements.\n\n` +
  `| Scenario | Baseline | ENFORCE | Enforced result/context | Avoided bytes | Context-adjusted estimated tokens saved |\n` +
  `|---|---:|---:|---:|---:|---:|\n${table}\n\n` +
  `| Aggregate | Result |\n|---|---:|\n` +
  `| Protected-read output reduction | ${totals.output_reduction_percent.toFixed(2)}% |\n` +
  `| Context-adjusted estimated tokens saved | ${totals.context_adjusted_estimated_tokens_saved} |\n` +
  `| MTS ledger estimated tokens saved | ${totals.estimated_net_tokens_saved} |\n` +
  `| Protected edits prevented | ${totals.protected_edits_prevented}/${totals.protected_edits_tested} |\n` +
  `| Functional control failures | ${totals.functional_control_failures}/${totals.controls} |\n` +
  `| Control policy denials | ${totals.control_policy_denials}/${totals.controls} |\n`;
write(join(runRoot, "comparison.md"), markdown);
console.log(JSON.stringify({ run_root: runRoot, totals }, null, 2));
