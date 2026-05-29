param(
    [Parameter(Mandatory)]
    [string]$TemplateFile,
    [Parameter(Mandatory)]
    [hashtable]$Vars,
    [Parameter(Mandatory)]
    [string]$OutFile
)

$content = Get-Content -LiteralPath $TemplateFile -Raw -Encoding UTF8
foreach ($key in $Vars.Keys) {
    $content = $content.Replace("{{$key}}", [string]$Vars[$key])
}

$dir = Split-Path -Parent $OutFile
if ($dir -and -not (Test-Path $dir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

Set-Content -LiteralPath $OutFile -Value $content -Encoding UTF8
return $OutFile
