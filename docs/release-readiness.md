# Release readiness

Current decision: **NO-GO for general availability**.

## Implemented scope

- Deterministic Rust policy engine with FULL BLOCK and PARTIAL BLOCK
- Literal shell intent extraction, bounded substitutes, retry circuits, and savings accounting
- Codex CLI `PreToolUse` enforcement for shell and `apply_patch`, locally verified on Windows x64
- Non-destructive native `PreToolUse` hook merge and MTS-only uninstall for Codex CLI, Claude Code CLI, and Google Antigravity CLI
- User configuration targets: `~/.codex/hooks.json`, `~/.claude/settings.json`, and `~/.gemini/config/hooks.json`
- Claude Code CLI 2.1.197 is installed but organization policy disables subscription access; Antigravity CLI (`agy`) is unavailable, so both remain UNVERIFIED/SHADOW
- Recoverable fan-out file transactions, physical per-target policies, project overlays, and last-known-valid fallback
- SQLite state, CLI, validated Ratatui policy editor, npm launcher, and English-only gate
- 64 UNVERIFIED/SHADOW adapter manifests and 704 contract fixture cases
- Generated support matrix, CI checks, setup/doctor/simulation smoke flow, and local benchmark smoke suite

## Direct Codex A/B evidence

The quota-independent real-hook run in the
[sanitized benchmark summary](benchmark-summary.md)
reduced protected-read context by 99.96 percent, conservatively estimated
524,067 net tokens avoided, and prevented 2/2 protected edits. One ordinary
source-read control was intercepted and incurred 426 bytes of overhead, so the
result does not establish the required below-1-percent false-positive rate.

The incomplete 20-task aggregate is recorded in the
[sanitized benchmark summary](benchmark-summary.md).
It has 13 valid matched pairs and 12 performance-comparable pairs; 13 individual
arms are excluded because Codex returned an account usage-limit error.
The 14 valid ENFORCE arms recorded 885,837 avoided bytes and 52,185 replacement
bytes, but model retries and reasoning eliminated the byte-level advantage in
the paired total-token result. That run predates the new explicit no-retry
guidance, so its retry effect remains unverified until the quota-invalid arms
are rerun.

- Paired task success regression: 7.69 percentage points - **fail**
- Median net token savings: -6.12 percent - **fail**
- Median tool-call ratio: 0.606 - **pass**
- Median retry amplification: 1.00 - **pass**
- FULL BLOCK false-positive rate below 1 percent - **not established**

## Evidence still required before GA

- Complete all 20 paired Codex tasks by retrying only quota-invalid arms
- Live-verify Claude Code CLI and Antigravity CLI hook installation, dispatch, and uninstall on hosts with `claude` and `agy`
- Verify 42 current upstream targets end to end
- Prove 12 targets STRICT or STRONG at real pre-tool boundaries
- Clean install/uninstall tests for all six published platform packages
- Task and test success regression no greater than 2 percentage points
- Median tool-call increase no greater than 10 percent
- FULL BLOCK false-positive rate below 1 percent
- Median retry amplification no greater than 1.10
- Representative median net token savings of at least 25 percent
- Signed binaries, checksums, provenance, and SBOM from the release environment
- Recorded 30-second retry demo, two-minute install video, and final TUI screenshots

Registry maxima are planning bounds, not verified claims. Unknown versions are
classified UNVERIFIED and new installations start in SHADOW; promotion should
wait for current official contracts and real smoke tests. No signing,
publishing, deployment, or GA claim is authorized by this local evidence.
