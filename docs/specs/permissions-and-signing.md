# Spec: Permissions & Signing
_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose
ScreenCaptureKit (the per-app/system audio path in `helper/audiocap`) requires the macOS Screen Recording (TCC) privacy grant for the process that launches the helper. macOS keys that grant to a binary's code-signing identity. An **ad-hoc** signature (`codesign -s -`) changes on every rebuild, so each rebuilt helper looks like a brand-new binary and the user must re-approve Screen Recording every time. This scope gives the helper a **stable, self-signed code-signing identity** so the grant is approved once and persists across rebuilds, documents the one-time approval steps, and clarifies which `SCStreamError` codes mean "permission denied" (`-3801`/`-3803`, fatal) versus "transient connection interruption" (`-3805`, recoverable). It covers the build/sign scripts only; the helper's runtime reconnect logic lives in `helper/audiocap.swift` and is described here where it intersects with permissions.

## Files
- `scripts/setup_codesign.sh` — creates/reuses a stable self-signed code-signing identity and signs the helper with it; prints one-time approval instructions.
- `scripts/build_helper.sh` — compiles `helper/audiocap.swift` to `helper/audiocap` and signs it (ad-hoc by default, or with `CODESIGN_IDENTITY` for a persistent grant).
- `helper/audiocap.swift` — the ScreenCaptureKit helper; its stream-error delegate distinguishes permission errors (`-3801`/`-3803`) from interruptions (`-3805`) and drives reconnection. (Referenced for behavior; full helper contract is out of scope here — see the audio/helper spec.)

## Public contract

### `scripts/setup_codesign.sh`
- **Invocation:** `bash scripts/setup_codesign.sh` (no positional args).
- **Input env var:** `CAPTURE_CODESIGN_CN` — certificate common name; default `capture-mcp-codesign` (`setup_codesign.sh:18`).
- **Side effects:** imports a self-signed identity into the default keychain (or `~/Library/Keychains/login.keychain-db` fallback), signs `helper/audiocap`, and builds the helper first if it does not exist.
- **stdout:** human-readable progress (`✓`/`✗` lines), `codesign -dvvv` excerpt (`Authority`/`Identifier`/`TeamIdentifier`), and a final here-doc with one-time approval steps and the persistent-rebuild command.
- **Exit codes:** `0` on success; `1` if identity creation fails (`setup_codesign.sh:53`). `set -euo pipefail` is in effect, so unhandled command failures also abort non-zero.
- **Idempotency:** if an identity matching `$CERT_NAME` already exists (`security find-identity -p codesigning | grep -q "$CERT_NAME"` — no `-v`, so untrusted self-signed certs are still detected), creation is skipped and only re-signing runs.

### `scripts/build_helper.sh`
- **Invocation:** `bash scripts/build_helper.sh` (no positional args).
- **Input env var:** `CODESIGN_IDENTITY` — signing identity passed to `codesign --sign`; default `-` (ad-hoc) via `IDENTITY="${CODESIGN_IDENTITY:--}"` (`build_helper.sh:25`).
- **Output artifact:** `helper/audiocap` (native binary).
- **Fixed signing identifier:** `com.local.audiocap` (`--identifier`, both scripts).
- **stdout:** build progress, `Signed <out> (identity: <IDENTITY>)` (or a codesign warning), `Built <out>`, and an IMPORTANT note about the Screen Recording requirement and `-3805`.
- **Exit codes:** `1` if `swiftc` is not found (`build_helper.sh:10-14`); otherwise `0`. A failed `codesign` does **not** abort the build — it prints `warning: codesign failed; ...` and continues (`build_helper.sh:26-28`).

### Relevant helper behavior (read-only contract from `audiocap.swift`)
- `--system` flag → whole-display audio capture; otherwise `--pid <PID>` or `--bundle <id>` (`audiocap.swift:35,38`). `setup_codesign.sh` uses `--system` to trigger the permission prompt.
- On a healthy stream the helper writes `READY rate=<n> channels=1 fmt=s16le target=<label>` to **stderr** (`audiocap.swift:227`), then raw s16le PCM on stdout — the process-boundary contract from `docs/architecture.md`.

## Behavior

