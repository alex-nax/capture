<#
.SYNOPSIS
  Windows bootstrap for capture-mcp (the parallel of init.sh).

  Creates a .venv, installs the package (pyobjc is platform-gated out on Windows;
  screenshots use GDI+ and window discovery uses ctypes — no extra deps), and runs
  the hermetic smoke test. ASR backends are optional extras:
      ./init.ps1 -Extras whisper,riva     # faster-whisper (CUDA) + NVIDIA Riva client

  Per-app audio (WASAPI process loopback) is not wired yet (feature #21); the
  smoke test stubs ASR and does not need a GPU or any permissions.
#>
[CmdletBinding()]
param(
    [string[]]$Extras = @(),
    [switch]$SkipSmoke
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
Set-Location $root
Write-Host "== capture-mcp Windows bootstrap ==" -ForegroundColor Cyan
Write-Host "repo: $root"

function Find-Python {
    # Prefer the py launcher (3.12), then the per-user python.org install, then
    # any `python` on PATH that is NOT the Microsoft Store execution-alias stub.
    # Returns @{ Exe = <path>; Args = @(...) } or $null.
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
    throw "No usable Python found. Install Python 3.12 (e.g. winget install Python.Python.3.12) and re-run."
}
Write-Host "python: $($py.Exe) $($py.Args -join ' ')"

# 1) venv
$venv = Join-Path $root '.venv'
if (-not (Test-Path $venv)) {
    Write-Host "creating venv at .venv ..."
    & $py.Exe @($py.Args) -m venv $venv
    if ($LASTEXITCODE -ne 0) { throw "venv creation failed" }
}
$vpy = Join-Path $venv 'Scripts\python.exe'
if (-not (Test-Path $vpy)) { throw "venv python missing at $vpy" }

# 2) install
Write-Host "upgrading pip ..."
& $vpy -m pip install --upgrade pip --quiet
if ($LASTEXITCODE -ne 0) { throw "pip upgrade failed" }

$spec = '.'
if ($Extras.Count -gt 0) { $spec = ".[$([string]::Join(',', $Extras))]" }
Write-Host "installing $spec (editable) ..."
& $vpy -m pip install -e $spec
if ($LASTEXITCODE -ne 0) { throw "pip install failed" }

# 3) smoke
if (-not $SkipSmoke) {
    Write-Host "running smoke test ..." -ForegroundColor Cyan
    & $vpy (Join-Path $root 'tests\smoke.py')
    $rc = $LASTEXITCODE
    if ($rc -ne 0) { throw "smoke test failed (rc=$rc)" }
    Write-Host "smoke test passed." -ForegroundColor Green
}

Write-Host ""
Write-Host "Done. Activate with:  .\.venv\Scripts\Activate.ps1" -ForegroundColor Green
Write-Host "Run the MCP server:   .\.venv\Scripts\capture-mcp.exe   (or: python -m capture_mcp.server)"
