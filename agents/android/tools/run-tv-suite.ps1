<#
.SYNOPSIS
    Run the Impresario Android instrumented suites on a device with a stall
    watchdog and automatic recovery.

.DESCRIPTION
    The TCL Android TV vendor stack (Android 11) freezes long-lived background
    processes: with the agent app's guard service running (I-69) the first
    `doFreeze` on the agent process lands ~223 s after the process started
    (measured +222 s and +223 s in consecutive attempts, I-68), and a freeze
    that hits an in-flight UiAutomation call wedges the instrumentation
    forever. Running driver + wire classes in one process always crossed that
    line during the wire test, so each class set now runs in its own fresh
    `am instrument` process - the freeze timer restarts with the process.

    Recovery is always the same: force-stop the instrumentation package
    (com.impresario.agent) so the slot is released, then start fresh.

    This script automates the whole cycle:
      1. preflight: adb connect + stay-awake settings (I-67) + a UiAutomation
         probe (uiautomator dump) with a recovery retry;
      2. per attempt, per class set: force-stop agent + stub (fresh process,
         fresh UiAutomation slot), start `am instrument`, watch the output
         file - no new output for -StallSeconds means stalled: force-stop the
         agent and force the set to end;
      3. non-green attempts are also retried (flakes, post-install slowness);
      4. prints a per-set summary and exits 0 only when an attempt was fully
         green (every set ran to completion with 0 failures - JUnit prints
         `OK (N tests)` for that; its `Tests run: N,  Failures: M` form
         appears only when something failed).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File agents/android/tools/run-tv-suite.ps1

.EXAMPLE
    powershell -File agents/android/tools/run-tv-suite.ps1 `
        -Serial 192.168.1.5:5555 -StallSeconds 90 -MaxAttempts 4
#>
param(
    [string]$Serial = "192.168.93.84:5555",
    # One `am instrument` invocation per entry (fresh process = fresh freeze
    # timer): the driver class (~3 min) and the wire class (~2 min) are kept
    # separate on purpose - see the description.
    [string[]]$ClassSets = @(
        "com.impresario.agent.DriverInstrumentedTest",
        "com.impresario.agent.AgentServerInstrumentedTest"
    ),
    # The wire test is one test that prints nothing for ~90 s (longer on a
    # loaded host); the watchdog only needs to catch a wedged run, not a slow
    # one - the driver suite prints per test, so silence there is suspicious.
    [int]$StallSeconds = 240,
    [int]$MaxAttempts = 3,
    [string]$AgentPackage = "com.impresario.agent",
    [string]$StubPackage = "com.impresario.stub",
    [string]$Runner = "com.impresario.agent.test/androidx.test.runner.AndroidJUnitRunner",
    [string]$LogDir = "$env:TEMP\impresario-suite",
    [switch]$SkipPreflight
)

$ErrorActionPreference = "Stop"

function Resolve-Adb {
    $candidates = @(
        $(if ($env:ANDROID_HOME) { Join-Path $env:ANDROID_HOME "platform-tools\adb.exe" }),
        (Join-Path $env:LOCALAPPDATA "Android\Sdk\platform-tools\adb.exe")
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }
    throw "adb.exe not found (set ANDROID_HOME or install the Android SDK)"
}

$adb = Resolve-Adb
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

function Invoke-AdbShell {
    param([string]$Command)
    & $adb -s $Serial shell $Command 2>&1 | ForEach-Object { $_ -replace "`r", "" }
}

function Stop-AgentAndStub {
    # Force-stopping the agent releases a stale UiAutomation registration held
    # by a previous (crashed or killed) instrumentation process.
    Invoke-AdbShell "am force-stop $AgentPackage" | Out-Null
    Invoke-AdbShell "am force-stop $StubPackage" | Out-Null
}

function Test-AccessibilityProbe {
    $probe = Invoke-AdbShell "uiautomator dump /sdcard/impresario-probe.xml"
    Invoke-AdbShell "rm -f /sdcard/impresario-probe.xml" | Out-Null
    return ($probe -join "`n") -match "dumped to"
}

# -- preflight ---------------------------------------------------------------
if (-not $SkipPreflight) {
    Write-Host "== preflight $Serial =="
    (& $adb connect $Serial 2>&1) | ForEach-Object { $_ -replace "`r", "" } |
        Select-Object -First 1 | Out-Host
    foreach ($setting in @(
            "svc power stayon true",
            "settings put global stay_on_while_plugged_in 7",
            "settings put system screen_off_timeout 2147483647",
            "settings put secure screensaver_enabled 0",
            "dumpsys deviceidle whitelist +$AgentPackage",
            "cmd appops set $AgentPackage RUN_ANY_IN_BACKGROUND allow"
        )) {
        Invoke-AdbShell $setting | Out-Null
    }
    Stop-AgentAndStub
    Start-Sleep -Seconds 4
    if (-not (Test-AccessibilityProbe)) {
        Write-Warning "accessibility layer did not answer the probe - recovery retry"
        Stop-AgentAndStub
        Start-Sleep -Seconds 6
        if (-not (Test-AccessibilityProbe)) {
            Write-Warning "accessibility layer still not answering; continuing anyway"
        }
    }
}

