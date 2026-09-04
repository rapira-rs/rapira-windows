#Requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $PhpVersion,
    [Parameter(Mandatory)] [string] $DevelUrl,
    [Parameter(Mandatory)] [string] $DevelSha256,
    [Parameter(Mandatory)] [string] $RuntimeUrl,
    [Parameter(Mandatory)] [string] $RuntimeSha256,
    [string] $ArchiveDirectory,
    [string] $InstallDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$extensions = @('openssl', 'curl', 'mbstring', 'pdo_sqlite', 'sqlite3', 'fileinfo')
$requiredExtensions = $extensions -join ','
$libclangDirectory = 'C:\Program Files\LLVM\bin'
$libclang = Join-Path $libclangDirectory 'libclang.dll'
$clang = Join-Path $libclangDirectory 'clang.exe'

if ($PhpVersion -notmatch '^8\.(4|5)\.\d+$') {
    throw "Unsupported PHP version '$PhpVersion'; expected an exact 8.4.x or 8.5.x version."
}

function Assert-Sha256 {
    param(
        [Parameter(Mandatory)] [string] $Value,
        [Parameter(Mandatory)] [string] $Name
    )

    if ($Value -notmatch '^[0-9a-fA-F]{64}$') {
        throw "$Name is not a SHA256 digest."
    }
}

function Assert-AssetUri {
    param(
        [Parameter(Mandatory)] [string] $Value,
        [Parameter(Mandatory)] [string] $ExpectedFilePattern,
        [Parameter(Mandatory)] [string] $Name
    )

    $uri = [Uri] $Value
    if (-not $uri.IsAbsoluteUri -or $uri.Scheme -ne 'https' -or $uri.Host -ne 'downloads.php.net') {
        throw "$Name must be an HTTPS URL on downloads.php.net."
    }
    $fileName = [Uri]::UnescapeDataString([IO.Path]::GetFileName($uri.AbsolutePath))
    if ($fileName -notmatch $ExpectedFilePattern) {
        throw "$Name has unexpected archive name '$fileName'."
    }
}

Assert-Sha256 -Value $DevelSha256 -Name 'DevelSha256'
Assert-Sha256 -Value $RuntimeSha256 -Name 'RuntimeSha256'
$escapedVersion = [regex]::Escape($PhpVersion)
Assert-AssetUri -Value $DevelUrl -ExpectedFilePattern "^php-devel-pack-$escapedVersion-Win32-vs17-x64\.zip$" -Name 'DevelUrl'
Assert-AssetUri -Value $RuntimeUrl -ExpectedFilePattern "^php-$escapedVersion-Win32-vs17-x64\.zip$" -Name 'RuntimeUrl'

$tempRoot = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    [IO.Path]::GetTempPath()
}
else {
    $env:RUNNER_TEMP
}
if ([string]::IsNullOrWhiteSpace($ArchiveDirectory)) {
    $ArchiveDirectory = Join-Path (Join-Path $tempRoot 'php-archives') $PhpVersion
}
if ([string]::IsNullOrWhiteSpace($InstallDirectory)) {
    $InstallDirectory = Join-Path (Join-Path $tempRoot 'php') $PhpVersion
}

