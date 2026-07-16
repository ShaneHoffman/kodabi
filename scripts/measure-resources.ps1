<#
.SYNOPSIS
    Samples kodabi.exe's (and its WebView2 helper processes') CPU% and
    working set at 1 Hz while a resource-budget measurement pass runs.

.DESCRIPTION
    Companion to docs/RESOURCE_BUDGET.md: an in-process sampler can only see
    kodabi.exe's own CPU/memory, not the msedgewebview2.exe helper processes
    Tauri spawns for the WebView2 renderer — this script samples both via
    Get-Counter so the idle/capturing numbers recorded in that doc reflect
    the whole app, not just the Rust side.

    Run this alongside a real measurement pass: start it, then drive the app
    through idle -> start capture -> join/hold a real meeting -> stop capture
    (per docs/RESOURCE_BUDGET.md's "How to reproduce"), and watch the CPU%
    column live for the target ceiling and listen for fan spin-up (the felt
    failure mode FOUNDING_DOC SS3.7 calls out — this script cannot detect
    that for you).

.PARAMETER Seconds
    How long to sample for. Default 600 (10 minutes) - long enough to cover
    idle, a capture window, and the post-stop transcription burst in one run.

.PARAMETER Out
    CSV path to append samples to. Default .\resources.csv in the current
    directory.

.EXAMPLE
    .\scripts\measure-resources.ps1 -Seconds 900 -Out meeting-01.csv
#>
param(
    [int]$Seconds = 600,
    [string]$Out = "resources.csv"
)

$logicalCores = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
if (-not (Test-Path $Out)) {
    "timestamp,process,pid,cpu_pct,working_set_mb" | Out-File -FilePath $Out -Encoding utf8
}

Write-Output "Sampling kodabi* and msedgewebview2* processes for ${Seconds}s (normalizing CPU% by $logicalCores logical cores) -> $Out"
Write-Output "Ctrl+C to stop early; partial samples are still written."

$samples = @{}
$deadline = (Get-Date).AddSeconds($Seconds)

while ((Get-Date) -lt $deadline) {
    $tick = Get-Date -Format "o"
    # Win32_PerfFormattedData_PerfProc_Process gives IDProcess directly, so
    # each row maps unambiguously to a process - no need to hand-construct a
    # \Process(name#N) counter instance path from a PID (N is a small
    # per-name instance index, not the PID, and getting that wrong just
    # silently drops every sample).
    $rows = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like "kodabi*" -or $_.Name -like "msedgewebview2*" }

    foreach ($row in $rows) {
        $cpuPct = [math]::Round($row.PercentProcessorTime / $logicalCores, 2)
        $workingSetMb = [math]::Round($row.WorkingSetPrivate / 1MB, 1)
        $name = ($row.Name -replace '#\d+$', '')

        "$tick,$name,$($row.IDProcess),$cpuPct,$workingSetMb" | Out-File -FilePath $Out -Append -Encoding utf8

        if (-not $samples.ContainsKey($name)) {
            $samples[$name] = @{ Cpu = @(); Mem = @() }
        }
        $samples[$name].Cpu += $cpuPct
        $samples[$name].Mem += $workingSetMb

        Write-Output ("{0}  {1,-20} pid={2,-7} cpu={3,6}%  ws={4,7}MB" -f $tick, $name, $row.IDProcess, $cpuPct, $workingSetMb)
    }

    Start-Sleep -Seconds 1
}

Write-Output ""
Write-Output "--- Summary (avg / peak) ---"
foreach ($key in $samples.Keys) {
    $cpu = $samples[$key].Cpu
    $mem = $samples[$key].Mem
    if ($cpu.Count -eq 0) { continue }
    $cpuAvg = [math]::Round(($cpu | Measure-Object -Average).Average, 2)
    $cpuPeak = [math]::Round(($cpu | Measure-Object -Maximum).Maximum, 2)
    $memAvg = [math]::Round(($mem | Measure-Object -Average).Average, 1)
    $memPeak = [math]::Round(($mem | Measure-Object -Maximum).Maximum, 1)
    Write-Output ("{0,-20} cpu avg={1,6}% peak={2,6}%   ws avg={3,7}MB peak={4,7}MB" -f $key, $cpuAvg, $cpuPeak, $memAvg, $memPeak)
}
Write-Output ""
Write-Output "Full samples written to $Out - copy the avg/peak numbers above into docs/RESOURCE_BUDGET.md."