# -- one class set in its own process ----------------------------------------
function Invoke-ClassSet {
    param(
        [string]$Label,
        [string]$Classes,
        [string]$Log,
        [string]$ErrLog,
        [string]$DeviceLog
    )
    Invoke-AdbShell "logcat -c" | Out-Null
    $instrumentArgs = @(
        "-s", $Serial, "shell", "am", "instrument", "-w", "-r",
        "-e", "class", $Classes, $Runner
    )
    $process = Start-Process -FilePath $adb -ArgumentList $instrumentArgs -PassThru -NoNewWindow `
        -RedirectStandardOutput $Log -RedirectStandardError $ErrLog
    $started = Get-Date
    $stalled = $false

    while (-not $process.HasExited) {
        Start-Sleep -Seconds 5
        $item = Get-Item $Log -ErrorAction SilentlyContinue
        if ($item -and ((Get-Date) - $item.LastWriteTime).TotalSeconds -gt $StallSeconds) {
            $stalled = $true
            Write-Warning "stalled: no instrumentation output for $StallSeconds s; force-stopping $AgentPackage to release the UiAutomation slot"
            Stop-AgentAndStub
            if (-not $process.WaitForExit(20000)) { $process.Kill() }
        }
    }

    $seconds = [int]((Get-Date) - $started).TotalSeconds
    Invoke-AdbShell "logcat -d -v time | grep -E 'impresario|FreezeManager|Accessibility|ANR|uiautomator'" |
        Out-File -FilePath $DeviceLog -Encoding utf8

    $text = if (Test-Path $Log) { Get-Content $Log -Raw } else { "" }
    # JUnit's text runner prints `Tests run: N,  Failures: M` only when tests
    # fail; a fully green run ends with `OK (N tests)` instead.
    $failMatch = [regex]::Match($text, "Tests run: (\d+),\s+Failures: (\d+)")
    $okMatch = [regex]::Match($text, "OK \((\d+) tests?\)")
    $tests = 0
    $failures = 0
    $green = $false
    if ($failMatch.Success) {
        $tests = [int]$failMatch.Groups[1].Value
        $failures = [int]$failMatch.Groups[2].Value
        $green = $failures -eq 0
    } elseif ($okMatch.Success) {
        $tests = [int]$okMatch.Groups[1].Value
        $green = $true
    }
    $state = if ($green) { "GREEN" } elseif ($stalled) { "STALLED" } else { "FAILED" }
    return [pscustomobject]@{
        Label    = $Label
        Seconds  = $seconds
        State    = $state
        Green    = $green
        Tests    = $tests
        Failures = $failures
        Log      = $Log
    }
}

# -- attempts ----------------------------------------------------------------
$outcome = "RED"
$reports = @()

for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
    Write-Host "== attempt $attempt/$MaxAttempts (stall watchdog: $StallSeconds s, $($ClassSets.Count) class sets) =="
    $attemptGreen = $true
    $greenLogs = @()

    for ($set = 0; $set -lt $ClassSets.Count; $set++) {
        $label = if ($ClassSets.Count -eq 2 -and $set -eq 1) { "wire" } elseif ($set -eq 0) { "driver" } else { "set$($set + 1)" }
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        $log = Join-Path $LogDir ("attempt-{0}-{1}-{2}.log" -f $attempt, $label, $stamp)
        Write-Host "   -- $label in a fresh process: $($ClassSets[$set])"

        Stop-AgentAndStub
        Start-Sleep -Seconds 3
        $result = Invoke-ClassSet -Label $label -Classes $ClassSets[$set] -Log $log -ErrLog "$log.err" -DeviceLog "$log.device"

        $reports += ("a{0} {1,-6} {2,5}s  tests={3} failures={4}  {5}" -f `
                $attempt, $result.Label, $result.Seconds, $result.Tests, $result.Failures, (Split-Path $log -Leaf))
        Write-Host ("   {0,5}s  {1,-7} tests={2} failures={3} -> {4}" -f `
                $result.Seconds, $result.State, $result.Tests, $result.Failures, $log)

        if ($result.Green) {
            $greenLogs += $log
        } else {
            $attemptGreen = $false
            Get-Content $log -Tail 12 | ForEach-Object { "   | $_" }
        }
    }

    if ($attemptGreen) {
        $outcome = "GREEN"
        foreach ($l in $greenLogs) {
            $suffix = (Split-Path $l -Leaf) -replace '^attempt-\d+-', ''
            Copy-Item $l (Join-Path $LogDir ("suite-latest-{0}" -f $suffix)) -Force
        }
        break
    }
}

Write-Host ""
Write-Host "== summary =="
$reports | ForEach-Object { Write-Host $_ }

if ($outcome -eq "GREEN") {
    Write-Host "GREEN - every class set passed (fresh process each); copies: $LogDir\suite-latest-*.log"
    exit 0
}

Write-Warning "no attempt was green; logs in $LogDir"
exit 1
