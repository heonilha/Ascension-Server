# Run every automated check: unit tests (terrain, monsters), Rust port checks + bot scenarios against two private test servers.
# Rust port: the C++ build writes a movement record, which must equal the committed one; then "cargo test" checks that
# the Rust crates (rust\) reproduce it bit for bit through FFI. Prints a SKIP line if cargo is not installed.
# Server A (-Port): PvE rooms are empty except for a timed slam pattern, so dodge timing checks stay exact.
# Server B (-Port + 1): PvE rooms have server monsters (spawning, chasing, attacking).
#   .\Scripts\Test.ps1               # builds first, prints one PASS/FAIL line per scenario (about 50 s)
#   .\Scripts\Test.ps1 -NoBuild
#   .\Scripts\Test.ps1 -UpdateGolden # after changing protocol\AscensionMovement.h on purpose: rewrite the committed record
# Logs: build\windows\test-logs\<scenario>.log and server.log. Exit code 1 if anything failed.
# Scenarios run in parallel, each in its own rooms (distinct --team-base).
param(
    [int]$Port = 17777,
    [switch]$NoBuild,
    [switch]$UpdateGolden
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $NoBuild) { & (Join-Path $PSScriptRoot 'Build.ps1') }
$bin = Join-Path $root 'build\windows\bin'
$logs = Join-Path $root 'build\windows\test-logs'
if (Test-Path $logs) { Remove-Item -Recurse -Force $logs }
New-Item -ItemType Directory -Force $logs | Out-Null

$scenarios = [ordered]@{
    # movement (server authoritative, prediction must match)
    'move_teams'      = '--bots 7 --seconds 5 --team-base 100'
    'room_full'       = '--bots 4 --team-size 4 --team-base 200'
    'move_speedhack'  = '--bots 3 --cheat --team-base 300'
    'move_dodge_spam' = '--bots 6 --burst --team-base 400'
    'move_wall'       = '--bots 1 --wall --seconds 6 --team-base 500'
    'move_lat100'     = '--bots 3 --seconds 6 --latency 100 --team-base 600'
    'move_lat300'     = '--bots 3 --seconds 6 --latency 300 --team-base 700'
    # dodge judgement, PvE (server slam pattern) at one-way latencies
    'pve_dodge_L0'    = '--dodge-test --bots 1 --seconds 14 --latency 0 --team-base 1000'
    'pve_dodge_L40'   = '--dodge-test --bots 1 --seconds 14 --latency 40 --team-base 1001'
    'pve_dodge_L100'  = '--dodge-test --bots 1 --seconds 14 --latency 100 --team-base 1002'
    'pve_dodge_L150'  = '--dodge-test --bots 1 --seconds 14 --latency 150 --team-base 1003'
    'pve_dodge_L300'  = '--dodge-test --bots 1 --seconds 14 --latency 300 --team-base 1004'
    'pve_stamp_cheat' = '--dodge-test --dodge-cheat --bots 1 --seconds 16 --team-base 1005'
    'pve_rtt_lie'     = '--dodge-test --dodge-cheat --rtt-lie 2000 --bots 1 --seconds 16 --team-base 1006'
    # dodge judgement, PvP (bot 0 attacks bot 1)
    'pvp_dodge_L0'    = '--mode pvp --dodge-test --bots 2 --team-size 2 --seconds 14 --latency 0 --team-base 1100'
    'pvp_dodge_L50'   = '--mode pvp --dodge-test --bots 2 --team-size 2 --seconds 14 --latency 50 --team-base 1101'
    'pvp_dodge_L100'  = '--mode pvp --dodge-test --bots 2 --team-size 2 --seconds 14 --latency 100 --team-base 1102'
    'pvp_dodge_L150'  = '--mode pvp --dodge-test --bots 2 --team-size 2 --seconds 14 --latency 150 --team-base 1103'
    'pvp_stamp_cheat' = '--mode pvp --dodge-test --dodge-cheat --bots 2 --team-size 2 --seconds 16 --team-base 1104'
    'pvp_rtt_lie'     = '--mode pvp --dodge-test --dodge-cheat --rtt-lie 2000 --bots 2 --team-size 2 --seconds 16 --team-base 1105'
}
# Monster scenarios run against server B.
$monsterScenarios = [ordered]@{
    'mon_hunt'        = '--hunt --bots 1 --seconds 20 --team-base 2000'
    'mon_team_watch'  = '--monsters --bots 3 --seconds 20 --team-base 2100'
    'mon_wave_cheat'  = '--wave-cheat --bots 1 --seconds 12 --team-base 2200'
    'mon_cap_48'      = '--monsters --expect-cap --bots 2 --team-size 2 --seconds 40 --team-base 2300'
    'mon_hunt_lat150' = '--hunt --bots 1 --seconds 20 --latency 150 --team-base 2400'
}

