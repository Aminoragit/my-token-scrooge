# Benchmarks

Benchmark fixtures compare the same task with MTS disabled and enabled. A report records task success, test status, tool calls, completion time, retry amplification, avoided output, replacement output, retry overhead, and net estimated tokens saved. Harness versions, policy hashes, and fixture hashes make comparisons reproducible.

Run `mts benchmark run` for the smoke suite and `mts benchmark export` for a local report.

The direct Codex CLI suite contains exactly 20 deterministic tasks and runs
baseline and ENFORCE arms with alternating order:

```powershell
$env:MTS_BINARY = "$PWD\target\release\mts.exe"
node scripts\codex-ab-benchmark.mjs --validate
node scripts\codex-ab-benchmark.mjs --timeout-ms 240000
```

Interrupted runs can be resumed with `--resume codex-20-<uuid>`. If raw Codex
evidence identifies external quota or service failures, add
`--retry-infrastructure` to rerun only those invalid arms. Product failures and
completed arms are preserved. A report with `status` other than `complete` is
not sufficient for a general-availability claim.

To measure the real Codex hook without spending model quota, run the isolated
directory read/edit evidence suite:

```powershell
$env:MTS_BINARY = "$PWD\target\release\mts.exe"
node scripts\hook-evidence.mjs --validate
node scripts\hook-evidence.mjs
```

This measures bytes returned or avoided at the hook boundary and verifies
mutation outcomes. Its token result is the MTS low-confidence deterministic
estimate of four bytes per token, not an API billing measurement.
