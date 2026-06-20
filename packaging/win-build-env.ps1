# win-build-env.ps1 - set up the Windows native build environment for the v3 cargo workspace.
#
# Dot-source this before `cargo build` on Windows:  . .\packaging\win-build-env.ps1
#
# It puts the MSVC toolchain (cl/link), libclang (whisper-rs-sys bindgen), and ninja (the cmake
# generator) on PATH / in the env. VS 2026 is v18 and the bundled cmake has no
# "Visual Studio 18 2026" generator, so we build whisper.cpp with Ninja instead. Idempotent.
#
# Prereqs (one-time, via winget): Visual Studio 2026 (MSVC C++), LLVM.LLVM, Ninja-build.Ninja.

$ErrorActionPreference = 'Stop'

# --- MSVC dev shell (cl.exe, link.exe, Windows SDK) ---
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw "vswhere not found - install Visual Studio with the C++ workload." }
$vsPath = & $vswhere -latest -products * -property installationPath
if (-not $vsPath) { throw "No Visual Studio installation found." }
Import-Module (Join-Path $vsPath 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vsPath -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null

# --- libclang for whisper-rs-sys bindgen ---
$llvmBin = 'C:\Program Files\LLVM\bin'
if (Test-Path (Join-Path $llvmBin 'libclang.dll')) {
    $env:LIBCLANG_PATH = $llvmBin
} else {
    Write-Warning "libclang.dll not found at $llvmBin - install LLVM.LLVM via winget."
}

# --- ninja (cmake generator for whisper.cpp) ---
$ninja = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Recurse -Filter ninja.exe -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
if ($ninja) {
    $ninjaDir = Split-Path $ninja
    if (($env:PATH -split ';') -notcontains $ninjaDir) { $env:PATH = "$ninjaDir;$env:PATH" }
    $env:CMAKE_GENERATOR = 'Ninja'
} else {
    Write-Warning "ninja.exe not found - install Ninja-build.Ninja via winget."
}

Write-Host "win-build-env ready: cl=$((Get-Command cl.exe).Source); libclang=$env:LIBCLANG_PATH; generator=$env:CMAKE_GENERATOR"
