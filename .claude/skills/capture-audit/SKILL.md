---
name: capture-audit
description: Health/consistency audit for the capture-mcp project (this repo). Use when the user says "capture audit", "harness audit", "check capture-mcp health", "spec drift", "garbage collect", or after several sessions to verify the harness artifacts are still accurate. Checks spec↔code sync (specs are mandatory here), features.json accuracy, progress-log integrity, smoke status, and platform assumptions. Read-only by default; reports findings.
---

# capture-audit

Garbage-collection / consistency audit for **capture-mcp**. Read-only: produce a report; only
fix with the user's go-ahead.

## Checks

1. **Spec ↔ code sync (the core invariant here).** For each `docs/specs/<scope>.md`, open the
   files it lists and confirm *Public contract / Behavior / Invariants / Failure modes* still
   match the code. Flag drift. Every scope must have a spec; every spec must be linked from
   `docs/specs/README.md`. New/renamed modules without a spec = a finding.
2. **features.json accuracy.** For each `"passes": true`, sanity-check it's actually implemented;
   for `"passes": false`, confirm it's genuinely open. Dependencies form a sane order. JSON parses.
3. **Progress-log integrity.** `claude-progress.md` newest-entry-first; latest "next task" still
   makes sense; known issues either fixed or still listed.
4. **Tests.** `python tests/smoke.py` → `20/20 passed`. Note any failure with output.
5. **Architecture constraints** (`docs/architecture.md`): no `print()`/stdout writes in the
   `server.py` MCP path; tool handlers async + offloaded; capture loops catch their own errors;
   start() rolls back on partial failure; audio reader joined before files close.
6. **Build/permissions artifacts present & coherent:** `scripts/build_helper.sh`,
   `scripts/setup_codesign.sh`, `init.sh`. Helper builds (`bash scripts/build_helper.sh`).
7. **Platform assumptions.** macOS-only paths (screencapture/Quartz/ScreenCaptureKit/sips/ffmpeg
   avfoundation, bash `init.sh`) are documented as such; cross-platform gaps tracked in
   features/`docs/specs/platform-abstraction.md` (important for the Windows follow-up).

## Output
A short report grouped by severity: **drift** (spec/code mismatch), **inaccuracy**
(features/progress), **risk** (constraint/test/platform). Each with file:line and a one-line fix.
Offer to apply fixes; on approval, update code + spec + progress together and re-run smoke.
