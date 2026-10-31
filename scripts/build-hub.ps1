param([string]$OutputDirectory = 'artifacts/release', [string]$Tag = '')
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
    $releaseTag = if ($Tag) { $Tag } else { "hub-v$version" }
    $zip = Join-Path ([IO.Path]::GetFullPath($OutputDirectory)) "ThodHUB-$version-win-x64.zip"
    if (Test-Path -LiteralPath $zip) { throw "Already exists: $zip" }
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::Open($zip, [IO.Compression.ZipArchiveMode]::Create)
    try {
        $base = [IO.Path]::GetFullPath($stage).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        foreach ($file in [IO.Directory]::EnumerateFiles($stage, '*', [IO.SearchOption]::AllDirectories)) {
            $relative = $file.Substring($base.Length).Replace([IO.Path]::DirectorySeparatorChar, '/')
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file, $relative, [IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
    } finally { $archive.Dispose() }
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'SHA256SUMS.txt'), "$hash  $([IO.Path]::GetFileName($zip))`n", [Text.UTF8Encoding]::new($false))
    $descriptor = [ordered]@{
        version = $version
        packageUrl = "https://github.com/ThodProd/ThodHUB/releases/download/$releaseTag/ThodHUB-$version-win-x64.zip"
        sha256 = $hash
        size = (Get-Item -LiteralPath $zip).Length
        notes = 'Обновления программ и самого хаба, контекстное меню программ и упорядоченное управление в карточках.'
        platform = 'win-x64'
        hubApi = 1
    }
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'thodhub-release.json'), ($descriptor | ConvertTo-Json -Compress), [Text.UTF8Encoding]::new($false))
    Write-Output $zip
} finally { Pop-Location }
