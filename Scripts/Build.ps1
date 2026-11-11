# Build the server, bot and tests with Visual Studio (2022 or newer). No WSL needed.
#   .\Scripts\Build.ps1            # RelWithDebInfo -> build\windows\bin
#   .\Scripts\Build.ps1 -Debug     # Debug build (same output folder)
#   .\Scripts\Build.ps1 -Clean     # delete build\windows first
param(
    [switch]$Debug,
    [switch]$Clean
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

function Find-CMake {
    $onPath = Get-Command cmake -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    # Visual Studio ships CMake ("C++ CMake tools for Windows" component).
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path $vswhere) {
        $installs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        foreach ($install in $installs) {
            $candidate = Join-Path $install 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
            if (Test-Path $candidate) { return $candidate }
        }
    }
    throw 'CMake not found. Install Visual Studio with "Desktop development with C++" (includes CMake), or install CMake and add it to PATH.'
}

$cmake = Find-CMake
$buildDir = Join-Path $root 'build\windows'
if ($Clean -and (Test-Path $buildDir)) { Remove-Item -Recurse -Force $buildDir }

Push-Location $root
try {
    if (-not (Test-Path (Join-Path $buildDir 'CMakeCache.txt'))) {
        & $cmake --preset windows
        if ($LASTEXITCODE -ne 0) { throw 'CMake configure failed.' }
    }
    $preset = if ($Debug) { 'windows-debug' } else { 'windows' }
    & $cmake --build --preset $preset
    if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
}
finally { Pop-Location }
Write-Host "Built: $buildDir\bin"
