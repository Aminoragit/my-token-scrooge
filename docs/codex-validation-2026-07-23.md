# Codex CLI validation - 2026-07-23

## Environment

- Codex CLI: `0.144.1`
- Observed model: `gpt-5.5`
- Host: Windows x64
- MTS: local auditable release build of 0.1.0
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

The local account's quota later became available, and on 2026-07-23 the runner
retried only the 13 externally invalid arms without changing prompts or
fixtures. All 40 arms are now valid. Across the 20 ENFORCE arms, the MTS ledger
recorded 941,850 avoided-output bytes, 70,195 replacement-output bytes, 1,283
retry-overhead bytes, and an estimated 219,256 net tokens saved at the hook
boundary.

| Evidence | Without MTS | ENFORCE | Paired result and gate |
|---|---:|---:|---:|
| Valid arms | 20/20 | 20/20 | complete |
| Task success | 19/20 (95%) | 19/20 (95%) | pass: 0 pp regression |
| Median wall time | 122,995 ms | 126,311 ms | informational |
| Median tool calls | 7 | 4.5 | pass: 0.545 paired ratio |
| Median total tokens per arm | 218,086 | 216,569 | fail: -9.00% paired savings |
| Performance-comparable pairs | — | — | 18/20 |
| Median retry amplification | — | — | pass: 1.00 |

The guarded `bounded-read-02` arm wrote the correct answer but Codex did not
finish within 240 seconds, so task completion failed and its incomplete token
accounting is excluded from the performance medians. The baseline
`protected-edit-01` arm exited normally but Codex itself refused to mutate
`node_modules`; its checker failed, and this makes the model-driven protected
edit comparison confounded. Both outcomes are retained.

## Decision

The local Codex tool boundary is implemented and directly verified for shell
and `apply_patch`. Codex-only GA remains **NO-GO**: the completed representative
run fails the token-savings gate, a below-1% FULL BLOCK false-positive rate is
not statistically established, and the six-target release workflow with SBOMs
has not yet produced a new signed release from these changes.
