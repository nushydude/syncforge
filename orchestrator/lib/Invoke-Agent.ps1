# Invoke Cursor CLI agent (fresh session per call — no --resume)
param(
    [Parameter(Mandatory)]
    [string]$PromptFile,
    [Parameter(Mandatory)]
    [string]$LogFile,
    [string]$Model = "composer-2.5",
    [string]$Workspace = (Get-Location).Path,
    [ValidateSet("agent", "ask", "plan")]
    [string]$Mode = "agent",
    [switch]$ReadOnly
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command agent -ErrorAction SilentlyContinue)) {
    throw "Cursor CLI 'agent' not found. Run: irm 'https://cursor.com/install?win32=true' | iex"
}

$prompt = Get-Content -LiteralPath $PromptFile -Raw -Encoding UTF8

$args = @(
    "-p", $prompt,
    "--model", $Model,
    "--workspace", $Workspace,
    "--output-format", "text",
    "-f",
    "--trust",
    "--approve-mcps"
)

if ($ReadOnly -or $Mode -eq "ask") {
    $args += @("--mode", "ask")
}

$logDir = Split-Path -Parent $LogFile
if ($logDir -and -not (Test-Path $logDir)) {
    New-Item -ItemType Directory -Force -Path $logDir | Out-Null
}

$header = @"
=== Agent invocation $(Get-Date -Format o) ===
Model: $Model
Mode: $(if ($ReadOnly) { 'ask' } else { $Mode })
Workspace: $Workspace
PromptFile: $PromptFile
===
"@

Set-Content -LiteralPath $LogFile -Value $header -Encoding UTF8

Push-Location $Workspace
try {
    $output = & agent @args 2>&1 | Out-String
    Add-Content -LiteralPath $LogFile -Value $output -Encoding UTF8
    return $output
}
finally {
    Pop-Location
}
