<#
.SYNOPSIS
  Build the Capture Windows installer: cargo (GUI + tray agent + native audio helper) + a
  PyInstaller-frozen daemon, staged and wrapped by Inno Setup into CaptureSetup-<ver>-x64.exe.
  The Windows analogue of packaging/build_macos_dmg.sh. See docs/specs/windows-release.md.

.DESCRIPTION
  Output: dist\CaptureSetup-<version>-x64.exe  (dist\ is gitignored)

  Env knobs:
    CAPTURE_GUI_VERSION    bundle version (default 0.2.5)
    CAPTURE_WIN_DEBUG=1    reuse/build DEBUG binaries (fast iteration) instead of release
    CAPTURE_SKIP_FREEZE=1  reuse an existing daemon freeze (skip PyInstaller)
    CAPTURE_SKIP_CARGO=1   reuse existing cargo binaries (skip cargo build)
    CAPTURE_ISCC           path to ISCC.exe (else common winget/Program Files locations)
    CAPTURE_WIN_SIGN_THUMBPRINT / CAPTURE_WIN_SIGN_CERT (+ _PASSWORD) / CAPTURE_WIN_SIGN_TIMESTAMP_URL
                           if set, Authenticode-sign every .exe + the installer via signtool.

  NOTE: CUDA runtime DLLs (nvidia-*-cu12) are NOT bundled - the frozen daemon does CPU faster-whisper
  out of the box and uses CUDA only if the user installed the nvidia wheels (see asr.md).
#>
[CmdletBinding()]
param()
# NOTE: native tools (cargo, PyInstaller, ISCC) log to stderr; under 'Stop' PowerShell 5.1 turns
# their first stderr line into a terminating error even on success. So keep 'Continue' and check
# $LASTEXITCODE after each native call; cmdlets that must abort use -ErrorAction Stop.
$ErrorActionPreference = "Continue"

$ROOT = (Resolve-Path "$PSScriptRoot\..").Path
$VERSION = if ($env:CAPTURE_GUI_VERSION) { $env:CAPTURE_GUI_VERSION } else { "0.2.6" }
$PROFILE_ = if ($env:CAPTURE_WIN_DEBUG -eq "1") { "debug" } else { "release" }
$TARGET = Join-Path $ROOT "gui\target"
$BIN = Join-Path $TARGET $PROFILE_
$DIST = Join-Path $ROOT "dist"
$STAGE = Join-Path $DIST "Capture"
$FREEZE = Join-Path $ROOT "packaging\build\dist\captured"
$VENVPY = Join-Path $ROOT ".venv\Scripts\python.exe"

Write-Output "==> Capture Windows build  version=$VERSION  profile=$PROFILE_"
if (-not (Test-Path $VENVPY)) { throw "missing venv python: $VENVPY (run init.ps1)" }

# Build into the GUI's target dir so already-built (Smart-App-Control-cleared) build-script
# artifacts are reused - fresh build-script probes get blocked by SAC on this box.
$env:CARGO_TARGET_DIR = $TARGET
$relFlag = if ($PROFILE_ -eq "release") { "--release" } else { $null }

if ($env:CAPTURE_SKIP_CARGO -ne "1") {
    Write-Output "==> cargo build ($PROFILE_): GUI + tray agent + native audio helper (gpui release is heavy)..."
    foreach ($mani in @("gui\Cargo.toml", "agent\windows\Cargo.toml", "helper\audiocap_win_rs\Cargo.toml")) {
        $args = @("build", "--manifest-path", (Join-Path $ROOT $mani))
        if ($relFlag) { $args += $relFlag }
        & cargo @args
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed for $mani" }
    }
}
foreach ($b in @("capture-gui.exe", "Capture.exe", "audiocap_win.exe")) {
    if (-not (Test-Path (Join-Path $BIN $b))) { throw "missing built binary: $BIN\$b" }
}

