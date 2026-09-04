#Requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('8.4', '8.5')]
    [string] $Branch
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$releasesUri = [Uri] 'https://downloads.php.net/~windows/releases/releases.json'
$archiveBaseUri = [Uri] 'https://downloads.php.net/~windows/releases/'
$sourceReleaseBaseUri = [Uri] 'https://www.php.net/releases/index.php'
$sourceArchiveBaseUri = [Uri] 'https://www.php.net/distributions/'
$variantName = 'ts-vs17-x64'

function Get-RequiredProperty {
    param(
        [Parameter(Mandatory)] [object] $Object,
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [string] $Context
    )

    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property -or $null -eq $property.Value) {
        throw "Missing '$Name' in $Context."
    }
    return $property.Value
}

function Get-Asset {
    param(
        [Parameter(Mandatory)] [object] $Variant,
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [string] $ExpectedFilePattern
    )

    $asset = Get-RequiredProperty -Object $Variant -Name $Name -Context "PHP $Branch $variantName"
    $path = [string] (Get-RequiredProperty -Object $asset -Name 'path' -Context "$Name asset")
    $sha256 = [string] (Get-RequiredProperty -Object $asset -Name 'sha256' -Context "$Name asset")

    if ($path -notmatch $ExpectedFilePattern -or $path -match '[/\\]' -or $path.Contains('..')) {
        throw "Unexpected $Name archive path '$path'."
    }
    if ($sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "Invalid SHA256 for '$path'."
    }

    return [pscustomobject] @{
        url = ([Uri]::new($archiveBaseUri, $path)).AbsoluteUri
        sha256 = $sha256.ToLowerInvariant()
    }
}

function Get-SourceAsset {
    param(
        [Parameter(Mandatory)] [object] $Release,
        [Parameter(Mandatory)] [string] $Version
    )

    $reportedVersion = [string] (Get-RequiredProperty -Object $Release -Name 'version' -Context "PHP $Version source metadata")
    if ($reportedVersion -cne $Version) {
        throw "The source metadata returned version '$reportedVersion'; expected '$Version'."
    }

    $expectedFile = "php-$Version.tar.xz"
    $sources = @(Get-RequiredProperty -Object $Release -Name 'source' -Context "PHP $Version source metadata")
    $matches = @($sources | Where-Object {
        $filenameProperty = $_.PSObject.Properties['filename']
        $null -ne $filenameProperty -and [string] $filenameProperty.Value -ceq $expectedFile
    })
    if ($matches.Count -ne 1) {
        throw "Expected exactly one '$expectedFile' source archive, found $($matches.Count)."
    }

    $filename = [string] (Get-RequiredProperty -Object $matches[0] -Name 'filename' -Context "PHP $Version source archive")
    $sha256 = [string] (Get-RequiredProperty -Object $matches[0] -Name 'sha256' -Context "PHP $Version source archive")
    $escapedVersion = [regex]::Escape($Version)
    if ($filename -notmatch "^php-$escapedVersion\.tar\.xz$" -or $filename -match '[/\\]' -or $filename.Contains('..')) {
        throw "Unexpected PHP source archive path '$filename'."
    }
    if ($sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "Invalid SHA256 for '$filename'."
    }

    return [pscustomobject] @{
        url = ([Uri]::new($sourceArchiveBaseUri, $filename)).AbsoluteUri
        sha256 = $sha256.ToLowerInvariant()
    }
}

function Write-GitHubOutput {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [string] $Value
    )

    if ([string]::IsNullOrWhiteSpace($env:GITHUB_OUTPUT)) {
        return
    }
    $line = "$Name=$Value$([Environment]::NewLine)"
    [IO.File]::AppendAllText($env:GITHUB_OUTPUT, $line, [Text.UTF8Encoding]::new($false))
}

try {
    $releases = Invoke-RestMethod -Uri $releasesUri -MaximumRetryCount 3 -RetryIntervalSec 2
}
catch {
    throw "Could not read the official Windows PHP release index at $releasesUri`: $($_.Exception.Message)"
}

$branchEntry = Get-RequiredProperty -Object $releases -Name $Branch -Context 'the Windows PHP release index'
$version = [string] (Get-RequiredProperty -Object $branchEntry -Name 'version' -Context "PHP $Branch")
$escapedBranch = [regex]::Escape($Branch)
if ($version -notmatch "^$escapedBranch\.\d+$") {
    throw "The release index returned unexpected version '$version' for PHP $Branch."
}

$variant = Get-RequiredProperty -Object $branchEntry -Name $variantName -Context "PHP $version"
$escapedVersion = [regex]::Escape($version)
$devel = Get-Asset -Variant $variant -Name 'devel_pack' -ExpectedFilePattern "^php-devel-pack-$escapedVersion-Win32-vs17-x64\.zip$"
$runtime = Get-Asset -Variant $variant -Name 'zip' -ExpectedFilePattern "^php-$escapedVersion-Win32-vs17-x64\.zip$"

$sourceReleaseUri = [Uri] "$($sourceReleaseBaseUri.AbsoluteUri)?json&version=$version"
try {
    $sourceRelease = Invoke-RestMethod -Uri $sourceReleaseUri -MaximumRetryCount 3 -RetryIntervalSec 2
}
catch {
    throw "Could not read the official PHP source metadata at $sourceReleaseUri`: $($_.Exception.Message)"
}
$source = Get-SourceAsset -Release $sourceRelease -Version $version

$result = [ordered] @{
    php_branch = $Branch
    php_version = $version
    php_variant = $variantName
    devel_url = $devel.url
    devel_sha256 = $devel.sha256
    runtime_url = $runtime.url
    runtime_sha256 = $runtime.sha256
    cache_key = "php-zts-vs17-x64-$version"
    source_url = $source.url
    source_sha256 = $source.sha256
    source_cache_key = "php-source-$version-$($source.sha256)"
}

foreach ($entry in $result.GetEnumerator()) {
    Write-GitHubOutput -Name $entry.Key -Value ([string] $entry.Value)
}

[pscustomobject] $result | ConvertTo-Json -Compress
