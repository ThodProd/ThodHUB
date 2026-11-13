param([string]$OutputDirectory = 'artifacts/release', [string]$Tag = '')
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$repo = Split-Path $root -Parent
$llvm = Join-Path $repo '.tools\llvm\llvm-mingw-20260616-ucrt-x86_64\bin'
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$env:PATH = "$llvm;$cargoBin;$env:PATH"

Push-Location $root
try {
    cargo test
    if ($LASTEXITCODE) { throw 'Tests failed' }
    cargo build --release
    if ($LASTEXITCODE) { throw 'Build failed' }

    $exe = Join-Path $root 'target\release\ThodHUB.exe'
    if (-not (Test-Path -LiteralPath $exe)) { throw "Missing $exe" }
    $loader = Get-ChildItem (Join-Path $root 'target\release\build\webview2-com-sys') -Recurse -Filter 'WebView2Loader.dll' |
        Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -First 1
    if (-not $loader) { throw 'WebView2Loader.dll (x64) was not produced by the build.' }

    $stage = Join-Path $root ('artifacts\hub-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'ThodHUB.exe')
    Copy-Item -LiteralPath $loader.FullName -Destination (Join-Path $stage 'WebView2Loader.dll')
    Copy-Item -LiteralPath (Join-Path $root 'wwwroot') -Destination (Join-Path $stage 'wwwroot') -Recurse
    Copy-Item -LiteralPath (Join-Path $root 'catalog.json') -Destination (Join-Path $stage 'catalog.json')
    Copy-Item -LiteralPath (Join-Path $root 'agent-prompt.md') -Destination (Join-Path $stage 'agent-prompt.md')

    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
    $versionLine = Select-String -LiteralPath (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $versionLine) { throw 'Cannot read version from Cargo.toml' }
    $version = $versionLine.Matches[0].Groups[1].Value
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
        notes = (Get-Content -LiteralPath (Join-Path $root 'release-notes.md') -Raw -Encoding UTF8).Trim()
        platform = 'win-x64'
        hubApi = 1
    }
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'thodhub-release.json'), ($descriptor | ConvertTo-Json -Compress), [Text.UTF8Encoding]::new($false))
    Write-Output $zip
} finally { Pop-Location }
