# Build (if needed) and run the game server in this window. Ctrl+C stops it.
#   .\Scripts\RunServer.ps1                       # port 7777
#   .\Scripts\RunServer.ps1 -NoMonsters           # PvE rooms without server monsters
#   .\Scripts\RunServer.ps1 -PveSlamMs 1500       # PvE rooms get the test slam attack every 1.5 s (dodge testing)
#   .\Scripts\RunServer.ps1 -Rust                 # run the Rust server instead of the C++ one
# Teammates on other PCs connect to <this PC's IPv4>:7777. Allow it once in an admin PowerShell:
#   New-NetFirewallRule -DisplayName "Ascension Server" -Direction Inbound -Protocol TCP -LocalPort 7777 -Action Allow
param(
    [int]$Port = 7777,
    [int]$Threads = 0,
    [int]$PveSlamMs = 0,
    [switch]$NoMonsters,
    [switch]$NoBuild,
    [switch]$Rust
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if ($Rust) {
    # Rust server (same protocol and options). Needs Rust (https://rustup.rs) and Visual Studio's C++ tools.
    if (-not $NoBuild) {
        & cargo build --release -p asc-server --manifest-path (Join-Path $root 'rust\Cargo.toml')
        if ($LASTEXITCODE -ne 0) { throw 'Rust server build failed.' }
    }
    $exe = Join-Path $root 'rust\target\release\ascension-server.exe'
}
else {
    if (-not $NoBuild) { & (Join-Path $PSScriptRoot 'Build.ps1') }
    $exe = Join-Path $root 'build\windows\bin\ascension_server.exe'
}
if (-not (Test-Path $exe)) { throw "Server not built: $exe" }

$serverArgs = @('--port', $Port)
if ($Threads -gt 0) { $serverArgs += @('--threads', $Threads) }
if ($PveSlamMs -gt 0) { $serverArgs += @('--pve-slam-ms', $PveSlamMs) }
if ($NoMonsters) { $serverArgs += @('--pve-monsters', 0) }

[Console]::OutputEncoding = [Text.Encoding]::UTF8
Push-Location $root
try { & $exe @serverArgs }
finally { Pop-Location }
