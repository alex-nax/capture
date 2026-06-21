# build_windows.ps1 - build + stage + package the v3 (all-Rust) Capture installer for Windows.
#
# The parallel of build_macos_dmg.sh. Produces dist\CaptureSetup-<version>-x64.exe (per-user, no UAC).
# v3 is a single cargo workspace: the daemon (captured), the GPUI window (capture-gui), and the MCP
# server (capture-mcp) are workspace members; the tray agent (Capture.exe) is the standalone
# agent\windows crate (its own workspace, parity with macOS CaptureBar being the bundle entry point).
# NO PyInstaller, NO bundled ASR engine (#58): the ASR runtime arrives as a pack the daemon installs
# on demand (see build_runtime_packs.ps1 / docs/specs/asr-runtimes.md).
#
# Usage (from the repo root):
#   . .\packaging\win-build-env.ps1            # MSVC dev shell (once per shell)
#   .\packaging\build_windows.ps1              # build everything + stage + compile the installer
#   .\packaging\build_windows.ps1 -NoBuild     # re-stage + recompile using existing target\ binaries
#   .\packaging\build_windows.ps1 -StageOnly   # stage the tree but skip the Inno compile
#
# Signing hook (optional): if CAPTURE_WIN_SIGN_THUMBPRINT is set, every staged .exe and the installer
# are signtool-signed (see docs/specs/windows-release.md section 5). Unset = unsigned build.

[CmdletBinding()]
param(
    [string]$Version,
    [switch]$NoBuild,
    [switch]$StageOnly
)

# Native tools (cargo, ISCC) log to stderr; under 'Stop', PowerShell 5.1 turns their first stderr line
# into a terminating error even on success. Keep 'Continue' and check $LASTEXITCODE after native calls;
# cmdlets that must abort pass -ErrorAction Stop explicitly.
$ErrorActionPreference = 'Continue'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$dist = Join-Path $repo 'dist'
$stage = Join-Path $dist 'Capture'

# --- version: explicit -Version wins, else CAPTURE_GUI_VERSION, else the gui crate's Cargo.toml ---
if (-not $Version) { $Version = $env:CAPTURE_GUI_VERSION }
if (-not $Version) {
    $line = Select-String -Path (Join-Path $repo 'crates\gui\Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' |
        Select-Object -First 1
    if ($line) { $Version = $line.Matches[0].Groups[1].Value }
}
if (-not $Version) { throw "could not determine version (pass -Version or set CAPTURE_GUI_VERSION)" }
Write-Output "==> building Capture $Version (Windows x64)"

# --- 1. build the four release binaries -------------------------------------------------------------
function Invoke-Cargo([string]$cargoArgs, [string]$log) {
    # cmd /c keeps cargo's stderr as plain text (no PowerShell ErrorRecord wrapping).
    $env:CARGO_TERM_COLOR = 'never'
    cmd /c "cargo $cargoArgs > `"$log`" 2>&1"
    if ($LASTEXITCODE -ne 0) {
        Get-Content $log -Tail 30 | Write-Output
        throw "cargo $cargoArgs failed (exit $LASTEXITCODE); full log: $log"
    }
}

New-Item -ItemType Directory -Force -Path $dist -ErrorAction Stop | Out-Null
if (-not $NoBuild) {
    Write-Output "==> cargo build --release (daemon + gui + mcp; gpui release is heavy, ~5 min)"
    Invoke-Cargo "build --release -p capture-daemon -p capture-gui -p capture-mcp" (Join-Path $dist 'build-workspace.log')
    Write-Output "==> cargo build --release (tray agent)"
    Invoke-Cargo "build --release --manifest-path `"$repo\agent\windows\Cargo.toml`"" (Join-Path $dist 'build-agent.log')
}

# --- 2. stage the install tree (see docs/specs/windows-release.md Public contract) ------------------
#   Capture\
#     Capture.exe                 tray agent (entry point + logon task)
#     capture-gui.exe             GPUI window (CAPTURE_AGENT=1)
#     capture-mcp.exe             MCP stdio server (the capture tool surface)
#     captured\captured.exe       Rust daemon (agent resolves it at sibling captured\captured.exe)
#     skill\                      bundled capture skill (skill.rs reads skill\ beside the exe)
#     register_logon_task.ps1     interactive logon-task registrar (run post-install)
$wsRel = Join-Path $repo 'target\release'
$agentRel = Join-Path $repo 'agent\windows\target\release'
$srcMap = [ordered]@{
    "$wsRel\capture-gui.exe" = "$stage\capture-gui.exe"
    "$wsRel\capture-mcp.exe" = "$stage\capture-mcp.exe"
    "$wsRel\captured.exe"    = "$stage\captured\captured.exe"
    "$agentRel\Capture.exe"  = "$stage\Capture.exe"
}

Write-Output "==> staging into $stage"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force -ErrorAction Stop }
New-Item -ItemType Directory -Force -Path $stage, "$stage\captured", "$stage\skill" -ErrorAction Stop | Out-Null
foreach ($src in $srcMap.Keys) {
    if (-not (Test-Path $src)) { throw "missing build artifact: $src (build first, or drop -NoBuild)" }
    Copy-Item $src $srcMap[$src] -Force -ErrorAction Stop
}
# bundled skill: skills\capture\* -> skill\ (drop any Python bytecode cruft)
Copy-Item (Join-Path $repo 'skills\capture\*') "$stage\skill" -Recurse -Force -Exclude '__pycache__', '*.pyc' -ErrorAction Stop
# logon-task registrar
Copy-Item (Join-Path $repo 'packaging\register_logon_task.ps1') "$stage\register_logon_task.ps1" -Force -ErrorAction Stop

