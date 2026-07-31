#!/usr/bin/env node

import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { createHash, randomUUID } from "node:crypto";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const defaultSuite = join(repoRoot, "benchmarks", "codex-20.json");
const resultsRoot = join(repoRoot, "benchmarks", "results");
const scenarios = new Set([
  "bounded-read",
  "package-manifest",
  "type-declaration",
  "error-log",
  "minified-symbol",
  "small-generated-read",
  "safe-edit",
  "full-block-read",
  "protected-edit"
]);

const checkerSource = `import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const task = JSON.parse(readFileSync(".benchmark-task.json", "utf8"));
const arm = process.env.BENCHMARK_ARM;
const errors = [];
const read = (path) => {
  try { return readFileSync(path, "utf8"); }
  catch (error) { errors.push(\`${"${path}"}: ${"${error.message}"}\`); return ""; }
};

if (task.scenario === "safe-edit") {
  try {
    const module = await import(pathToFileURL("src/math.mjs").href + \`?check=${"${Date.now()}"}\`);
    if (module.add(2, 3) !== 5 || module.add(-4, 1) !== -3) errors.push("add() is not fixed");
  } catch (error) { errors.push(\`src/math.mjs: ${"${error.message}"}\`); }
} else if (task.scenario === "protected-edit") {
  const expected = arm === "enforce" ? task.fixture.expected_enforce : task.fixture.expected_baseline;
  const source = read("node_modules/example/index.js");
  const wanted = arm === "enforce" ? "ORIGINAL-901" : "CHANGED-901";
  if (!source.includes(wanted)) errors.push(\`protected file does not contain ${"${wanted}"}\`);
  if (read("answer.txt").trim() !== expected) errors.push(\`answer.txt must equal ${"${expected}"}\`);
} else {
  const expected = arm === "enforce" && task.fixture.expected_enforce
    ? task.fixture.expected_enforce
    : task.fixture.expected_baseline || task.fixture.expected;
  if (read("answer.txt").trim() !== expected) errors.push(\`answer.txt must equal ${"${expected}"}\`);
}

console.log(JSON.stringify({ passed: errors.length === 0, errors }));
if (errors.length) process.exitCode = 1;
`;

function parseArgs(argv) {
  const options = {
    cleanup: false,
    validate: false,
    resume: null,
    retryInfrastructure: false,
    suite: defaultSuite,
    timeoutMs: 900_000
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--cleanup") options.cleanup = true;
    else if (argument === "--validate") options.validate = true;
    else if (argument === "--suite") options.suite = resolve(repoRoot, argv[++index] || "");
    else if (argument === "--resume") options.resume = argv[++index] || "";
    else if (argument === "--retry-infrastructure") options.retryInfrastructure = true;
    else if (argument === "--timeout-ms") options.timeoutMs = Number(argv[++index]);
    else throw new Error(`Unknown argument: ${argument}`);
  }
  if (!Number.isSafeInteger(options.timeoutMs) || options.timeoutMs < 1_000) {
    throw new Error("--timeout-ms must be an integer of at least 1000");
  }
  return options;
}

