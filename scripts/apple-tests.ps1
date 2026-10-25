# Run the Apple simulator suite from Windows (or PowerShell on any OS).
#
#   pwsh scripts/apple-tests.ps1                      # iOS + iPadOS + tvOS
#   pwsh scripts/apple-tests.ps1 -Platform ios
#   pwsh scripts/apple-tests.ps1 -Platform tvos -Ref master
#
# This only needs GitHub CLI (`gh auth login`) and network access. GitHub's
# hosted macOS runner is the Xcode host; no local Mac is involved.
param(
    [ValidateSet("all", "ios", "ipados", "tvos")]
    [string]$Platform = "all",
    [string]$Ref = "",
    [string]$OutDir = "target/apple-simulator"
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw "GitHub CLI (gh) is required: https://cli.github.com/ then 'gh auth login'"
}
if (-not $Ref) {
    $Ref = (git rev-parse --abbrev-ref HEAD).Trim()
    if (-not $Ref) { throw "cannot infer a git branch; pass -Ref" }
}

$workflow = "apple-simulator.yml"
gh auth status *> $null
if ($LASTEXITCODE -ne 0) { throw "gh is not authenticated; run 'gh auth login'" }

Write-Host "Queueing $workflow on $Ref (platforms: $Platform)..." -ForegroundColor Cyan
$before = @(gh run list --workflow $workflow --branch $Ref --limit 50 --json databaseId --jq ".[].databaseId")
if ($LASTEXITCODE -ne 0) { throw "could not list existing runs" }

gh workflow run $workflow --ref $Ref -f "platforms=$Platform"
if ($LASTEXITCODE -ne 0) { throw "could not trigger $workflow" }

$runId = $null
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 3
    $ids = @(gh run list --workflow $workflow --branch $Ref --limit 50 --json databaseId --jq ".[].databaseId")
    $candidate = $ids | Where-Object { $before -notcontains $_ } | Select-Object -First 1
    if ($candidate) {
        $runId = $candidate
        break
    }
}
if (-not $runId) { throw "the workflow did not appear in 'gh run list' within 3 minutes" }

Write-Host "Watching run $runId..." -ForegroundColor Cyan
gh run watch $runId --interval 10 --exit-status
$watchExit = $LASTEXITCODE

$status = (gh run view $runId --json conclusion --jq ".conclusion").Trim()
$url = (gh run view $runId --json url --jq ".url").Trim()

if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
gh run download $runId -D $OutDir

Write-Host ""
Write-Host "Run: $url" -ForegroundColor Cyan
Write-Host "Status: $status" -ForegroundColor $(if ($status -eq "success") { "Green" } else { "Red" })
$logs = Get-ChildItem $OutDir -Recurse -File -Filter "*-e2e.log" -ErrorAction SilentlyContinue
foreach ($log in $logs) {
    Write-Host ""
    Write-Host "== $($log.FullName) ==" -ForegroundColor DarkGray
    Get-Content $log.FullName -Tail 30
}
if ($logs.Count -eq 0) {
    Write-Host "No e2e logs were downloaded; inspect $url for the build/smoke failure." -ForegroundColor Yellow
}

if ($status -ne "success") { exit 1 }
if ($watchExit -ne 0) { exit $watchExit }
