# Run all stories sequentially until done or STOP file / failure
param(
    [string]$ConfigPath = "$PSScriptRoot\stories.json",
    [string]$StartFrom = ""
)

$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
Set-Location $Root

# Preflight
if (-not (Get-Command agent -ErrorAction SilentlyContinue)) {
    Write-Error "Cursor CLI not installed. Run: irm 'https://cursor.com/install?win32=true' | iex"
}
$status = & agent status 2>&1 | Out-String
if ($status -match "Not logged in") {
    Write-Error @"
Cursor CLI is not logged in. Run once in a terminal:
  agent login
Then re-run: .\orchestrator\Run-All.ps1
"@
}

$config = Get-Content $ConfigPath -Raw | ConvertFrom-Json
$completed = @{}
$statePath = Join-Path $PSScriptRoot "state.json"
if (Test-Path $statePath) {
    $state = Get-Content $statePath -Raw | ConvertFrom-Json
    foreach ($s in @($state.stories)) {
        if ($s.status -eq "completed") { $completed[$s.id] = $true }
    }
}

$started = [bool]$StartFrom
foreach ($story in $config.stories) {
    if ($StartFrom -and -not $started) {
        if ($story.id -ne $StartFrom) { continue }
        $started = $true
    }

    if ($completed.ContainsKey($story.id)) {
        Write-Host "Skipping $($story.id) (already completed)."
        continue
    }

    foreach ($dep in @($story.dependsOn)) {
        if (-not $completed.ContainsKey($dep)) {
            Write-Error "Dependency $dep not completed before $($story.id). Run stories in order."
        }
    }

    if (Test-Path (Join-Path $PSScriptRoot "STOP")) {
        Write-Host "STOP file present. Orchestrator halted."
        exit 0
    }

    Write-Host "========== Starting $($story.id): $($story.title) =========="
    & "$PSScriptRoot\Run-Story.ps1" -StoryId $story.id -ConfigPath $ConfigPath
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Story $($story.id) failed with exit $LASTEXITCODE"
    }
    $completed[$story.id] = $true
}

Write-Host "========== All stories completed =========="
exit 0
