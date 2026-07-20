# Codex CLI validation - 2026-07-16

## Environment

- Codex CLI: `0.144.1`
- Observed model: `gpt-5.5`
- Host: Windows x64
- MTS: local debug build from this workspace
- Codex sandbox: `workspace-write`
- Approval policy: `never`
- User Codex configuration: unchanged
- Benchmark MTS state, project hooks, and fixtures: isolated under the run directory

## Real boundary validation

Codex `PreToolUse` interception was exercised through the installed CLI, not a
mock dispatcher.

- A protected `node_modules/pkg/index.js` `apply_patch` was denied and the file
  remained unchanged.
- An ordinary `apply_patch` in the same probe succeeded.
- A protected shell read was denied.
- A permitted bounded shell read returned replacement context.
- `mts setup --targets codex-cli --yes` merged the MTS-owned hook without
  deleting an existing handler.
- `mts uninstall --targets codex-cli` removed only the MTS-owned handler.

The accepted matcher is `^(Bash|apply_patch|Edit|Write)$`. The earlier wildcard
probe was invalid because Codex hook matchers are regular expressions over tool
names, not glob patterns. The earlier conclusion that `apply_patch` was not
intercepted is therefore superseded.

## Quota-independent directory waste evidence

The real `mts hook codex-cli` process was exercised against isolated baseline
and ENFORCE fixture trees. This measures the hook boundary without invoking the
Codex model. The raw report is in
the local ignored benchmark-results directory; the public aggregate is in the
[sanitized benchmark summary](benchmark-summary.md).

| Scenario | Baseline | ENFORCE | Enforced result/context | Avoided bytes | Context-adjusted estimated tokens saved |
|---|---:|---:|---:|---:|---:|
| `node_modules` read | 1,048,576 B | PARTIAL BLOCK | 511 B | 1,048,576 | 262,016 |
| `__pycache__` read | 524,288 B | FULL BLOCK | 185 B | 524,288 | 131,025 |
| `.git/objects` read | 524,288 B | FULL BLOCK | 182 B | 524,288 | 131,026 |
| `node_modules` edit | mutated | FULL BLOCK | unchanged | 0 | 0 |
| `dist` edit | mutated | FULL BLOCK | unchanged | 0 | 0 |
| ordinary source read control | 65,536 B | PARTIAL BLOCK | 65,962 B | 65,536 | 0 |
| ordinary source edit control | mutated | ALLOW | mutated | 0 | 0 |

Across the three protected reads, ENFORCE reduced delivered context from
2,097,152 bytes to 878 bytes, a 99.96 percent reduction. The conservative
four-bytes-per-token estimate is 524,067 net tokens avoided. Both protected
edits were prevented. The two controls had no functional failure, but the
ordinary source read was still intercepted by the global size-bound rule and
added 426 bytes of context overhead; MTS correctly recorded zero token savings
for it.

These token values are low-confidence byte-based estimates, not billed API
tokens. Two controls cannot establish a below-1-percent false-positive rate.

## Direct 20-task A/B run

Raw evidence and the machine-readable report remain in the local ignored
benchmark-results directory. The public aggregate is in the
[sanitized benchmark summary](benchmark-summary.md).
Baseline disabled hooks; ENFORCE enabled the real project hook. Arm order
alternated by task and both arms used identical fixture hashes.

The run is incomplete because the local ChatGPT account hit its Codex usage
limit. Thirteen arms are marked `codex_usage_limit` and excluded from product
metrics. They remain in the raw evidence and are not counted as failures.

Across the 14 valid ENFORCE arms, the MTS ledger recorded 885,837 avoided bytes,
52,185 replacement bytes, and an estimated 208,819 net tokens saved at the hook
boundary. That byte-level saving did not translate into end-to-end token saving
in the completed pairs, as shown below. This model-driven run predates the
explicit no-retry guidance and must be rerun before claiming that the new
message reduces retries or reasoning.

| Evidence | Result | Gate |
|---|---:|---:|
| Valid matched pairs | 13 of 20 | incomplete |
| Performance-comparable pairs | 12 | incomplete |
| Baseline success on valid pairs | 13/13 (100%) | reference |
| ENFORCE success on valid pairs | 12/13 (92.31%) | fail: regression 7.69 pp |
| Median token savings | -6.12% | fail: at least 25% required |
| Median tool-call ratio | 0.606 | pass: no more than 1.10 |
| Median retry amplification | 1.00 | pass: no more than 1.10 |

The ENFORCE failure completed the fixture checker but the Codex turn did not
finish within 240 seconds after repeated Windows command-quoting retries. It is
retained as a guarded-arm completion failure. Its incomplete token accounting is
excluded from performance medians.

After Codex quota is available, retry only externally invalid arms without
changing prompts or fixtures:

```powershell
$env:MTS_BINARY = "$PWD\target\release\mts.exe"
node scripts\codex-ab-benchmark.mjs --resume codex-20-<run-id> --retry-infrastructure --timeout-ms 240000
```

## Decision

The local Codex tool boundary is implemented and directly verified for shell
and `apply_patch`. Codex-only GA remains **NO-GO**: the representative run is
incomplete, the available paired sample fails the success and token-savings
gates, a below-1% FULL BLOCK false-positive rate is not statistically
established, and signed distribution evidence is absent.
