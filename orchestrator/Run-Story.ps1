# Run one story: implement -> review loop -> approve -> merge commit on main
param(
    [Parameter(Mandatory)]
    [string]$StoryId,
    [string]$ConfigPath = "$PSScriptRoot\stories.json"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
Set-Location $Root

. "$PSScriptRoot\lib\Ensure-AgentPath.ps1"
. "$PSScriptRoot\lib\Render-Prompt.ps1"
. "$PSScriptRoot\lib\Invoke-Agent.ps1"
. "$PSScriptRoot\lib\Parse-Verdict.ps1"
Ensure-AgentPath

$config = Get-Content $ConfigPath -Raw | ConvertFrom-Json
$story = $config.stories | Where-Object { $_.id -eq $StoryId } | Select-Object -First 1
if (-not $story) { throw "Story not found: $StoryId" }

$workspace = if ($config.workspace) { $config.workspace } else { $Root }
$model = if ($config.model) { $config.model } else { "composer-2.5" }
$maxIter = if ($config.maxReviewIterations) { [int]$config.maxReviewIterations } else { 5 }

$storyPath = Join-Path $Root ($story.storyFile -replace '/', '\')
$branch = $story.branch
$logBase = Join-Path $PSScriptRoot "logs\$StoryId"
New-Item -ItemType Directory -Force -Path $logBase | Out-Null

function Update-State($patch) {
    $statePath = Join-Path $PSScriptRoot "state.json"
    $state = if (Test-Path $statePath) {
        Get-Content $statePath -Raw | ConvertFrom-Json
    } else {
        [pscustomobject]@{ stories = @(); current = $null; startedAt = (Get-Date -Format o) }
    }
    # JSON deserializes stories as fixed-size Object[] (still IList) — always use ArrayList
    $list = [System.Collections.ArrayList]@()
    foreach ($s in @($state.stories)) { [void]$list.Add($s) }

    $existingIdx = -1
    for ($i = 0; $i -lt $list.Count; $i++) {
        if ($list[$i].id -eq $StoryId) { $existingIdx = $i; break }
    }

    if ($existingIdx -ge 0) {
        $existing = $list[$existingIdx]
        $hash = @{ id = $StoryId }
        foreach ($prop in $existing.PSObject.Properties) {
            if ($prop.Name -ne 'id') { $hash[$prop.Name] = $prop.Value }
        }
        foreach ($k in $patch.Keys) { $hash[$k] = $patch[$k] }
        $list[$existingIdx] = [pscustomobject]$hash
    } else {
        $hash = @{ id = $StoryId }
        foreach ($k in $patch.Keys) { $hash[$k] = $patch[$k] }
        [void]$list.Add([pscustomobject]$hash)
    }
    $state.stories = @($list.ToArray())
    $state.current = $StoryId
    $state | ConvertTo-Json -Depth 6 | Set-Content $statePath -Encoding UTF8
}

# Git: ensure on story branch from main (git writes to stderr; do not treat as terminating)
function Invoke-Git {
    param([string[]]$GitArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & git @GitArgs 2>&1 | Out-Null
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    return $code
}

if ((Invoke-Git @("rev-parse", "--verify", "main")) -ne 0) {
    Invoke-Git @("checkout", "-b", "main") | Out-Null
    Invoke-Git @("commit", "--allow-empty", "-m", "chore: initialize main") | Out-Null
}

Invoke-Git @("checkout", "main") | Out-Null
Invoke-Git @("checkout", "-B", $branch) | Out-Null

Update-State @{ status = "implementing"; branch = $branch; iteration = 0 }

$feedbackSection = ""
$reviewerOutput = ""
$approved = $false

for ($iter = 1; $iter -le $maxIter; $iter++) {
    if (Test-Path (Join-Path $PSScriptRoot "STOP")) {
        Write-Host "STOP file detected. Exiting story $StoryId."
        Update-State @{ status = "stopped" }
        exit 2
    }

    Update-State @{ status = "implementing"; iteration = $iter }

    $implVars = @{
        STORY_ID         = $StoryId
        STORY_TITLE      = $story.title
        WORKSPACE        = $workspace
        BRANCH           = $branch
        STORY_FILE       = $storyPath
        FEEDBACK_SECTION = $feedbackSection
    }
    $implPrompt = Join-Path $logBase "iter$iter-implement-prompt.md"
    Render-Prompt -TemplateFile "$PSScriptRoot\prompts\implementer.md" -Vars $implVars -OutFile $implPrompt

    Write-Host "[$StoryId] Implementer iteration $iter ..."
    $implLog = Join-Path $logBase "iter$iter-implement.log"
    $implOut = Invoke-Agent -PromptFile $implPrompt -LogFile $implLog -Model $model -Workspace $workspace
    $implStatus = Parse-Verdict -Output $implOut -Kind implement
    if ($implStatus -eq "BLOCKED") {
        Update-State @{ status = "blocked"; lastLog = $implLog }
        throw "Implementer blocked on $StoryId. See $implLog"
    }

    Update-State @{ status = "reviewing"; iteration = $iter }

    $revVars = @{
        STORY_FILE = $storyPath
        BRANCH     = $branch
    }
    $revPrompt = Join-Path $logBase "iter$iter-review-prompt.md"
    Render-Prompt -TemplateFile "$PSScriptRoot\prompts\reviewer.md" -Vars $revVars -OutFile $revPrompt

    Write-Host "[$StoryId] Reviewer (fresh session) iteration $iter ..."
    $revLog = Join-Path $logBase "iter$iter-review.log"
    $revOut = Invoke-Agent -PromptFile $revPrompt -LogFile $revLog -Model $model -Workspace $workspace -ReadOnly
    $reviewerOutput = $revOut
    $revVerdict = Parse-Verdict -Output $revOut -Kind review

    if ($revVerdict -eq "APPROVED") {
        $approved = $true
        break
    }

    $tail = if ($revOut.Length -gt 4000) { $revOut.Substring($revOut.Length - 4000) } else { $revOut }
    $feedbackSection = @"

## Reviewer feedback (iteration $iter)

$tail

Address every issue above before marking IMPLEMENT_STATUS: DONE.
"@
    Write-Host "[$StoryId] Changes requested. Re-running implementer ..."
}

if (-not $approved) {
    Update-State @{ status = "review_failed"; iteration = $maxIter }
    throw "Reviewer did not approve $StoryId after $maxIter iterations."
}

Update-State @{ status = "approving" }

$tailReviewer = if ($reviewerOutput.Length -gt 3000) {
    $reviewerOutput.Substring($reviewerOutput.Length - 3000)
} else { $reviewerOutput }

$appVars = @{
    STORY_FILE           = $storyPath
    BRANCH               = $branch
    REVIEWER_OUTPUT_TAIL = $tailReviewer
}
$appPrompt = Join-Path $logBase "approve-prompt.md"
Render-Prompt -TemplateFile "$PSScriptRoot\prompts\approver.md" -Vars $appVars -OutFile $appPrompt

Write-Host "[$StoryId] Approver (fresh session) ..."
$appLog = Join-Path $logBase "approve.log"
$appOut = Invoke-Agent -PromptFile $appPrompt -LogFile $appLog -Model $model -Workspace $workspace -ReadOnly
$appVerdict = Parse-Verdict -Output $appOut -Kind approve

if ($appVerdict -ne "YES") {
    Update-State @{ status = "approve_denied" }
    throw "Approver denied $StoryId. See $appLog"
}

# Merge story branch into main locally
Invoke-Git @("checkout", "main") | Out-Null
if ((Invoke-Git @("merge", "--no-ff", $branch, "-m", "merge: $StoryId $($story.title)")) -ne 0) {
    throw "git merge failed for $StoryId"
}

Update-State @{ status = "completed"; completedAt = (Get-Date -Format o) }
Write-Host "[$StoryId] Completed and merged to main."
exit 0