### `setup_codesign.sh`
1. Resolve `CERT_NAME` (default `capture-mcp-codesign`), repo `ROOT`, `HELPER=$ROOT/helper/audiocap`, and `KEYCHAIN` from `security default-keychain` (falling back to `login.keychain-db`) (`:18-22`).
2. `have_identity()` checks whether a codesigning identity whose listing contains `$CERT_NAME` already exists (`:24`).
3. If it exists, print `✓ ... already exists.` and skip to signing (`:26-27`).
4. Otherwise, in a temp dir (cleaned on EXIT), generate an OpenSSL config with `CN=$CERT_NAME`, `keyUsage=critical,digitalSignature`, `extendedKeyUsage=critical,codeSigning`, `CA:false` (`:30-42`).
5. Create a self-signed RSA-2048 X.509 cert valid 3650 days, export it to a PKCS#12, and `security import` it into `$KEYCHAIN` authorizing `/usr/bin/codesign` to use the key (`-T /usr/bin/codesign`). Two macOS/OpenSSL-3 compatibility requirements (`:45-52`): the export uses **`-legacy`** when available (OpenSSL 3.x's default PKCS#12 MAC is SHA-256+AES, which `security import` rejects with "MAC verification failed during PKCS12 import"; `-legacy` emits the 3DES/RC2 + SHA-1 form macOS reads), and a **non-empty throwaway passphrase** (`P12_PASS="capture-mcp"`, matched on import via `-P`) — an empty-password PKCS#12 also fails MAC verification. The passphrase only protects the temp `.p12`; the identity lives in the keychain after import.
6. Run `security set-key-partition-list -S apple-tool:,apple: -s -k "" "$KEYCHAIN"` to suppress codesign's "wants to use key" prompt; if it cannot run non-interactively, print a note that codesign may prompt once (click "Always Allow") (`:51-52`).
7. Re-check `have_identity`; print `✓ Created` or `✗ Failed ... exit 1`. **`have_identity()` lists with `security find-identity -p codesigning` WITHOUT `-v`** (`:24`): a self-signed cert is untrusted (`CSSMERR_TP_NOT_TRUSTED`) so it never appears under `-v` (valid-identities-only), yet `codesign --sign "$CERT_NAME"` can still use it by name. Using `-v` here made the post-import check (and the idempotency skip) always report failure even when import succeeded.
8. If `$HELPER` is not executable, run `build_helper.sh` to build it (`:56-59`).
9. Sign the helper: `codesign --force --options runtime --sign "$CERT_NAME" --identifier com.local.audiocap "$HELPER"` (`:62`).
10. Print a `codesign -dvvv` excerpt (Authority/Identifier/TeamIdentifier) (`:63`).
11. Print the one-time approval here-doc (`:65-79`): (a) `./helper/audiocap --system` to trigger the prompt, (b) enable `audiocap` (and the terminal app) under System Settings → Privacy & Security → Screen Recording, then quit and reopen the terminal; plus the persistent-rebuild command `CODESIGN_IDENTITY="$CERT_NAME" bash scripts/build_helper.sh`.

### `build_helper.sh`
1. Resolve repo root, `src=helper/audiocap.swift`, `out=helper/audiocap` (`:6-8`).
2. If `swiftc` is missing, print the `xcode-select --install` hint and exit 1 (`:10-14`).
3. Compile: `swiftc -O -o "$out" "$src" -framework ScreenCaptureKit -framework AVFoundation -framework CoreMedia` (`:17-20`).
4. Resolve `IDENTITY` from `CODESIGN_IDENTITY` (default `-`, ad-hoc) (`:25`).
5. Sign: `codesign --force --sign "$IDENTITY" --identifier com.local.audiocap "$out"`; on success print `Signed ... (identity: <IDENTITY>)`, on failure print a warning and continue (`:26-28`).
6. Print `Built <out>` and the IMPORTANT Screen Recording / `-3805` note (`:30-37`).

### Runtime permission handling (`audiocap.swift`, for context)
- The stream-error delegate (`stream(_:didStopWithError:)`, `:141-154`) inspects `NSError.code`:
  - `-3801` (userDeclined) or `-3803` (missingEntitlements): genuine permission failures — log `permission error — grant Screen Recording (see README); not retrying` and `shutdown(1)` (`:147-150`).
  - Anything else (notably `-3805`, connection interrupted on a Space/display/focus change): call `scheduleReconnect()` to rebuild the stream and keep capturing (`:151-153`).

## Invariants & constraints
- **Stable identity ⇒ persistent TCC grant.** The grant is keyed to the code-signing identity. To keep it across rebuilds, every build must reuse the same identity: `CODESIGN_IDENTITY="$CERT_NAME" bash scripts/build_helper.sh`. Ad-hoc (`-`) invalidates the grant on each rebuild.
- **Identifier is fixed.** Both scripts sign with `--identifier com.local.audiocap`; this must not change without re-approving the grant.
- **`-3805` ≠ `-3801`.** `-3805` is a transient connection interruption and MUST be treated as recoverable (reconnect); `-3801`/`-3803` are permission denials and MUST NOT be retried. Do not conflate them.
- **stdout is sacred** (`docs/architecture.md` hard constraints): the helper emits PCM on stdout and all human-readable status (incl. `READY`, error logs) on stderr. The build/sign scripts are CLI tooling, not part of the MCP server path, but the helper they produce must preserve this contract.
- **Screen Recording is required for the launching process.** The TCC grant must cover the process that spawns the helper (e.g. the terminal app, or the MCP host). Enabling `audiocap` alone is not always sufficient — the parent must also be enabled, then the terminal restarted (`setup_codesign.sh:71-72`).
- **macOS-only.** Per `docs/architecture.md` Platform section, this entire scope is macOS-specific (codesign, security, ScreenCaptureKit, TCC).
- **Idempotent setup.** `setup_codesign.sh` must remain safe to re-run on a fresh or already-configured machine.