# --- 3. optional code signing (signtool hook) ------------------------------------------------------
function Find-SignTool {
    $c = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $cand = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($cand) { return $cand.FullName }
    return $null
}
function Invoke-Sign($path) {
    if (-not $env:CAPTURE_WIN_SIGN_THUMBPRINT) { return }
    $st = Find-SignTool
    if (-not $st) { Write-Warning "signtool not found - leaving $path unsigned"; return }
    $ts = if ($env:CAPTURE_WIN_SIGN_TIMESTAMP_URL) { $env:CAPTURE_WIN_SIGN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    & $st sign /fd sha256 /tr $ts /td sha256 /sha1 $env:CAPTURE_WIN_SIGN_THUMBPRINT $path
    if ($LASTEXITCODE -ne 0) { throw "signtool failed on $path" }
}
if ($env:CAPTURE_WIN_SIGN_THUMBPRINT) {
    Write-Output "==> signing staged binaries (thumbprint $($env:CAPTURE_WIN_SIGN_THUMBPRINT))"
    Get-ChildItem $stage -Recurse -Include *.exe, *.dll | ForEach-Object { Invoke-Sign $_.FullName }
} else {
    Write-Output "==> unsigned build (set CAPTURE_WIN_SIGN_THUMBPRINT to Authenticode-sign; SmartScreen will warn once)"
}

if ($StageOnly) {
    Write-Output "==> staged at $stage (skipping installer compile per -StageOnly)"
    return
}

# --- 4. compile the Inno Setup installer -----------------------------------------------------------
$iscc = $env:CAPTURE_ISCC
if (-not $iscc) {
    $iscc = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw "ISCC.exe not found - install Inno Setup 6 (winget install JRSoftware.InnoSetup) or set CAPTURE_ISCC" }

Write-Output "==> compiling installer with $iscc"
& $iscc "/DMyAppVersion=$Version" "/DStageDir=$stage" "/DOutDir=$dist" (Join-Path $repo 'packaging\capture.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed (exit $LASTEXITCODE)" }

$out = Join-Path $dist "CaptureSetup-$Version-x64.exe"
if (-not (Test-Path $out)) { throw "installer not produced: $out" }
Invoke-Sign $out

Write-Output ""
Write-Output "==> DONE: $out  ($([math]::Round((Get-Item $out).Length/1MB,1)) MB)"
if (-not $env:CAPTURE_WIN_SIGN_THUMBPRINT) {
    Write-Output "    Unsigned - first run shows a SmartScreen warning (More info -> Run anyway). Captures never trigger it."
}
