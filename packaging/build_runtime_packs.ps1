# build_runtime_packs.ps1 - build Windows ASR runtime PACKS (#81) for capture v3.
#
# A "pack" is the dlopen'd whisper.cpp engine cdylib (capture_asr_whisper.dll), named as the
# GitHub-release asset the daemon downloads. The app ships engine-less; POST /v1/asr/runtimes/install
# fetches the newest `pack-<id>-v<semver>` release's asset matching this OS/arch into
# ~\.capture\runtimes\<id>\ (the Windows sibling of scripts/build_asr_pack.sh on macOS).
#
#   .\packaging\build_runtime_packs.ps1                 # default: whisper-cpu
#   .\packaging\build_runtime_packs.ps1 -Id whisper-cuda
#   .\packaging\build_runtime_packs.ps1 -Id all
#
# Output:
#   whisper-cpu  -> dist\packs\whisper-cpu-windows-x86_64.dll           (single cdylib; CUDA-free)
#   whisper-cuda -> dist\packs\whisper-cuda-windows-x86_64.tar.gz       (cdylib + CUDA runtime DLLs)
#
# Prereqs: packaging\win-build-env.ps1 (MSVC + libclang + ninja). whisper-cuda also needs the CUDA
# toolkit (nvcc) installed and the `cuda` feature on capture-asr-whisper.
param(
    [ValidateSet('whisper-cpu', 'whisper-cuda', 'all')]
    [string]$Id = 'whisper-cpu'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot 'win-build-env.ps1') | Out-Null

$arch = 'x86_64'   # this workspace targets x86_64-pc-windows-msvc
$outDir = Join-Path $root 'dist\packs'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

function Build-CpuPack {
    Write-Host "==> Building whisper-cpu (release; whisper.cpp links in statically, CUDA-free)..."
    Push-Location $root
    cargo build --release -p capture-asr-whisper
    if ($LASTEXITCODE -ne 0) { Pop-Location; throw "cargo build (cpu) failed" }
    Pop-Location
    $dll = Join-Path $root 'target\release\capture_asr_whisper.dll'
    if (-not (Test-Path $dll)) { throw "build ok but $dll missing" }
    $asset = Join-Path $outDir "whisper-cpu-windows-$arch.dll"
    Copy-Item $dll $asset -Force
    Write-Host ("==> Done: {0} ({1:N1} MB)" -f $asset, ((Get-Item $asset).Length / 1MB))
    return $asset
}

function Build-CudaPack {
    # The CUDA engine links cuBLAS/cudart dynamically, so the pack is an ARCHIVE: the cdylib plus the
    # CUDA runtime DLLs the daemon's installer extracts into the runtime dir.
    if (-not (Get-Command nvcc -ErrorAction SilentlyContinue)) {
        throw "whisper-cuda needs the CUDA toolkit (nvcc) on PATH - install it, then re-run."
    }
    Write-Host "==> Building whisper-cuda (release; --features cuda)..."
    Push-Location $root
    cargo build --release -p capture-asr-whisper --features cuda
    if ($LASTEXITCODE -ne 0) { Pop-Location; throw "cargo build (cuda) failed" }
    Pop-Location
    $dll = Join-Path $root 'target\release\capture_asr_whisper.dll'
    if (-not (Test-Path $dll)) { throw "build ok but $dll missing" }

    $stage = Join-Path $outDir "whisper-cuda-windows-$arch"
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item $dll $stage -Force

    # Bundle the CUDA runtime DLLs whisper.cpp's cuBLAS backend needs at load time. CUDA 13 keeps them in
    # bin\x64; CUDA <=12 in bin\. Copy whichever exists.
    $cudaDirs = @((Join-Path $env:CUDA_PATH 'bin\x64'), (Join-Path $env:CUDA_PATH 'bin'))
    foreach ($pat in 'cudart64_*.dll', 'cublas64_*.dll', 'cublasLt64_*.dll') {
        foreach ($cudaBin in $cudaDirs) {
            Get-ChildItem (Join-Path $cudaBin $pat) -ErrorAction SilentlyContinue | ForEach-Object {
                Copy-Item $_.FullName $stage -Force
            }
        }
    }
    $bundled = (Get-ChildItem $stage -Filter '*.dll').Name -join ', '
    Write-Host "    bundled DLLs: $bundled"
    $asset = Join-Path $outDir "whisper-cuda-windows-$arch.tar.gz"
    tar -czf $asset -C $stage .
    Write-Host ("==> Done: {0} ({1:N1} MB)" -f $asset, ((Get-Item $asset).Length / 1MB))
    return $asset
}

$assets = @()
if ($Id -in 'whisper-cpu', 'all') { $assets += Build-CpuPack }
if ($Id -in 'whisper-cuda', 'all') { $assets += Build-CudaPack }

Write-Host ""
Write-Host "Publish each as a runtime-pack release (its own version line, auto-updated by the daemon):"
foreach ($a in $assets) {
    $name = Split-Path $a -Leaf
    $packId = if ($name -like 'whisper-cuda*') { 'whisper-cuda' } else { 'whisper-cpu' }
    Write-Host "  gh release create pack-$packId-vX.Y.Z `"$a`" --repo alex-nax/capture --prerelease ``"
    Write-Host "    --title `"ASR pack: $packId X.Y.Z`" --notes `"whisper.cpp engine for windows-$arch`""
}
