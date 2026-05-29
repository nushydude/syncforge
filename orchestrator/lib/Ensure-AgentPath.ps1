function Ensure-AgentPath {
    $machine = [System.Environment]::GetEnvironmentVariable("Path", "Machine")
    $user = [System.Environment]::GetEnvironmentVariable("Path", "User")
    if ($machine -or $user) {
        $env:Path = "$machine;$user"
    }

    $candidates = @(
        "$env:LOCALAPPDATA\cursor-agent",
        "$env:USERPROFILE\.local\bin",
        "$env:USERPROFILE\AppData\Local\cursor-agent"
    )
    foreach ($dir in $candidates) {
        if (Test-Path $dir) {
            Get-ChildItem $dir -Recurse -Filter "agent.exe" -ErrorAction SilentlyContinue |
                Select-Object -First 1 -ExpandProperty DirectoryName |
                ForEach-Object {
                    if ($env:Path -notlike "*$_*") { $env:Path = "$_;$env:Path" }
                }
        }
    }
}
