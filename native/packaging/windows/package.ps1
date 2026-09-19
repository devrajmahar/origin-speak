[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Version,

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputDir = "dist/native/windows",

    [Parameter(Mandatory = $false)]
    [string]$ManagerBinaryPath,

    [Parameter(Mandatory = $false)]
    [string]$RuntimeBinaryPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path

function Resolve-SignTool {
    $fromPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($null -ne $fromPath) {
        return $fromPath.Source
    }

    $windowsKits = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $windowsKits) {
        $candidate = Get-ChildItem -LiteralPath $windowsKits -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
            Where-Object { Test-Path -LiteralPath $_ } |
            Select-Object -First 1
        if ($candidate) {
            return $candidate
        }
    }

    throw "signtool.exe was not found. Install a Windows SDK before signing Origin Speak."
}

function Invoke-CodeSign([string]$Path) {
    $certificatePath = $env:WINDOWS_SIGNING_CERTIFICATE_PATH
    if ([string]::IsNullOrWhiteSpace($certificatePath)) {
        return
    }
    if (-not (Test-Path -LiteralPath $certificatePath -PathType Leaf)) {
        throw "WINDOWS_SIGNING_CERTIFICATE_PATH does not identify a PFX file."
    }
    if ([string]::IsNullOrWhiteSpace($env:WINDOWS_SIGNING_CERTIFICATE_PASSWORD)) {
        throw "WINDOWS_SIGNING_CERTIFICATE_PASSWORD is required when Windows signing is enabled."
    }
    if ([string]::IsNullOrWhiteSpace($env:WINDOWS_SIGN_TIMESTAMP_URL)) {
        throw "WINDOWS_SIGN_TIMESTAMP_URL is required when Windows signing is enabled."
    }

    $signTool = Resolve-SignTool
    & $signTool sign `
        /fd SHA256 `
        /td SHA256 `
        /tr $env:WINDOWS_SIGN_TIMESTAMP_URL `
        /f $certificatePath `
        /p $env:WINDOWS_SIGNING_CERTIFICATE_PASSWORD `
        $Path
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed to sign $Path with exit code $LASTEXITCODE"
    }

    & $signTool verify /pa /v $Path
    if ($LASTEXITCODE -ne 0) {
        throw "signtool could not verify $Path after signing."
    }
}

function Resolve-RequiredBinary([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label binary is missing: $Path"
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

if ([string]::IsNullOrWhiteSpace($ManagerBinaryPath)) {
    $ManagerBinaryPath = Join-Path $projectRoot "native\target\release\origin.exe"
}
if ([string]::IsNullOrWhiteSpace($RuntimeBinaryPath)) {
    $RuntimeBinaryPath = Join-Path $projectRoot "native\target\release\origin-runtime.exe"
}

$ManagerBinaryPath = Resolve-RequiredBinary $ManagerBinaryPath "Origin Speak CLI manager"
$RuntimeBinaryPath = Resolve-RequiredBinary $RuntimeBinaryPath "Origin Speak silent runtime"
if ($ManagerBinaryPath -eq $RuntimeBinaryPath) {
    throw "CLI manager and runtime must be separate binaries."
}

if ([System.IO.Path]::IsPathRooted($OutputDir)) {
    $resolvedOutputDir = $OutputDir
} else {
    $resolvedOutputDir = Join-Path $projectRoot $OutputDir
}
[System.IO.Directory]::CreateDirectory($resolvedOutputDir) | Out-Null
$resolvedOutputDir = (Resolve-Path $resolvedOutputDir).Path

$managerName = "origin-speak-$Version-windows-x86_64.exe"
$runtimeName = "origin-speak-runtime-$Version-windows-x86_64.exe"
$managerArtifact = Join-Path $resolvedOutputDir $managerName
$runtimeArtifact = Join-Path $resolvedOutputDir $runtimeName

Copy-Item -LiteralPath $ManagerBinaryPath -Destination $managerArtifact -Force
Copy-Item -LiteralPath $RuntimeBinaryPath -Destination $runtimeArtifact -Force
Invoke-CodeSign $managerArtifact
Invoke-CodeSign $runtimeArtifact

$sumLines = @()
foreach ($artifact in @($managerArtifact, $runtimeArtifact)) {
    $hash = Get-FileHash -LiteralPath $artifact -Algorithm SHA256
    $name = [System.IO.Path]::GetFileName($artifact)
    $line = "$($hash.Hash.ToLowerInvariant())  $name"
    $line | Set-Content -LiteralPath "$artifact.sha256" -Encoding ascii
    $sumLines += $line
}
$sumLines | Set-Content -LiteralPath (Join-Path $resolvedOutputDir "SHA256SUMS-windows.txt") -Encoding ascii

Write-Host "Created $managerArtifact"
Write-Host "Created $runtimeArtifact"