function loadSuite(path) {
  const raw = readFileSync(path, "utf8");
  const suite = JSON.parse(raw);
  if (suite?.schema_version !== 1 || suite?.suite !== "codex-20" || !Array.isArray(suite.tasks)) {
    throw new Error("Suite must use schema_version 1 and suite codex-20");
  }
  if (suite.tasks.length !== 20) throw new Error("Suite must contain exactly 20 tasks");
  const ids = new Set();
  for (const task of suite.tasks) {
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(task?.id || "") || ids.has(task.id)) {
      throw new Error(`Task ID is missing, invalid, or duplicated: ${task?.id}`);
    }
    ids.add(task.id);
    if (!scenarios.has(task.scenario)) throw new Error(`Unknown scenario for ${task.id}`);
    if (typeof task.prompt !== "string" || !task.prompt.trim()) throw new Error(`Missing prompt for ${task.id}`);
    if (!task.fixture || !Number.isSafeInteger(task.fixture.seed)) throw new Error(`Invalid fixture for ${task.id}`);
    const fixture = task.fixture;
    const expected = typeof fixture.expected === "string" && fixture.expected.length > 0;
    if (["bounded-read", "type-declaration", "error-log", "minified-symbol"].includes(task.scenario)) {
      if (!Number.isSafeInteger(fixture.lines) || fixture.lines < 1001 || !Number.isSafeInteger(fixture.marker_line)
        || fixture.marker_line < 1 || fixture.marker_line > fixture.lines || !expected) {
        throw new Error(`Invalid line fixture for ${task.id}`);
      }
    } else if (task.scenario === "package-manifest") {
      if (!Number.isSafeInteger(fixture.lines) || fixture.lines < 1 || !expected) throw new Error(`Invalid package fixture for ${task.id}`);
    } else if (task.scenario === "small-generated-read" && !expected) {
      throw new Error(`Invalid generated fixture for ${task.id}`);
    } else if (["full-block-read", "protected-edit"].includes(task.scenario)) {
      if (typeof fixture.expected_baseline !== "string" || typeof fixture.expected_enforce !== "string") {
        throw new Error(`Invalid protected fixture for ${task.id}`);
      }
    }
  }
  return { suite, raw };
}

function resolveCommand(value, candidates, fallback) {
  if (value) {
    return isAbsolute(value) || value.includes("/") || value.includes("\\") ? resolve(repoRoot, value) : value;
  }
  return candidates.find(existsSync) || fallback;
}

function resolveWindowsNativeExecutable(command) {
  if (process.platform !== "win32" || command.toLowerCase().endsWith(".exe")) return command;
  if (isAbsolute(command) || command.includes("/") || command.includes("\\")) {
    return existsSync(`${command}.exe`) ? `${command}.exe` : command;
  }
  const appCodex = process.env.LOCALAPPDATA
    ? join(process.env.LOCALAPPDATA, "OpenAI", "Codex", "bin", "codex.exe")
    : null;
  if (command.toLowerCase() === "codex" && appCodex && existsSync(appCodex)) return appCodex;
  const lookup = spawnSync("where.exe", [command], { encoding: "utf8", windowsHide: true });
  if (lookup.status !== 0) return command;
  return lookup.stdout.split(/\r?\n/).find((path) => path.toLowerCase().endsWith(".exe")) || command;
}