[Console]::OutputEncoding = [Text.Encoding]::UTF8 # unit test output is UTF-8
$failed = 0
Push-Location $root
try {
    & (Join-Path $bin 'ascension_terrain_test.exe') | Out-File -Encoding utf8 (Join-Path $logs 'terrain_test.log')
    $ok = $LASTEXITCODE -eq 0
    if (-not $ok) { $failed++ }
    Write-Host ('{0,-18} {1}' -f 'terrain_test', $(if ($ok) { 'PASS' } else { 'FAIL' }))
    & (Join-Path $bin 'ascension_monster_test.exe') | Out-File -Encoding utf8 (Join-Path $logs 'monster_test.log')
    $ok = $LASTEXITCODE -eq 0
    if (-not $ok) { $failed++ }
    Write-Host ('{0,-18} {1}' -f 'monster_test', $(if ($ok) { 'PASS' } else { 'FAIL' }))

    $goldenFile = Join-Path $root 'rust\crates\movement\tests\golden\movement_v4.txt'
    $goldenNow = Join-Path $logs 'movement_golden.txt'
    & (Join-Path $bin 'ascension_movement_golden.exe') 'data\CombatLab.txt' $goldenNow
    $ok = $LASTEXITCODE -eq 0
    if ($ok -and $UpdateGolden) { Copy-Item $goldenNow $goldenFile -Force }
    $ok = $ok -and (((Get-Content $goldenNow -Raw) -replace "`r`n", "`n") -ceq ((Get-Content $goldenFile -Raw) -replace "`r`n", "`n"))
    if (-not $ok) { $failed++ }
    Write-Host ('{0,-18} {1}' -f 'movement_golden', $(if ($ok) { 'PASS' } else { 'FAIL' }))

    $extraChecks = 3
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        $extraChecks = 4
        $manifest = Join-Path $root 'rust\Cargo.toml'
        $rustLog = Join-Path $logs 'rust_tests.log'
        cmd /c "cargo test --workspace --manifest-path `"$manifest`" > `"$rustLog`" 2>&1"
        $ok = $LASTEXITCODE -eq 0
        if (-not $ok) { $failed++ }
        Write-Host ('{0,-18} {1}' -f 'rust_tests', $(if ($ok) { 'PASS' } else { 'FAIL' }))
    }
    else { Write-Host ('{0,-18} {1}' -f 'rust_tests', 'SKIP (cargo not found: install Rust from https://rustup.rs)') }

    $server = Start-Process -FilePath (Join-Path $bin 'ascension_server.exe') -ArgumentList "--port $Port --threads 4 --pve-monsters 0 --pve-slam-ms 1500" `
        -RedirectStandardOutput (Join-Path $logs 'server.log') -RedirectStandardError (Join-Path $logs 'server.err.log') -PassThru -NoNewWindow
    $monsterPort = $Port + 1
    $monsterServer = Start-Process -FilePath (Join-Path $bin 'ascension_server.exe') -ArgumentList "--port $monsterPort --threads 4" `
        -RedirectStandardOutput (Join-Path $logs 'server_monsters.log') -RedirectStandardError (Join-Path $logs 'server_monsters.err.log') -PassThru -NoNewWindow
    Start-Sleep -Milliseconds 700
    if ($server.HasExited -or $monsterServer.HasExited) { throw "Test server did not start (port $Port or $monsterPort busy?). See $logs" }

    $running = @()
    $all = @()
    foreach ($name in $scenarios.Keys) { $all += [pscustomobject]@{ Name = $name; Arguments = "--port $Port $($scenarios[$name])" } }
    foreach ($name in $monsterScenarios.Keys) { $all += [pscustomobject]@{ Name = $name; Arguments = "--port $monsterPort $($monsterScenarios[$name])" } }
    foreach ($scenario in $all) {
        $name = $scenario.Name
        $p = Start-Process -FilePath (Join-Path $bin 'ascension_bot.exe') -ArgumentList $scenario.Arguments `
            -RedirectStandardOutput (Join-Path $logs "$name.log") -RedirectStandardError (Join-Path $logs "$name.err.log") -PassThru -NoNewWindow
        $null = $p.Handle # keep the handle so ExitCode stays readable after exit
        $running += [pscustomobject]@{ Name = $name; Process = $p }
    }
    foreach ($item in $running) {
        if (-not $item.Process.WaitForExit(120000)) { $item.Process.Kill() }
        $ok = $item.Process.HasExited -and $item.Process.ExitCode -eq 0
        if (-not $ok) { $failed++ }
        Write-Host ('{0,-18} {1}' -f $item.Name, $(if ($ok) { 'PASS' } else { 'FAIL' }))
    }
}
finally {
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force }
    if ($monsterServer -and -not $monsterServer.HasExited) { Stop-Process -Id $monsterServer.Id -Force }
    Pop-Location
    Get-ChildItem $logs -Filter '*.err.log' | Where-Object { $_.Length -eq 0 } | Remove-Item
}
if ($failed -gt 0) { Write-Host "$failed FAILED. Logs: $logs"; exit 1 }
Write-Host "ALL PASS ($($scenarios.Count + $monsterScenarios.Count + $extraChecks) checks). Logs: $logs"