# --- Freeze the daemon (PyInstaller onedir) --------------------------------------
# LEAN by default (#58): NO ASR engine bundled — the user installs a runtime pack later
# (docs/specs/asr-runtimes.md). Keeps huggingface_hub (model downloads) + the runtime activator.
# Excludes faster-whisper/ctranslate2/mlx; the engine arrives via a pack on sys.path.
if ($env:CAPTURE_SKIP_FREEZE -eq "1" -and (Test-Path (Join-Path $FREEZE "captured.exe"))) {
    Write-Output "==> CAPTURE_SKIP_FREEZE=1 - reusing freeze at $FREEZE"
} else {
    Write-Output "==> Freezing the daemon (PyInstaller onedir; LEAN - no ASR engine bundled)..."
    & $VENVPY -m PyInstaller --noconfirm --onedir --name captured `
        --distpath (Join-Path $ROOT "packaging\build\dist") `
        --workpath (Join-Path $ROOT "packaging\build\work") `
        --specpath (Join-Path $ROOT "packaging\build") `
        --hidden-import capture_mcp.core.platform.windows `
        --hidden-import capture_mcp.core.import_media `
        --hidden-import capture_mcp.core.vision_client `
        --hidden-import capture_mcp.core.indexer `
        --hidden-import capture_mcp.core.frames `
        --hidden-import capture_mcp.core.asr.whisper_local `
        --hidden-import capture_mcp.core.asr.openai_compat `
        --hidden-import capture_mcp.core.asr.runtimes `
        --collect-all huggingface_hub `
        --exclude-module faster_whisper --exclude-module ctranslate2 `
        --exclude-module mlx --exclude-module mlx_whisper --exclude-module torch `
        (Join-Path $ROOT "packaging\captured_main.py")
    if ($LASTEXITCODE -ne 0) { throw "PyInstaller freeze failed" }
}
if (-not (Test-Path (Join-Path $FREEZE "captured.exe"))) { throw "freeze missing: $FREEZE\captured.exe" }

# --- Stage the install tree ------------------------------------------------------
Write-Output "==> Staging install tree at $STAGE ..."
if (Test-Path $STAGE) { Remove-Item -Recurse -Force $STAGE -ErrorAction Stop }
New-Item -ItemType Directory -Force -Path $STAGE -ErrorAction Stop | Out-Null
Copy-Item (Join-Path $BIN "Capture.exe")     (Join-Path $STAGE "Capture.exe") -ErrorAction Stop
Copy-Item (Join-Path $BIN "capture-gui.exe") (Join-Path $STAGE "capture-gui.exe") -ErrorAction Stop
Copy-Item -Recurse $FREEZE (Join-Path $STAGE "captured") -ErrorAction Stop
Copy-Item (Join-Path $BIN "audiocap_win.exe") (Join-Path $STAGE "captured\audiocap_win.exe") -ErrorAction Stop
Copy-Item (Join-Path $ROOT "packaging\register_logon_task.ps1") (Join-Path $STAGE "register_logon_task.ps1") -ErrorAction Stop
New-Item -ItemType Directory -Force -Path (Join-Path $STAGE "skill") -ErrorAction Stop | Out-Null
Copy-Item -Recurse -Force (Join-Path $ROOT "skills\capture\*") (Join-Path $STAGE "skill") -Exclude "__pycache__", "*.pyc" -ErrorAction Stop

# --- Optional Authenticode signing (signtool hook) -------------------------------
$signThumb = $env:CAPTURE_WIN_SIGN_THUMBPRINT
$signCert = $env:CAPTURE_WIN_SIGN_CERT
function Find-SignTool {
    $c = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $cand = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($cand) { return $cand.FullName }
    return $null
}
function Invoke-Sign($path) {
    if (-not ($signThumb -or $signCert)) { return }
    $st = Find-SignTool
    if (-not $st) { Write-Warning "signtool not found - leaving $path unsigned"; return }
    $ts = if ($env:CAPTURE_WIN_SIGN_TIMESTAMP_URL) { $env:CAPTURE_WIN_SIGN_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }
    if ($signThumb) {
        & $st sign /fd sha256 /tr $ts /td sha256 /sha1 $signThumb $path
    } else {
        & $st sign /fd sha256 /tr $ts /td sha256 /f $signCert /p $env:CAPTURE_WIN_SIGN_PASSWORD $path
    }
}
if ($signThumb -or $signCert) {
    Write-Output "==> Signing staged binaries (signtool)..."
    Get-ChildItem -Recurse $STAGE -Include *.exe, *.dll | ForEach-Object { Invoke-Sign $_.FullName }
} else {
    Write-Output "==> (unsigned build - set CAPTURE_WIN_SIGN_THUMBPRINT/_CERT to Authenticode-sign; SmartScreen will warn)"
}

# --- Compile the installer (Inno Setup) ------------------------------------------
$ISCC = $env:CAPTURE_ISCC
if (-not $ISCC) {
    foreach ($p in @(
            "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
            "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
            "C:\Program Files\Inno Setup 6\ISCC.exe")) {
        if (Test-Path $p) { $ISCC = $p; break }
    }
}
if (-not $ISCC) { throw "ISCC.exe not found - install Inno Setup (winget install JRSoftware.InnoSetup) or set CAPTURE_ISCC" }

Write-Output "==> Compiling installer with $ISCC ..."
& $ISCC "/DMyAppVersion=$VERSION" "/DStageDir=$STAGE" "/DOutDir=$DIST" (Join-Path $ROOT "packaging\capture.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$installer = Join-Path $DIST "CaptureSetup-$VERSION-x64.exe"
if (-not (Test-Path $installer)) { throw "installer not produced: $installer" }
Invoke-Sign $installer

$size = "{0:N1} MB" -f ((Get-Item $installer).Length / 1MB)
Write-Output "==> Done: $installer ($size)"
if (-not ($signThumb -or $signCert)) {
    Write-Output "    Unsigned - first run shows a SmartScreen warning (More info -> Run anyway). Captures never trigger it."
}
