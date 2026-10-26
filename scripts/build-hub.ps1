param([string]$OutputDirectory = 'artifacts/release')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    dotnet run --project tests/ThodHub.Tests
    if ($LASTEXITCODE) { throw 'Tests failed' }
    $stage = Join-Path $root ('artifacts/hub-' + [guid]::NewGuid().ToString('N'))
    dotnet publish src/ThodHub -c Release -r win-x64 --self-contained true -o $stage --nologo
    if ($LASTEXITCODE) { throw 'Build failed' }
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
    $version = ([xml](Get-Content src/ThodHub/ThodHub.csproj)).Project.PropertyGroup.Version
    $zip = Join-Path ([IO.Path]::GetFullPath($OutputDirectory)) "ThodHUB-$version-win-x64.zip"
    if (Test-Path -LiteralPath $zip) { throw "Already exists: $zip" }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($stage,$zip,[IO.Compression.CompressionLevel]::Optimal,$false)
    (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + [IO.Path]::GetFileName($zip) | Set-Content (Join-Path $OutputDirectory 'SHA256SUMS.txt') -Encoding utf8
    Write-Output $zip
} finally { Pop-Location }