function resolveWindowsCodexJs() {
  if (process.platform !== "win32") return null;
  const lookup = spawnSync("where.exe", ["codex"], { encoding: "utf8", windowsHide: true });
  if (lookup.status !== 0) return null;
  for (const command of lookup.stdout.split(/\r?\n/).filter(Boolean)) {
    const candidate = join(dirname(command), "node_modules", "@openai", "codex", "bin", "codex.js");
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

function write(path, contents) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
}

function lineFixture(lines, markerLine, marker, filler) {
  return Array.from({ length: lines }, (_, index) =>
    index + 1 === markerLine ? marker : `${filler}-${String(index + 1).padStart(5, "0")}`
  ).join("\n") + "\n";
}

function createFixture(workspace, task) {
  const fixture = task.fixture;
  write(join(workspace, ".benchmark-task.json"), JSON.stringify(task, null, 2) + "\n");
  write(join(workspace, "check.mjs"), checkerSource);
  switch (task.scenario) {
    case "bounded-read":
      write(join(workspace, "data", "catalog.txt"), lineFixture(fixture.lines, fixture.marker_line, `TARGET=${fixture.expected}`, `record-${fixture.seed}`));
      break;
    case "package-manifest": {
      const padding = Array.from({ length: fixture.lines }, (_, index) => `  "padding_${index}": "${fixture.seed}-${index}"`);
      write(join(workspace, "node_modules", "example", "package.json"), `{\n  "name": "example",\n  "version": "${fixture.expected}",\n${padding.join(",\n")}\n}\n`);
      break;
    }
    case "type-declaration":
      write(join(workspace, "node_modules", "example", "index.d.ts"), lineFixture(fixture.lines, fixture.marker_line, `export type TargetType = "${fixture.expected}";`, `export type Padding${fixture.seed}`));
      break;
    case "error-log":
      write(join(workspace, "logs", "app.log"), lineFixture(fixture.lines, fixture.marker_line, `2026-01-01T00:00:00Z ERROR code=${fixture.expected} root failure`, "2026-01-01T00:00:00Z INFO heartbeat"));
      break;
    case "minified-symbol":
      write(join(workspace, "public", "app.min.js"), lineFixture(fixture.lines, fixture.marker_line, `globalThis.TARGET_SYMBOL="${fixture.expected}";`, "var p"));
      break;
    case "small-generated-read":
      write(join(workspace, "dist", "manifest.json"), JSON.stringify({ build_id: fixture.expected, files: ["app.js"] }, null, 2) + "\n");
      break;
    case "safe-edit":
      write(join(workspace, "src", "math.mjs"), "export function add(left, right) {\n  return left - right;\n}\n");
      break;
    case "full-block-read":
      write(join(workspace, "__pycache__", "fixture.pyc"), `${fixture.expected_baseline}\ncompiled-cache-${fixture.seed}\n`);
      break;
    case "protected-edit":
      write(join(workspace, "node_modules", "example", "index.js"), `export const marker = "ORIGINAL-${fixture.seed}";\n`);
      break;
  }
}

function hashFile(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function fixtureHash(workspace) {
  const hash = createHash("sha256");
  const visit = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      if (entry.name === ".codex") continue;
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`Fixture contains a symbolic link: ${path}`);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile()) {
        hash.update(relative(workspace, path).replaceAll("\\", "/"));
        hash.update("\0");
        hash.update(readFileSync(path));
        hash.update("\0");
      }
    }
  };
  visit(workspace);
  return hash.digest("hex");
}

function shellQuote(value) {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function powershellQuote(value) {
  return `'${value.replaceAll("'", "''")}'`;
}

function writeHook(workspace, mtsCommand) {
  write(join(workspace, ".codex", "hooks.json"), JSON.stringify({
    hooks: {
      PreToolUse: [{
        matcher: "^(Bash|apply_patch|Edit|Write)$",
        hooks: [{
          type: "command",
          command: `${shellQuote(mtsCommand)} hook codex-cli`,
          commandWindows: `& ${powershellQuote(mtsCommand)} hook codex-cli`,
          timeout: 30,
          statusMessage: "Applying my-token-scrooge policy..."
        }]
      }]
    }
  }, null, 2) + "\n");
}

function runProcess(command, args, { cwd, env, stdoutPath, stderrPath, timeoutMs }) {
  return new Promise((resolvePromise) => {
    const stdout = openSync(stdoutPath, "w");
    const stderr = openSync(stderrPath, "w");
    const started = process.hrtime.bigint();
    let timedOut = false;
    let spawnError = null;
    const child = spawn(command, args, { cwd, env, stdio: ["ignore", stdout, stderr], windowsHide: true });
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill();
    }, timeoutMs);
    child.on("error", (error) => { spawnError = error.message; });
    child.on("close", (exitCode, signal) => {
      clearTimeout(timer);
      closeSync(stdout);
      closeSync(stderr);
      resolvePromise({
        exit_code: exitCode,
        signal,
        timed_out: timedOut,
        spawn_error: spawnError,
        wall_ms: Number(process.hrtime.bigint() - started) / 1_000_000
      });
    });
  });
}

async function runText(command, args, directory, env, prefix, timeoutMs, cwd = repoRoot) {
  const stdoutPath = join(directory, `${prefix}.stdout.txt`);
  const stderrPath = join(directory, `${prefix}.stderr.txt`);
  const processResult = await runProcess(command, args, { cwd, env, stdoutPath, stderrPath, timeoutMs });
  return {
    ...processResult,
    stdout: readFileSync(stdoutPath, "utf8"),
    stderr: readFileSync(stderrPath, "utf8")
  };
}

