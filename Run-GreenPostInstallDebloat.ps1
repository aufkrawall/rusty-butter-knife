#Requires -Version 5.1
param(
    # Extra arguments passed through to GreenPostInstallDebloatNative.exe,
    # e.g.: .\Run-GreenPostInstallDebloat.ps1 --include-ngx --no-color
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$PassthroughArgs
)

$ErrorActionPreference = 'Stop'

# Relaunch this script with administrator privileges if necessary.
$currentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
$currentPrincipal = [Security.Principal.WindowsPrincipal]::new($currentIdentity)
$isAdministrator = $currentPrincipal.IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator
)

if (-not $isAdministrator) {
    $arguments = @(
        '-NoProfile'
        '-ExecutionPolicy', 'Bypass'
        '-File', "`"$PSCommandPath`""
    )
    if ($PassthroughArgs) { $arguments += $PassthroughArgs }

    Start-Process `
        -FilePath 'powershell.exe' `
        -Verb RunAs `
        -ArgumentList $arguments

    exit
}

$executable = Join-Path $PSScriptRoot 'GreenPostInstallDebloatNative.exe'

if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    Write-Error "Executable not found: $executable"
    exit 1
}

$argList = @(
    '--execute'
    '--kill-lockers'
    '--disable-services'
    '--delete-scheduled-tasks'
    '--schedule-reboot-delete'
)
if ($PassthroughArgs) { $argList += $PassthroughArgs }

$process = Start-Process `
    -FilePath $executable `
    -ArgumentList $argList `
    -WorkingDirectory $PSScriptRoot `
    -Wait `
    -PassThru

Write-Host ("GreenPostInstallDebloatNative finished with exit code {0}." -f $process.ExitCode)

# The elevated console would close immediately otherwise, hiding the results.
# Set GPD_NO_PAUSE=1 to skip this when automating.
if ([Environment]::UserInteractive -and -not $env:GPD_NO_PAUSE) {
    Read-Host 'Press Enter to close' | Out-Null
}

exit $process.ExitCode
