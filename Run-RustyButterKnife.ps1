#Requires -Version 5.1
param(
    # Extra arguments passed through to RustyButterKnife.exe,
    # e.g.: .\Run-RustyButterKnife.ps1 --include-ngx --no-color
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$PassthroughArgs
)

$ErrorActionPreference = 'Stop'

# Start-Process -ArgumentList joins array elements with spaces WITHOUT
# quoting them, so arguments containing spaces or quotes must be pre-quoted
# here or they arrive split at the target (e.g. --log-file "C:\my logs\x.log").
function Format-NativeArgument {
    param([string]$Value)
    if ($Value -notmatch '[\s"]') { return $Value }
    # Windows CRT/argv rules: each run of k backslashes directly before a
    # quote becomes 2k backslashes plus the escaped quote (2k+1 total); any
    # trailing backslashes double once more before our closing quote.
    $escaped = $Value -replace '(\\+)(?=")', '$1$1'
    $q = [char]34
    $bs = [char]92
    $escaped = $escaped -replace $q, ($bs + $q)
    $escaped = $escaped -replace '\\+$', '$0$0'
    return '"' + $escaped + '"'
}

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
    if ($PassthroughArgs) {
        $arguments += ($PassthroughArgs | ForEach-Object { Format-NativeArgument $_ })
    }

    Start-Process `
        -FilePath (Join-Path $PSHOME 'powershell.exe') `
        -Verb RunAs `
        -ArgumentList $arguments

    exit
}

$candidatePaths = @(
    (Join-Path $PSScriptRoot 'RustyButterKnife.exe')
    (Join-Path $PSScriptRoot 'dist\x86_64\RustyButterKnife.exe')
    (Join-Path $PSScriptRoot 'dist\aarch64\RustyButterKnife.exe')
    (Join-Path $PSScriptRoot 'target\release\RustyButterKnife.exe')
    (Join-Path $PSScriptRoot 'GreenPostInstallDebloatNative.exe')
)

$executable = $candidatePaths | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1

if (-not $executable) {
    Write-Error ("Executable not found. Checked candidate paths:`n  " + ($candidatePaths -join "`n  "))
    exit 1
}

$argList = @(
    '--execute'
    '--kill-lockers'
    '--disable-services'
    '--delete-scheduled-tasks'
    '--schedule-reboot-delete'
)
if ($PassthroughArgs) {
    $argList += ($PassthroughArgs | ForEach-Object { Format-NativeArgument $_ })
}

$process = Start-Process `
    -FilePath $executable `
    -ArgumentList $argList `
    -WorkingDirectory $PSScriptRoot `
    -Wait `
    -PassThru

Write-Host ("Rusty Butter Knife finished with exit code {0}." -f $process.ExitCode)

# The elevated console would close immediately otherwise, hiding the results.
# Set RBK_NO_PAUSE=1 (or legacy GPD_NO_PAUSE=1) to skip this when automating.
if ([Environment]::UserInteractive -and -not $env:RBK_NO_PAUSE -and -not $env:GPD_NO_PAUSE) {
    Read-Host 'Press Enter to close' | Out-Null
}

exit $process.ExitCode