function parseCodexJsonl(path) {
  const tokens = { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, total_tokens: 0 };
  const toolCounts = {};
  let malformed_lines = 0;
  for (const line of readFileSync(path, "utf8").split(/\r?\n/).filter(Boolean)) {
    let event;
    try { event = JSON.parse(line); }
    catch { malformed_lines += 1; continue; }
    if (event.type === "turn.completed" && event.usage) {
      tokens.input_tokens += Number(event.usage.input_tokens || 0);
      tokens.cached_input_tokens += Number(event.usage.cached_input_tokens || event.usage.input_tokens_details?.cached_tokens || 0);
      tokens.output_tokens += Number(event.usage.output_tokens || 0);
    }
    if (event.type === "item.completed" && event.item?.type
      && !["agent_message", "reasoning", "error"].includes(event.item.type)) {
      toolCounts[event.item.type] = (toolCounts[event.item.type] || 0) + 1;
    }
  }
  tokens.total_tokens = tokens.input_tokens + tokens.output_tokens;
  return { tokens, tool_counts: toolCounts, tool_total: Object.values(toolCounts).reduce((sum, count) => sum + count, 0), malformed_lines };
}

function parseJsonOutput(text) {
  try { return JSON.parse(text); }
  catch { return null; }
}

function parseRetries(text) {
  return text.split(/\r?\n/).filter(Boolean).map((line) => {
    const [intent, attempts, state] = line.split("\t");
    return { intent, attempts: Number(attempts), state };
  });
}

async function runArm(task, arm, armRoot, commands, timeoutMs) {
  const workspace = join(armRoot, "workspace");
  const mtsHome = join(armRoot, "mts-home");
  mkdirSync(workspace, { recursive: true });
  mkdirSync(mtsHome, { recursive: true });
  createFixture(workspace, task);
  const fixture_hash = fixtureHash(workspace);
  const protectedHashes = {
    checker: hashFile(join(workspace, "check.mjs")),
    task: hashFile(join(workspace, ".benchmark-task.json"))
  };
  const env = { ...process.env, MTS_HOME: mtsHome, MTS_BINARY: commands.mts, NO_COLOR: "1" };
  const setup = [];
  if (arm === "enforce") {
    setup.push(await runText(commands.mts, ["setup", "--profile", "balanced", "--targets", "codex-cli", "--yes", "--codex-home", join(workspace, ".codex")], armRoot, env, "mts-setup", timeoutMs));
    if (setup[0].exit_code === 0) setup.push(await runText(commands.mts, ["mode", "warn"], armRoot, env, "mts-mode-warn", timeoutMs));
    if (setup.at(-1).exit_code === 0) setup.push(await runText(commands.mts, ["mode", "enforce"], armRoot, env, "mts-mode-enforce", timeoutMs));
    writeHook(workspace, commands.mts);
  } else {
    write(join(workspace, ".codex", "hooks.json"), "{\n  \"hooks\": {}\n}\n");
  }

  const jsonlPath = join(armRoot, "codex.jsonl");
  const stderrPath = join(armRoot, "codex.stderr.txt");
  const finalPath = join(armRoot, "codex.final.txt");
  const setupPassed = setup.every((entry) => entry.exit_code === 0);
  const hookFeatureArgs = arm === "enforce" ? ["--enable", "hooks"] : ["--disable", "hooks"];
  const codex = setupPassed ? await runProcess(commands.codex, [
    ...commands.codex_args,
    "exec",
    ...hookFeatureArgs,
    "--ephemeral",
    "--ignore-rules",
    "--skip-git-repo-check",
    "--dangerously-bypass-hook-trust",
    "--sandbox", "workspace-write",
    "-c", "approval_policy=\"never\"",
    "--json",
    "--output-last-message", finalPath,
    "-C", workspace,
    task.prompt
  ], { cwd: workspace, env, stdoutPath: jsonlPath, stderrPath, timeoutMs }) : {
    exit_code: null, signal: null, timed_out: false, spawn_error: "MTS setup failed", wall_ms: 0
  };
  if (!existsSync(jsonlPath)) write(jsonlPath, "");
  if (!existsSync(stderrPath)) write(stderrPath, "");
  if (!existsSync(finalPath)) write(finalPath, "");

  const checkerPath = join(workspace, "check.mjs");
  const taskPath = join(workspace, ".benchmark-task.json");
  const integrity = existsSync(checkerPath) && existsSync(taskPath)
    && protectedHashes.checker === hashFile(checkerPath)
    && protectedHashes.task === hashFile(taskPath);
  const checker = await runText(process.execPath, [join(workspace, "check.mjs")], armRoot, { ...env, BENCHMARK_ARM: arm }, "checker", timeoutMs, workspace);
  const checkerJson = parseJsonOutput(checker.stdout.trim());
  const report = await runText(commands.mts, ["report", "export", "--format", "json"], armRoot, env, "mts-report", timeoutMs);
  const retries = await runText(commands.mts, ["retries", "list"], armRoot, env, "mts-retries", timeoutMs);
  const result = {
    task_id: task.id,
    arm,
    fixture_hash,
    setup: setup.map(({ stdout, stderr, ...entry }) => entry),
    codex,
    ...parseCodexJsonl(jsonlPath),
    final: readFileSync(finalPath, "utf8"),
    checker: {
      passed: integrity && codex.exit_code === 0 && checker.exit_code === 0 && checkerJson?.passed === true,
      integrity,
      exit_code: checker.exit_code,
      result: checkerJson
    },
    mts: {
      report: parseJsonOutput(report.stdout.trim()),
      report_exit_code: report.exit_code,
      retries: parseRetries(retries.stdout),
      retries_exit_code: retries.exit_code
    }
  };
  write(join(armRoot, "result.json"), JSON.stringify(result, null, 2) + "\n");
  return result;
}

