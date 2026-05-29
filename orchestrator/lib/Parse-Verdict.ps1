function Parse-Verdict {
    param(
        [Parameter(Mandatory)]
        [string]$Output,
        [Parameter(Mandatory)]
        [ValidateSet("review", "approve", "implement")]
        [string]$Kind
    )

    switch ($Kind) {
        "review" {
            if ($Output -match "REVIEW_VERDICT:\s*APPROVED") { return "APPROVED" }
            if ($Output -match "REVIEW_VERDICT:\s*CHANGES_REQUESTED") { return "CHANGES_REQUESTED" }
            return "UNKNOWN"
        }
        "approve" {
            if ($Output -match "APPROVE_VERDICT:\s*YES") { return "YES" }
            if ($Output -match "APPROVE_VERDICT:\s*NO") { return "NO" }
            return "UNKNOWN"
        }
        "implement" {
            if ($Output -match "IMPLEMENT_STATUS:\s*DONE") { return "DONE" }
            if ($Output -match "IMPLEMENT_STATUS:\s*BLOCKED") { return "BLOCKED" }
            return "UNKNOWN"
        }
    }
}
