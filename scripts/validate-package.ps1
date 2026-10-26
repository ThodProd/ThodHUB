param([Parameter(Mandatory=$true)][string]$Package,[Parameter(Mandatory=$true)][string]$Release,[Parameter(Mandatory=$true)][string]$Repository)
$ErrorActionPreference='Stop'
$packagePath=(Resolve-Path -LiteralPath $Package).Path
$releasePath=(Resolve-Path -LiteralPath $Release).Path
$project=Join-Path (Split-Path $PSScriptRoot -Parent) 'tests/ThodHub.Tests/ThodHub.Tests.csproj'
dotnet run --project $project -- --validate-package $packagePath $releasePath $Repository
if($LASTEXITCODE){throw 'ThodHUB package validation failed'}
