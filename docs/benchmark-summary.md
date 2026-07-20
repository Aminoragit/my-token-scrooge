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

The incomplete 20-task run produced 13 valid matched pairs and 12
performance-comparable pairs before the local account reached its usage limit.

| Metric | Result |
|---|---:|
| Baseline success on valid pairs | 13/13 |
| ENFORCE success on valid pairs | 12/13 |
| Median total-token savings | -6.12% |
| Median tool-call ratio | 0.606 |
| Median retry amplification | 1.00 |

That run predates the explicit policy-specific no-retry guidance now returned by
MTS. It does not prove the current message reduces model retries. GA remains
NO-GO until the quota-invalid arms are rerun and every release gate passes.