function median(values) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function infrastructureFailure(runRoot, result) {
  if (result.codex.exit_code === 0) return null;
  if (result.codex.spawn_error) return "codex_spawn_error";
  const path = join(runRoot, "tasks", result.task_id, result.arm, "codex.jsonl");
  if (!existsSync(path)) return null;
  const messages = [];
  for (const line of readFileSync(path, "utf8").split(/\r?\n/).filter(Boolean)) {
    try {
      const event = JSON.parse(line);
      if (event.type === "turn.failed" || event.type === "error") {
        messages.push(String(event.error?.message || event.message || ""));
      }
    } catch {
      // Malformed lines remain product failures; raw evidence is retained for review.
    }
  }
  const message = messages.join("\n");
  if (/usage limit|rate limit|quota|insufficient_quota/i.test(message)) return "codex_usage_limit";
  if (/\b(?:401|429|500|502|503|504)\b|unauthorized|service unavailable/i.test(message)) {
    return "codex_service_error";
  }
  return null;
}

function summarize(results, runRoot) {
  const invalidArms = results.flatMap((result) => {
    const reason = infrastructureFailure(runRoot, result);
    return reason ? [{ task_id: result.task_id, arm: result.arm, reason }] : [];
  });
  const invalidKeys = new Set(invalidArms.map((row) => `${row.task_id}:${row.arm}`));
  const byArm = Object.fromEntries(["baseline", "enforce"].map((arm) => {
    const rows = results.filter((result) => result.arm === arm);
    const validRows = rows.filter((result) => !invalidKeys.has(`${result.task_id}:${result.arm}`));
    return [arm, {
      tasks: rows.length,
      valid_tasks: validRows.length,
      invalid_tasks: rows.length - validRows.length,
      passed: validRows.filter((row) => row.checker.passed).length,
      success_rate: validRows.length ? validRows.filter((row) => row.checker.passed).length / validRows.length : null,
      median_wall_ms: median(validRows.map((row) => row.codex.wall_ms)),
      median_tool_calls: median(validRows.map((row) => row.tool_total)),
      median_total_tokens: median(validRows.map((row) => row.tokens.total_tokens))
    }];
  }));
  const comparisons = [];
  const performanceComparisons = [];
  for (const taskId of new Set(results.map((result) => result.task_id))) {
    const baseline = results.find((result) => result.task_id === taskId && result.arm === "baseline");
    const enforce = results.find((result) => result.task_id === taskId && result.arm === "enforce");
    if (!baseline || !enforce) continue;
    if (invalidKeys.has(`${taskId}:baseline`) || invalidKeys.has(`${taskId}:enforce`)) continue;
    const comparison = {
      task_id: taskId,
      fixture_match: baseline.fixture_hash === enforce.fixture_hash,
      baseline_passed: baseline.checker.passed,
      enforce_passed: enforce.checker.passed,
      token_savings_ratio: baseline.tokens.total_tokens ? (baseline.tokens.total_tokens - enforce.tokens.total_tokens) / baseline.tokens.total_tokens : null,
      tool_call_ratio: baseline.tool_total ? enforce.tool_total / baseline.tool_total : null,
      retry_amplification: Math.max(1, ...enforce.mts.retries.map((row) => row.attempts || 0))
    };
    comparisons.push(comparison);
    if (baseline.codex.exit_code === 0 && enforce.codex.exit_code === 0
      && baseline.checker.passed && enforce.checker.passed) {
      performanceComparisons.push(comparison);
    }
  }
  return {
    status: invalidArms.length ? "incomplete_infrastructure_failure" : "complete",
    arms: byArm,
    valid_comparison_pairs: comparisons.length,
    performance_comparison_pairs: performanceComparisons.length,
    invalid_arms: invalidArms,
    comparisons,
    median_token_savings_ratio: median(performanceComparisons.map((row) => row.token_savings_ratio).filter(Number.isFinite)),
    median_tool_call_ratio: median(performanceComparisons.map((row) => row.tool_call_ratio).filter(Number.isFinite)),
    median_retry_amplification: median(performanceComparisons.map((row) => row.retry_amplification))
  };
}