## Failure modes & handling
- **`swiftc` missing** (`build_helper.sh`): prints install hint, exits 1. No artifact produced.
- **`codesign` fails during build** (`build_helper.sh`): prints `warning: codesign failed; the helper may be blocked by Gatekeeper/TCC` and continues with exit 0 — the binary exists but may be unsigned/blocked.
- **Identity creation fails** (`setup_codesign.sh`): final `have_identity` check fails → prints `✗ Failed to create identity.` to stderr, exits 1.
- **Cannot set partition list non-interactively** (`setup_codesign.sh:51-52`): falls back to a printed note; codesign may prompt once for the login keychain password — user clicks "Always Allow". Not fatal.
- **Permission not yet granted at runtime** (`audiocap.swift`): `startCapture` fails or the stream stops; `-3801`/`-3803` → log permission error and exit 1; any other code → reconnect with backoff.
- **Transient interruption `-3805`** (Space/display/focus change): handled by `scheduleReconnect()` — backoff `min(2.0, 0.25 * reconnects)` seconds, `reconnects` reset to 0 once audio flows (`audiocap.swift:130,205-216`). If audio never flows, it gives up after >20 attempts (~30s) and exits 1 (`:210-213`).
- **Wrong/changed identity across rebuilds:** no error is raised, but macOS treats the helper as new and silently revokes the grant; the next capture fails with a permission error until re-approved. Mitigation: always pass the same `CODESIGN_IDENTITY`.

## Outputs / artifacts
- `helper/audiocap` — signed native binary (arm64/x86_64). Signed with `--identifier com.local.audiocap`; identity is ad-hoc by default or `CODESIGN_IDENTITY`/`$CERT_NAME` when persistence is desired.
- A self-signed code-signing identity/certificate named `$CAPTURE_CODESIGN_CN` (default `capture-mcp-codesign`), RSA-2048, 3650-day validity, stored in the default login keychain. Created by `setup_codesign.sh`.
- No other files are written; the OpenSSL key/cert/p12 live only in a temp dir removed on EXIT (`setup_codesign.sh:30`).

## Configuration
| Variable | Used by | Default | Effect |
|---|---|---|---|
| `CAPTURE_CODESIGN_CN` | `setup_codesign.sh` | `capture-mcp-codesign` | Common name of the self-signed cert / identity to create and sign with. |
| `CODESIGN_IDENTITY` | `build_helper.sh` | `-` (ad-hoc) | Identity passed to `codesign --sign`. Set to the stable cert name for a persistent TCC grant. |

Fixed (not configurable): code-signing `--identifier` = `com.local.audiocap`; cert key type RSA-2048; validity 3650 days; OpenSSL EKU `codeSigning`.

## Known limitations / open items
- **Self-signed, not notarized/Developer ID.** The identity is local and self-signed; it is sufficient for a stable TCC grant on the same machine but is not portable across machines and is not Gatekeeper-trusted for distribution.
- **Per-machine.** The grant and keychain identity are local; a new machine needs `setup_codesign.sh` re-run and re-approval.
- **`set-key-partition-list` may still prompt once** if it cannot run non-interactively (depends on keychain state / macOS version) — handled gracefully but requires a manual "Always Allow".
- **Parent-process grant nuance is documented only in prose.** Whether the terminal/host or `audiocap` (or both) needs the grant depends on launch context; the scripts cannot enforce it and rely on the printed one-time steps. (Stated as guidance, not verified programmatically.)
- **Uncertain:** the exact macOS-version behavior of `security default-keychain` quoting/whitespace handling is normalized with `tr -d ' "'` (`setup_codesign.sh:21`) but not otherwise validated; edge cases (non-default keychains) are untested here.

## Tests
- `tests/smoke.py` deliberately exercises only the permission-free paths (launch-mode logging + screenshots) and explicitly **does not** require Screen Recording / Microphone permission or a GPU (`tests/smoke.py:5-12`). It therefore does **not** cover this scope's signing/permission behavior.
- This scope is currently verified **manually** via the one-time flow printed by `setup_codesign.sh`:
  1. `bash scripts/setup_codesign.sh` (creates identity, signs helper).
  2. `./helper/audiocap --system` to trigger the TCC prompt; approve Screen Recording for the launching app, then restart the terminal.
  3. Confirm a `READY rate=... fmt=s16le` line on stderr (grant active) rather than a `permission error` log.
  4. Rebuild with `CODESIGN_IDENTITY="capture-mcp-codesign" bash scripts/build_helper.sh` and confirm capture still works **without** re-approval (proves grant persistence).
- **Recommended additions (not yet present):** (a) a scriptable check that `codesign -dvvv helper/audiocap` reports the expected `Identifier=com.local.audiocap` and the stable Authority; (b) a non-interactive assertion that `security find-identity -p codesigning` (no `-v`) contains `$CAPTURE_CODESIGN_CN` after setup; (c) a unit test of the helper's `-3805` vs `-3801`/`-3803` branch selection (today only enforceable by code review of `audiocap.swift:141-154`).
