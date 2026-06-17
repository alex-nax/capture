---
name: capture-release
description: Cut a versioned GitHub release of capture-mcp (bump version → build → notarize → tag → publish the signed DMG). USE THIS SKILL ONLY when the user explicitly wants to PREPARE, CUT, SHIP, or PUBLISH a release / new version to GitHub — e.g. "cut a release", "release a new version", "publish 0.3", "ship this to GitHub". Default bump is a PATCH/revision; do a minor or major bump ONLY when the user says so. This is the ONLY time the version number changes — NEVER bump the version for a local dogfood build/install (see the no-version-bump rule).
---

# capture-release

Publishing a release is the **only** time the version changes. A local rebuild/install for testing on
this machine is NOT a release — for that, just run `packaging/build_macos_dmg.sh` at the current version
and change nothing. Reach for this skill only when the user explicitly asks to release/ship/publish.

## Version policy
- **Default = PATCH** (revision): `x.y.Z` → `x.y.(Z+1)`.
- `minor` (`x.(Y+1).0`) or `major` (`(X+1).0.0`) **only when the user explicitly asks** for one.
- The version lives in FOUR files; `scripts/bump_version.py` edits all of them atomically so they can't
  drift: `src/capture_mcp/__init__.py`, `pyproject.toml`, `gui/Cargo.toml`, `packaging/build_macos_dmg.sh`.

## Steps

### 1. Pre-flight (don't release a broken tree)
- Confirm with the user the bump level (patch unless they said minor/major) and that they want it **published
  to GitHub** (this skill is the publish path the standing "don't publish a release" rule otherwise forbids).
- From the repo root with the venv: tests must be green —
  `.venv/bin/python tests/smoke.py` (68/68), `tests/contract/run_contracts.py` (4/4),
  `tests/indexing_hermetic.py`. Don't release on red.
- Working tree should be committed (or commit the pending batch first — release tags a real commit).
- Check the baseline: `gh release list -L 3` and `scripts/bump_version.py --current`.

### 2. Bump the version
```bash
.venv/bin/python .claude/skills/capture-release/scripts/bump_version.py            # patch (default)
# or: ... bump_version.py minor   |   ... bump_version.py major   |   ... bump_version.py --set X.Y.Z
```
It prints `OLD -> NEW`. Note `NEW` for the rest of the flow.

### 3. Build + notarize the signed DMG
```bash
export CAPTURE_SIGN_IDENTITY="Developer ID Application: Alexander Dodonov (YH3QP44ST4)"
export CAPTURE_ASR_SELFTEST=0
bash packaging/build_macos_dmg.sh           # → dist/Capture-<NEW>.dmg  (re-freezes daemon + builds GUI)
```
Notarize with **INLINE** creds (the `capture-notary` keychain profile is flaky — see the inline-creds note).
NEVER echo the password; read it via `$(cat .notary-password)`:
```bash
xcrun notarytool submit "dist/Capture-<NEW>.dmg" \
  --apple-id pr0fedt@gmail.com --team-id YH3QP44ST4 --password "$(cat .notary-password)" --wait
xcrun stapler staple "dist/Capture-<NEW>.dmg"
spctl -a -t open --context context:primary-signature -v "dist/Capture-<NEW>.dmg" || true  # DMG isn't signed; the app inside is
```

### 4. Commit + tag
```bash
git add src/capture_mcp/__init__.py pyproject.toml gui/Cargo.toml packaging/build_macos_dmg.sh
git commit -m "release: vNEW"   # end with the project's Co-Authored-By trailer
git tag vNEW
```
Branch first if on `main` and the project requires it; otherwise tag the release commit directly.

### 5. Publish the GitHub release (the explicit publish step)
The repo is `alex-nax/capture`. Attach the **notarized .dmg as an asset** — the in-app auto-updater (#48)
finds the update by semver-comparing `tag_name` and downloads the `.dmg` asset, so it MUST be attached.
```bash
git push origin <branch> --tags
gh release create vNEW "dist/Capture-<NEW>.dmg" \
  --repo alex-nax/capture --title "Capture <NEW>" --notes "<short changelog>"
```
Only run this once the user has confirmed they want it on GitHub. Draft the notes from the commits since the
last tag (`git log v<PREV>..HEAD --oneline`).

### 6. Verify
- `gh release list -L 3` shows `vNEW` as Latest with the `.dmg` asset.
- An installed older app's #48 check now offers the update.

## Gotchas
- **Never bump for a local build.** Five phantom bumps (0.2.1–0.2.5) happened that way; the version is a
  release artifact only.
- **Inline notarization**, not the keychain profile. Never paste the app password; `.notary-password`,
  `.asp.capture`, `*.secret`, `agent/build/` are gitignored and must never be committed.
- **The .dmg asset is load-bearing** for auto-update — a release without it leaves users unable to update.
- Commit messages describe WHAT changed; no process meta.
