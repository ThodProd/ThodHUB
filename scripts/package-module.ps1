param(
    [Parameter(Mandatory=$true)][string]$PackageDirectory,
    [Parameter(Mandatory=$true)][string]$Repository,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [string]$Notes = 'Первый выпуск для ThodHUB.'
)
$ErrorActionPreference = 'Stop'
$manifest = Get-Content -LiteralPath (Join-Path $PackageDirectory 'manifest.json') -Raw | ConvertFrom-Json
if ($manifest.version -notmatch '^\d+\.\d+\.\d+$' -or $manifest.id -notmatch '^[a-z][a-z0-9-]+$') { throw 'Invalid manifest' }
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$name = "$($manifest.id)-$($manifest.version)-win-x64.zip"
$zip = Join-Path $OutputDirectory $name
if (Test-Path -LiteralPath $zip) { throw "Package already exists: $zip" }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory((Resolve-Path -LiteralPath $PackageDirectory).Path, $zip, [System.IO.Compression.CompressionLevel]::Optimal, $false)
$release = [ordered]@{
    version = $manifest.version
    packageUrl = "https://github.com/$Repository/releases/download/hub-v$($manifest.version)/$name"
    sha256 = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    size = (Get-Item -LiteralPath $zip).Length
    notes = $Notes
    platform = $manifest.platform
    hubApi = $manifest.hubApi
}
$jsonPath=Join-Path $OutputDirectory 'thodhub-release.json'
[IO.File]::WriteAllText($jsonPath,($release | ConvertTo-Json -Depth 5),[Text.UTF8Encoding]::new($false))
& (Join-Path $PSScriptRoot 'validate-package.ps1') -Package $zip -Release $jsonPath -Repository $Repository | Write-Host
Write-Output $zip
