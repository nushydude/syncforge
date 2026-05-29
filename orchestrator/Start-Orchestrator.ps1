# Wait for CLI auth, then run all stories until done or STOP
param(
    [int]$PollSeconds = 10,
    [int]$MaxWaitMinutes = 30
)

$ErrorActionPreference = "Stop"
$deadline = (Get-Date).AddMinutes($MaxWaitMinutes)

Write-Host "Waiting for 'agent login' to complete..."
while ((Get-Date) -lt $deadline) {
    $status = & agent status 2>&1 | Out-String
    if ($status -notmatch "Not logged in") {
        Write-Host "Authenticated. Starting orchestrator."
        & "$PSScriptRoot\Run-All.ps1"
        exit $LASTEXITCODE
    }
    Start-Sleep -Seconds $PollSeconds
}

Write-Error "Timed out waiting for agent login. Run: agent login"
exit 1
