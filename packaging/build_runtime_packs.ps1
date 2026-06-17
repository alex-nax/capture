<#
.SYNOPSIS
  Build the ASR **runtime packs** for the lean Windows daemon (feature #58). Each pack is the engine's
  wheels installed for the frozen daemon's Python tag, zipped — published as GitHub release assets that
  the daemon downloads on demand (it can't pip-install into a frozen bundle). See
  docs/specs/asr-runtimes.md.

.DESCRIPTION
  For each LOCAL runtime in core/asr/runtimes.REGISTRY, runs `pip install --target` of its `pip` list
  (using the venv's Python, whose tag matches the PyInstaller freeze), then zips it to
  dist/runtime-<id>-<pytag>.zip. Host these as release assets under CAPTURE_ASR_PACK_BASE; the daemon's
  POST /v1/asr/runtimes/install downloads + extracts them.

  Output: dist/packs/<id>/ (staging) + dist/runtime-<id>-<pytag>.zip

  Env knobs:
    CAPTURE_PACK_IDS   comma list to limit which runtimes to build (default: all local ones)
#>
[CmdletBinding()]
param()
$ErrorActionPreference = "Continue"   # pip logs to stderr; check $LASTEXITCODE explicitly

$ROOT = (Resolve-Path "$PSScriptRoot\..").Path
$VENVPY = Join-Path $ROOT ".venv\Scripts\python.exe"
$DIST = Join-Path $ROOT "dist"
if (-not (Test-Path $VENVPY)) { throw "missing venv python: $VENVPY (run init.ps1)" }

$pytag = & $VENVPY -c "import sysconfig; v=sysconfig.get_python_version().replace('.',''); print(f'cp{v}-win_amd64')"
$pytag = $pytag.Trim()
Write-Output "==> Building ASR runtime packs for $pytag"

# Single source of truth: the registry's local runtimes + their pip lists.
$json = & $VENVPY -c "from capture_mcp.core.asr import runtimes as r; import json; print(json.dumps([{'id':x['id'],'pip':x['pip']} for x in r.REGISTRY if x['kind']=='local']))"
$runtimes = $json | ConvertFrom-Json

$only = if ($env:CAPTURE_PACK_IDS) { $env:CAPTURE_PACK_IDS.Split(",") | ForEach-Object { $_.Trim() } } else { $null }

foreach ($rt in $runtimes) {
    if ($only -and ($rt.id -notin $only)) { continue }
    if (-not $rt.pip -or $rt.pip.Count -eq 0) { Write-Output "   skip $($rt.id) (no pip)"; continue }
    $target = Join-Path $DIST "packs\$($rt.id)"
    $zip = Join-Path $DIST "runtime-$($rt.id)-$pytag.zip"
    Write-Output "==> Pack '$($rt.id)': pip install --target  [$($rt.pip -join ', ')]"
    if (Test-Path $target) { Remove-Item -Recurse -Force $target }
    New-Item -ItemType Directory -Force -Path $target | Out-Null
    & $VENVPY -m pip install --disable-pip-version-check --target $target @($rt.pip)
    if ($LASTEXITCODE -ne 0) { throw "pip install failed for runtime $($rt.id)" }
    # strip caches to shrink the pack
    Get-ChildItem -Recurse $target -Include "__pycache__" -Directory -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Write-Output "   zipping -> $zip"
    Compress-Archive -Path (Join-Path $target "*") -DestinationPath $zip -CompressionLevel Optimal
    $mb = "{0:N0} MB" -f ((Get-Item $zip).Length / 1MB)
    Write-Output "   done: $zip ($mb)"
}
Write-Output "==> Runtime packs built in $DIST. Publish runtime-*-$pytag.zip as release assets; set CAPTURE_ASR_PACK_BASE to their base URL."