$ArchiveDirectory = [IO.Path]::GetFullPath($ArchiveDirectory)
$InstallDirectory = [IO.Path]::GetFullPath($InstallDirectory)
$installRoot = [IO.Path]::GetPathRoot($InstallDirectory).TrimEnd('\', '/')
if ($InstallDirectory.TrimEnd('\', '/') -eq $installRoot) {
    throw 'InstallDirectory must not be a filesystem root.'
}
$managedInstallRoot = [IO.Path]::GetFullPath((Join-Path $tempRoot 'php')).TrimEnd('\', '/')
$managedInstallPrefix = $managedInstallRoot + [IO.Path]::DirectorySeparatorChar
if ($InstallDirectory.TrimEnd('\', '/').Equals($managedInstallRoot, [StringComparison]::OrdinalIgnoreCase) -or
    -not $InstallDirectory.StartsWith($managedInstallPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "InstallDirectory must be a child of the script-owned PHP directory '$managedInstallRoot'."
}
$archiveWithSeparator = $ArchiveDirectory.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
$installWithSeparator = $InstallDirectory.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
if ($ArchiveDirectory.Equals($InstallDirectory, [StringComparison]::OrdinalIgnoreCase) -or
    $archiveWithSeparator.StartsWith($installWithSeparator, [StringComparison]::OrdinalIgnoreCase) -or
    $installWithSeparator.StartsWith($archiveWithSeparator, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'ArchiveDirectory and InstallDirectory must not overlap.'
}

function Test-ArchiveHash {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $ExpectedSha256
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $false
    }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    return $actual.Equals($ExpectedSha256, [StringComparison]::OrdinalIgnoreCase)
}

function Assert-ArchiveHash {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $ExpectedSha256
    )

    if (-not (Test-ArchiveHash -Path $Path -ExpectedSha256 $ExpectedSha256)) {
        $actual = if (Test-Path -LiteralPath $Path -PathType Leaf) {
            (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        else {
            '<missing>'
        }
        throw "SHA256 mismatch for '$Path': expected $($ExpectedSha256.ToLowerInvariant()), got $actual."
    }
}

function Get-VerifiedArchive {
    param(
        [Parameter(Mandatory)] [string] $Uri,
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $ExpectedSha256
    )

    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        if (Test-ArchiveHash -Path $Path -ExpectedSha256 $ExpectedSha256) {
            Write-Host "Using verified cached archive '$Path'."
            return
        }
        Write-Warning "Discarding cached archive with a mismatched SHA256: '$Path'."
        Remove-Item -LiteralPath $Path -Force
    }

    $downloadPath = "$Path.download-$([Guid]::NewGuid().ToString('N'))"
    try {
        Invoke-WebRequest -Uri $Uri -OutFile $downloadPath -MaximumRetryCount 3 -RetryIntervalSec 2 | Out-Null
        Assert-ArchiveHash -Path $downloadPath -ExpectedSha256 $ExpectedSha256
        Move-Item -LiteralPath $downloadPath -Destination $Path
    }
    finally {
        if (Test-Path -LiteralPath $downloadPath) {
            Remove-Item -LiteralPath $downloadPath -Force
        }
    }

    Assert-ArchiveHash -Path $Path -ExpectedSha256 $ExpectedSha256
}

function Invoke-Php {
    param(
        [Parameter(Mandatory)] [string] $PhpExe,
        [Parameter(Mandatory)] [string] $WorkingDirectory,
        [Parameter(Mandatory)] [string[]] $Arguments
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $PhpExe
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw "Could not start '$PhpExe'."
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        return [pscustomobject] @{
            ExitCode = $process.ExitCode
            Stdout = $stdout
            Stderr = $stderr
        }
    }
    finally {
        $process.Dispose()
    }
}

function Write-GitHubEnvironment {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [string] $Value
    )

    if ([string]::IsNullOrWhiteSpace($env:GITHUB_ENV)) {
        return
    }
    $line = "$Name=$Value$([Environment]::NewLine)"
    [IO.File]::AppendAllText($env:GITHUB_ENV, $line, [Text.UTF8Encoding]::new($false))
}

New-Item -ItemType Directory -Path $ArchiveDirectory -Force | Out-Null
$develArchive = Join-Path $ArchiveDirectory 'devel.zip'
$runtimeArchive = Join-Path $ArchiveDirectory 'runtime.zip'
Get-VerifiedArchive -Uri $DevelUrl -Path $develArchive -ExpectedSha256 $DevelSha256
Get-VerifiedArchive -Uri $RuntimeUrl -Path $runtimeArchive -ExpectedSha256 $RuntimeSha256

# Verify both files immediately before use, including files from the cache.
Assert-ArchiveHash -Path $develArchive -ExpectedSha256 $DevelSha256
Assert-ArchiveHash -Path $runtimeArchive -ExpectedSha256 $RuntimeSha256

if (Test-Path -LiteralPath $InstallDirectory) {
    $installItem = Get-Item -LiteralPath $InstallDirectory -Force
    if (($installItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "InstallDirectory must not be a reparse point: '$InstallDirectory'."
    }
    Remove-Item -LiteralPath $InstallDirectory -Recurse -Force
}
$develExtract = Join-Path $InstallDirectory 'devel'
$runtimeRoot = Join-Path $InstallDirectory 'runtime'
New-Item -ItemType Directory -Path $develExtract, $runtimeRoot -Force | Out-Null
Expand-Archive -LiteralPath $develArchive -DestinationPath $develExtract
Expand-Archive -LiteralPath $runtimeArchive -DestinationPath $runtimeRoot

$importLibraries = @(Get-ChildItem -LiteralPath $develExtract -Recurse -File -Filter 'php8ts.lib' |
    Where-Object { $_.Directory.Name -eq 'lib' })
if ($importLibraries.Count -ne 1) {
    throw "Expected exactly one devel-pack lib\php8ts.lib, found $($importLibraries.Count)."
}
$develRoot = $importLibraries[0].Directory.Parent.FullName
$phpHeader = Join-Path $develRoot 'include\main\php.h'
$versionHeader = Join-Path $develRoot 'include\main\php_version.h'
$configHeader = Join-Path $develRoot 'include\main\config.w32.h'
foreach ($requiredFile in @($phpHeader, $versionHeader, $configHeader)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "The devel pack is missing '$requiredFile'."
    }
}
$versionHeaderText = Get-Content -LiteralPath $versionHeader -Raw
if ($versionHeaderText -notmatch ('(?m)^#define PHP_VERSION "{0}"$' -f $escapedVersion)) {
    throw "The devel pack headers do not report PHP $PhpVersion."
}
$configHeaderText = Get-Content -LiteralPath $configHeader -Raw
if ($configHeaderText -notmatch '(?m)^#define PHP_COMPILER_ID "VS17"\r?$') {
    throw 'The devel pack was not built with VS17.'
}

$phpExe = Join-Path $runtimeRoot 'php.exe'
$phpDll = Join-Path $runtimeRoot 'php8ts.dll'
foreach ($requiredFile in @($phpExe, $phpDll)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "The PHP runtime is missing '$requiredFile'."
    }
}

$identity = Invoke-Php -PhpExe $phpExe -WorkingDirectory $runtimeRoot -Arguments @(
    '-n',
    '-r',
    'echo PHP_VERSION, "|", PHP_ZTS ? "1" : "0", "|", PHP_INT_SIZE, "|", PHP_OS_FAMILY;'
)
if ($identity.ExitCode -ne 0) {
    throw "php.exe identity check failed with exit code $($identity.ExitCode): $($identity.Stderr.Trim())"
}
$expectedIdentity = "$PhpVersion|1|8|Windows"
if ($identity.Stdout.Trim() -cne $expectedIdentity) {
    throw "Unexpected PHP runtime identity '$($identity.Stdout.Trim())'; expected '$expectedIdentity'."
}

foreach ($extension in $extensions) {
    $extensionDll = Join-Path $runtimeRoot "ext\php_$extension.dll"
    if (-not (Test-Path -LiteralPath $extensionDll -PathType Leaf)) {
        throw "The PHP runtime is missing bundled extension '$extensionDll'."
    }
    $modulesResult = Invoke-Php -PhpExe $phpExe -WorkingDirectory $runtimeRoot -Arguments @(
        '-n',
        '-d',
        'extension_dir=ext',
        '-d',
        "extension=$extension",
        '-m'
    )
    if ($modulesResult.ExitCode -ne 0) {
        throw "PHP extension '$extension' load check failed with exit code $($modulesResult.ExitCode): $($modulesResult.Stderr.Trim())"
    }
    $modules = @($modulesResult.Stdout -split '\r?\n' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    if (-not ($modules -contains $extension)) {
        throw "Bundled extension '$extension' did not load.`n$($modulesResult.Stdout)`n$($modulesResult.Stderr)"
    }
    foreach ($coverageDriver in @('xdebug', 'pcov')) {
        if ($modules -contains $coverageDriver) {
            throw "PHP coverage driver '$coverageDriver' is enabled; it is incompatible with the embed tests."
        }
    }
}

foreach ($llvmFile in @($libclang, $clang)) {
    if (-not (Test-Path -LiteralPath $llvmFile -PathType Leaf)) {
        throw "The LLVM toolchain file '$llvmFile' was not found."
    }
}

$env:PHP_FULL = $PhpVersion
$env:PHP_DEVEL_DIR = $develRoot
$env:LIBCLANG_PATH = $libclangDirectory
$env:CLANG_PATH = $clang
$env:PHP_RUNTIME = $runtimeRoot
$env:RAPIRA_REQUIRE_EXTS = $requiredExtensions
$env:PATH = "$runtimeRoot;$env:PATH"

Write-GitHubEnvironment -Name 'PHP_FULL' -Value $PhpVersion
Write-GitHubEnvironment -Name 'PHP_DEVEL_DIR' -Value $develRoot
Write-GitHubEnvironment -Name 'LIBCLANG_PATH' -Value $libclangDirectory
Write-GitHubEnvironment -Name 'CLANG_PATH' -Value $clang
Write-GitHubEnvironment -Name 'PHP_RUNTIME' -Value $runtimeRoot
Write-GitHubEnvironment -Name 'RAPIRA_REQUIRE_EXTS' -Value $requiredExtensions
if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_PATH)) {
    [IO.File]::AppendAllText(
        $env:GITHUB_PATH,
        "$runtimeRoot$([Environment]::NewLine)",
        [Text.UTF8Encoding]::new($false)
    )
}

Write-Host "Provisioned PHP $PhpVersion ZTS x64 with VS17 headers at '$InstallDirectory'."
