function Parse-Verdict {
    param(
        [Parameter(Mandatory)]
        [string]$Output,
        [Parameter(Mandatory)]
        [ValidateSet("review", "approve", "implement")]
        [string]$Kind
    )

    $lines = @($Output -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 })
    if ($lines.Count -eq 0) { return "UNKNOWN" }
    $lastLine = $lines[$lines.Count - 1].Trim()

    switch ($Kind) {
        "review" {
            if ($lastLine -match "^REVIEW_VERDICT:\s*APPROVED$") { return "APPROVED" }
            if ($lastLine -match "^REVIEW_VERDICT:\s*CHANGES_REQUESTED$") { return "CHANGES_REQUESTED" }
            return "UNKNOWN"
        }
        "approve" {
            if ($lastLine -match "^APPROVE_VERDICT:\s*YES$") { return "YES" }
            if ($lastLine -match "^APPROVE_VERDICT:\s*NO$") { return "NO" }
            return "UNKNOWN"
        }
        "implement" {
            if ($lastLine -match "^IMPLEMENT_STATUS:\s*DONE$") { return "DONE" }
            if ($lastLine -match "^IMPLEMENT_STATUS:\s*BLOCKED$") { return "BLOCKED" }
            return "UNKNOWN"
        }
    }
}
