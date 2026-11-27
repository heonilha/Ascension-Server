# Build (if needed) and run the game server (Rust) in this window. Ctrl+C stops it.
#   .\Scripts\RunServer.ps1                       # port 7777, full 36-minute matches, 2 teams to start
#   .\Scripts\RunServer.ps1 -TimeScale 0.1        # demo: every match time x0.1 (about 3.6 minutes per match)
#   .\Scripts\RunServer.ps1 -StartTeams 1         # start a match with a single team (solo testing)
#   .\Scripts\RunServer.ps1 -NoMonsters           # no server monsters (matches and practice rooms)
#   .\Scripts\RunServer.ps1 -PracticeSlamMs 1500  # practice rooms get the test slam attack every 1.5 s (dodge testing)
# Teammates on other PCs connect to <this PC's IPv4>:7777. Allow it once in an admin PowerShell:
#   New-NetFirewallRule -DisplayName "Ascension Server" -Direction Inbound -Protocol TCP -LocalPort 7777 -Action Allow
# Needs Rust (https://rustup.rs) and Visual Studio's C++ tools (the shared movement header is compiled in).
param(
    [int]$Port = 7777,
    [int]$Threads = 0,
    [double]$TimeScale = 1.0,
    [int]$StartTeams = 2,
    [int]$PracticeSlamMs = 0,
    [switch]$NoMonsters,
    [switch]$NoBuild
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $NoBuild) {
    & cargo build --release -p asc-server --manifest-path (Join-Path $root 'rust\Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'Rust server build failed.' }
}
$exe = Join-Path $root 'rust\target\release\ascension-server.exe'
if (-not (Test-Path $exe)) { throw "Server not built: $exe" }

$serverArgs = @('--port', $Port, '--match-time-scale', $TimeScale.ToString([Globalization.CultureInfo]::InvariantCulture), '--match-start-teams', $StartTeams)
if ($Threads -gt 0) { $serverArgs += @('--threads', $Threads) }
if ($PracticeSlamMs -gt 0) { $serverArgs += @('--practice-slam-ms', $PracticeSlamMs) }
if ($NoMonsters) { $serverArgs += @('--practice-monsters', 0, '--match-monsters', 0) }

[Console]::OutputEncoding = [Text.Encoding]::UTF8
Push-Location $root
try { & $exe @serverArgs }
finally { Pop-Location }
