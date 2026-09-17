[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Version,

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputDir = "dist/native/windows",

    [Parameter(Mandatory = $false)]
    [string]$BinaryPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$installerScript = (Resolve-Path (Join-Path $PSScriptRoot "installer.nsi")).Path
$iconPath = (Resolve-Path (Join-Path $projectRoot "native\assets\app-icon.ico")).Path

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

    throw "signtool.exe was not found. Install a Windows SDK before signing ListenOS."
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

if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
    $BinaryPath = Join-Path $projectRoot "native\target\release\listenos-native.exe"
}
$BinaryPath = (Resolve-Path $BinaryPath).Path
Invoke-CodeSign $BinaryPath

if ([System.IO.Path]::IsPathRooted($OutputDir)) {
    $resolvedOutputDir = $OutputDir
} else {
    $resolvedOutputDir = Join-Path $projectRoot $OutputDir
}
[System.IO.Directory]::CreateDirectory($resolvedOutputDir) | Out-Null
$resolvedOutputDir = (Resolve-Path $resolvedOutputDir).Path

$makensis = Get-Command makensis -ErrorAction SilentlyContinue
if ($null -eq $makensis) {
    throw "makensis was not found on PATH. Install NSIS before packaging ListenOS."
}

$artifactName = "ListenOS-$Version-Setup-x86_64.exe"
$artifactPath = Join-Path $resolvedOutputDir $artifactName

& $makensis.Source `
    "/V2" `
    "/DAPP_VERSION=$Version" `
    "/DBUILD_EXE=$BinaryPath" `
    "/DICON_FILE=$iconPath" `
    "/DOUTPUT_FILE=$artifactPath" `
    $installerScript

if ($LASTEXITCODE -ne 0) {
    throw "makensis failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $artifactPath -PathType Leaf)) {
    throw "NSIS completed without producing $artifactPath"
}

Invoke-CodeSign $artifactPath

$hash = Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256
$hashPath = Join-Path $resolvedOutputDir "$artifactName.sha256"
"$($hash.Hash.ToLowerInvariant())  $artifactName" | Set-Content -LiteralPath $hashPath -Encoding ascii

Write-Host "Created $artifactPath"
Write-Host "SHA256 $($hash.Hash.ToLowerInvariant())"
