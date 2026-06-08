<#
.SYNOPSIS
  Install (or update) capture-mcp on Windows, ready to register in an MCP client.
  The PowerShell parallel of install.sh. Idempotent. Prints CAPTURE_MCP_BIN and
  CAPTURE_MCP_PY on success.

  On Windows, screenshots use GDI+ and window discovery uses EnumWindows (no extra
  deps). Per-app audio (WASAPI process loopback) is not wired yet, so capture runs
  with screenshots + logs (and mic fallback); no Screen Recording permission needed.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install.ps1
  powershell -ExecutionPolicy Bypass -File install.ps1 -Extras whisper   # faster-whisper (CUDA)

.NOTES
  Env overrides (parallel to install.sh):
    CAPTURE_HOME      install dir            (default: %USERPROFILE%\.capture-mcp)
    CAPTURE_REPO_URL  git repo to clone from (default: the public capture repo)
#>
[CmdletBinding()]
param(
    # ASR backend extra(s). faster-whisper runs CUDA on an NVIDIA box; CPU otherwise.
    [string[]]$Extras = @('whisper'),
    [switch]$SkipSmoke
)

$ErrorActionPreference = "Stop"

$captureHome = if ($env:CAPTURE_HOME) { $env:CAPTURE_HOME } else { Join-Path $env:USERPROFILE '.capture-mcp' }
$repoUrl     = if ($env:CAPTURE_REPO_URL) { $env:CAPTURE_REPO_URL } else { 'https://github.com/alex-nax/capture.git' }

if (-not (Get-Command git -ErrorAction SilentlyContinue)) { throw "git is required" }

# 1) Clone or update.
if (Test-Path (Join-Path $captureHome '.git')) {
    Write-Host "Updating capture-mcp in $captureHome ..."
    git -C $captureHome pull --ff-only 2>$null | Out-Null
} else {
    Write-Host "Cloning capture-mcp into $captureHome ..."
    git clone --depth 1 $repoUrl $captureHome
    if ($LASTEXITCODE -ne 0) { throw "git clone failed" }
}
Set-Location $captureHome

# 2) Find a usable Python (prefer the py launcher 3.12, then python.org, then a
#    non-Store python on PATH). Mirrors init.ps1's Find-Python.
function Find-Python {
    $candidates = @()
    if (Get-Command py -ErrorAction SilentlyContinue) {
        $candidates += @{ Exe = 'py'; Args = @('-3.12') }
        $candidates += @{ Exe = 'py'; Args = @('-3') }
    }
    $userPy = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python312\python.exe'
    if (Test-Path $userPy) { $candidates += @{ Exe = $userPy; Args = @() } }
    $onPath = (Get-Command python -ErrorAction SilentlyContinue).Source
    if ($onPath -and $onPath -notmatch 'WindowsApps') { $candidates += @{ Exe = $onPath; Args = @() } }
    foreach ($c in $candidates) {
        try {
            $exe = & $c.Exe @($c.Args) -c "import sys; print(sys.executable)" 2>$null
            if ($LASTEXITCODE -eq 0 -and $exe) { return $c }
        } catch { }
    }
    return $null
}

$py = Find-Python
if (-not $py) {
    throw "No usable Python found. Install Python 3.12 (winget install Python.Python.3.12) and re-run."
}
Write-Host "python: $($py.Exe) $($py.Args -join ' ')"

# 3) venv
$venv = Join-Path $captureHome '.venv'
if (-not (Test-Path $venv)) {
    Write-Host "creating venv at .venv ..."
    & $py.Exe @($py.Args) -m venv $venv
    if ($LASTEXITCODE -ne 0) { throw "venv creation failed" }
}
$vpy = Join-Path $venv 'Scripts\python.exe'
if (-not (Test-Path $vpy)) { throw "venv python missing at $vpy" }

# 4) install
& $vpy -m pip install --upgrade pip --quiet
if ($LASTEXITCODE -ne 0) { throw "pip upgrade failed" }
$spec = if ($Extras.Count -gt 0) { ".[$([string]::Join(',', $Extras))]" } else { '.' }
Write-Host "Installing capture-mcp$spec (editable) ..."
& $vpy -m pip install -e $spec
if ($LASTEXITCODE -ne 0) { throw "pip install failed" }

# 5) smoke (hermetic; no GPU/permissions needed)
if (-not $SkipSmoke) {
    Write-Host "running smoke test ..." -ForegroundColor Cyan
    & $vpy (Join-Path $captureHome 'tests\smoke.py')
    if ($LASTEXITCODE -ne 0) { throw "smoke test failed (rc=$LASTEXITCODE)" }
    Write-Host "smoke test passed." -ForegroundColor Green
}

$bin = Join-Path $venv 'Scripts\capture-mcp.exe'

Write-Host ""
Write-Host "OK capture-mcp ready." -ForegroundColor Green
Write-Host "CAPTURE_MCP_BIN=$bin"
Write-Host "CAPTURE_MCP_PY=$vpy"
Write-Host ""
Write-Host "Next: register it in your project's .mcp.json:"
Write-Host "  & `"$vpy`" `"$(Join-Path $PSScriptRoot 'configure_mcp.py')`" --bin `"$bin`""