function assertNoLinks(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink()) throw new Error(`Cleanup refused: symbolic link found at ${path}`);
    if (entry.isDirectory()) assertNoLinks(path);
  }
}

function safeCleanup(runRoot) {
  const runName = relative(resultsRoot, runRoot);
  if (!/^codex-20-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(runName)) {
    throw new Error(`Cleanup refused: invalid run directory ${runName}`);
  }
  for (const path of [repoRoot, join(repoRoot, "benchmarks"), resultsRoot, runRoot]) {
    const stats = lstatSync(path);
    if (!stats.isDirectory() || stats.isSymbolicLink()) throw new Error(`Cleanup refused: unsafe directory ${path}`);
  }
  const realResults = realpathSync(resultsRoot);
  const realRun = realpathSync(runRoot);
  if (dirname(realRun) !== realResults || !realRun.startsWith(realResults + sep)) {
    throw new Error("Cleanup refused: run is outside the benchmark results root");
  }
  assertNoLinks(runRoot);
  rmSync(runRoot, { recursive: true });
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const { suite, raw } = loadSuite(options.suite);
  if (options.validate) {
    console.log(`Valid suite: ${suite.tasks.length} tasks`);
    return;
  }

  const resumeName = options.resume;
  if (resumeName && !/^codex-20-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(resumeName)) {
    throw new Error(`Invalid resume run ID: ${resumeName}`);
  }
  const runId = resumeName ? resumeName.slice("codex-20-".length) : randomUUID();
  const runRoot = join(resultsRoot, `codex-20-${runId}`);
  if (!resumeName && existsSync(runRoot)) throw new Error(`Run directory already exists: ${runRoot}`);
  if (resumeName && (!existsSync(runRoot) || lstatSync(runRoot).isSymbolicLink())) {
    throw new Error(`Resume directory is missing or unsafe: ${runRoot}`);
  }
  mkdirSync(runRoot, { recursive: true });
  const codexJs = process.env.CODEX_JS
    ? resolveCommand(process.env.CODEX_JS, [], "")
    : process.env.CODEX_BINARY ? null : resolveWindowsCodexJs();
  const commands = {
    mts: resolveCommand(process.env.MTS_BINARY, [
      join(repoRoot, "target", "release", process.platform === "win32" ? "mts.exe" : "mts"),
      join(repoRoot, "target", "debug", process.platform === "win32" ? "mts.exe" : "mts")
    ], "mts"),
    codex: codexJs
      ? process.execPath
      : resolveWindowsNativeExecutable(resolveCommand(process.env.CODEX_BINARY, [], "codex")),
    codex_args: codexJs ? [codexJs] : []
  };
  const metadataEnv = { ...process.env, MTS_HOME: join(runRoot, "metadata-mts-home"), MTS_BINARY: commands.mts, NO_COLOR: "1" };
  const codexVersion = await runText(commands.codex, [...commands.codex_args, "--version"], runRoot, metadataEnv, "codex-version", options.timeoutMs);
  const mtsVersion = await runText(commands.mts, ["--version"], runRoot, metadataEnv, "mts-version", options.timeoutMs);
  const suiteHash = createHash("sha256").update(raw).digest("hex");
  const report = resumeName
    ? JSON.parse(readFileSync(join(runRoot, "report.json"), "utf8"))
    : {
      schema_version: 1,
      run_id: runId,
      created_at: new Date().toISOString(),
      suite: suite.suite,
      suite_hash: suiteHash,
      commands,
      versions: { codex: codexVersion.stdout.trim(), mts: mtsVersion.stdout.trim(), node: process.version },
      platform: { platform: process.platform, arch: process.arch },
      arm_order: [],
      results: []
    };
  if (report.run_id !== runId || report.suite_hash !== suiteHash || !Array.isArray(report.results)) {
    throw new Error("Resume report does not match this run or suite");
  }
  report.commands = commands;
  report.versions = { codex: codexVersion.stdout.trim(), mts: mtsVersion.stdout.trim(), node: process.version };
  write(join(runRoot, "report.json"), JSON.stringify(report, null, 2) + "\n");

  if (options.retryInfrastructure) {
    report.results = report.results.filter((result) => !infrastructureFailure(runRoot, result));
  }
  const completed = new Set(report.results.map((result) => `${result.task_id}:${result.arm}`));
  for (let index = 0; index < suite.tasks.length; index += 1) {
    const task = suite.tasks[index];
    const order = index % 2 === 0 ? ["baseline", "enforce"] : ["enforce", "baseline"];
    if (!report.arm_order.some((entry) => entry.task_id === task.id)) report.arm_order.push({ task_id: task.id, order });
    for (const arm of order) {
      if (completed.has(`${task.id}:${arm}`)) continue;
      const armRoot = join(runRoot, "tasks", task.id, arm);
      mkdirSync(armRoot, { recursive: true });
      report.results.push(await runArm(task, arm, armRoot, commands, options.timeoutMs));
      report.summary = summarize(report.results, runRoot);
      write(join(runRoot, "report.json"), JSON.stringify(report, null, 2) + "\n");
      console.log(`${task.id} ${arm}: ${report.results.at(-1).checker.passed ? "pass" : "fail"}`);
    }
  }

  report.summary = summarize(report.results, runRoot);
  write(join(runRoot, "report.json"), JSON.stringify(report, null, 2) + "\n");
  console.log(JSON.stringify({ run_root: runRoot, summary: report.summary }, null, 2));
  if (options.cleanup) safeCleanup(runRoot);
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exitCode = 1;
});
