$ErrorActionPreference = 'Stop'
$previousProgressPreference = $ProgressPreference
$ProgressPreference = 'SilentlyContinue'

$manifestUrl = 'https://github.com/devrajmahar/origin-speak/releases/latest/download/bootstrap-update.json'
$staging = Join-Path ([System.IO.Path]::GetTempPath()) ("origin-speak-install-" + [Guid]::NewGuid().ToString('N'))

function Download-VerifiedFile {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Sha256
    )

    Invoke-WebRequest -Uri $Url -OutFile $Path -UseBasicParsing
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Sha256.ToLowerInvariant()) {
        throw "SHA-256 verification failed for $([System.IO.Path]::GetFileName($Path))"
    }
}

try {
    New-Item -ItemType Directory -Path $staging -Force | Out-Null

    Write-Host ''
    Write-Host '  Origin Speak' -ForegroundColor Cyan
    Write-Host '  Local voice-to-text'
    Write-Host ''
    Write-Host '[1/3] Resolving latest release...'
    $manifestPath = Join-Path $staging 'bootstrap-update.json'
    Invoke-WebRequest -Uri $manifestUrl -OutFile $manifestPath -UseBasicParsing
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json

    $platform = $manifest.platforms.'windows-x86_64'
    if ($null -eq $platform) {
        throw 'Latest Origin Speak release does not contain a Windows x86_64 CLI package.'
    }

    $managerPath = Join-Path $staging $platform.manager.path
    $runtimePath = Join-Path $staging $platform.runtime.path

    Write-Host "[2/3] Downloading Origin Speak $($manifest.version)..."
    Write-Host '      CLI manager'
    Download-VerifiedFile -Url $platform.manager.url -Path $managerPath -Sha256 $platform.manager.sha256
    Write-Host '      Resident runtime'
    Download-VerifiedFile -Url $platform.runtime.url -Path $runtimePath -Sha256 $platform.runtime.sha256
    Write-Host '      Checksums verified' -ForegroundColor Green

    Write-Host '[3/3] Running first-time setup...'
    & $managerPath setup
    if ($LASTEXITCODE -ne 0) {
        throw "Origin Speak setup exited with code $LASTEXITCODE"
    }

    Write-Host ''
    Write-Host '  Origin Speak is installed.' -ForegroundColor Green
    Write-Host '  Open a new terminal and run: origin status'
}
finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
    }
    $ProgressPreference = $previousProgressPreference
}
