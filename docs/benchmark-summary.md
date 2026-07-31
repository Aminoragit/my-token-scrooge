# Benchmark summary

This file contains the sanitized aggregate evidence suitable for public source
control. Raw runs are intentionally local because they contain generated
fixtures, absolute machine paths, and complete agent transcripts.

## Real Codex hook boundary

- Date: 2026-07-16
- Binary: release build of MTS 0.1.0
- Environment: Windows x64
- Boundary: Codex `PreToolUse`
- Method: identical isolated fixture trees with MTS disabled and ENFORCE

| Scenario | Baseline | ENFORCE | Delivered context | Context-adjusted estimated tokens saved |
|---|---:|---|---:|---:|
| `node_modules` read | 1,048,576 B | PARTIAL BLOCK | 511 B | 262,016 |
| `__pycache__` read | 524,288 B | FULL BLOCK | 185 B | 131,025 |
| `.git/objects` read | 524,288 B | FULL BLOCK | 182 B | 131,026 |
| `node_modules` edit | Mutated | FULL BLOCK | Unchanged | 0 |
| `dist` edit | Mutated | FULL BLOCK | Unchanged | 0 |
| Ordinary source read control | 65,536 B | PARTIAL BLOCK | 65,962 B | 0 |
| Ordinary source edit control | Mutated | ALLOW | Mutated | 0 |

Protected-read context fell from 2,097,152 bytes to 878 bytes, a 99.96 percent
reduction. The context-adjusted estimate is 524,067 net tokens avoided.
Protected edits were prevented 2/2. Both controls completed functionally, while
the ordinary source read incurred 426 bytes of bounded-result overhead.

Token values use a low-confidence four-bytes-per-token estimate and are not
billed API-token measurements. Two controls cannot establish a below-one-percent
false-positive rate.

## Model-driven Codex A/B

The same 20-task run was completed on 2026-07-23 by retrying only the arms that
previously returned an account usage-limit error. It now contains 40 valid arms,
20 matched pairs, and 18 performance-comparable pairs.

| Metric | Without MTS | ENFORCE | Paired result |
|---|---:|---:|---:|
| Valid arms | 20/20 | 20/20 | Complete |
| Task success | 19/20 (95%) | 19/20 (95%) | 0 pp regression |
| Median wall time | 122,995 ms | 126,311 ms | — |
| Median tool calls | 7 | 4.5 | 0.545 ratio |
| Median total tokens per arm | 218,086 | 216,569 | -9.00% median savings |
| Performance-comparable pairs | — | — | 18/20 |
| Median retry amplification | — | — | 1.00 |

The guarded `bounded-read-02` task produced the correct file but the Codex turn
timed out after 240 seconds, so it failed task completion and is excluded from
performance medians. In the baseline `protected-edit-01` task, Codex itself
refused to mutate `node_modules`; that baseline failed its checker and makes the
model-driven protected-edit comparison confounded. Both limitations remain in
the aggregate rather than being discarded.

The run passes the task-regression, tool-call, and retry gates. It fails the
representative token-savings gate because the median matched pair used 9.00
percent more tokens under ENFORCE, despite MTS recording 219,256 estimated net
tokens saved at the hook boundary. See the
[detailed Codex validation](codex-validation-2026-07-23.md). GA remains NO-GO.
