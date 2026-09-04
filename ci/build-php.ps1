[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $PhpVersion,
    [Parameter(Mandatory)] [string] $SourceUrl,
    [Parameter(Mandatory)] [string] $SourceSha256,
    [Parameter(Mandatory)] [string] $InstallDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$requiredModules = @(
    'calendar',
    'ctype',
    'exif',
    'fileinfo',
    'filter',
    'ftp',
    'mbstring',
    'session',
    'sockets',
    'tokenizer',
    'Zend OPcache'
)
$requiredExtensionModules = @('fileinfo', 'mbstring')
$requiredExtensions = $requiredExtensionModules -join ','

if ($PhpVersion -notmatch '^8\.(4|5)\.\d+$') {
    throw "Unsupported PHP version '$PhpVersion'; expected an exact 8.4.x or 8.5.x version."
}
if ($SourceSha256 -notmatch '^[0-9a-fA-F]{64}$') {
    throw 'SourceSha256 is not a SHA256 digest.'
}
$SourceSha256 = $SourceSha256.ToLowerInvariant()

$escapedVersion = [regex]::Escape($PhpVersion)
$sourceUri = [Uri] $SourceUrl
$sourceFileName = [Uri]::UnescapeDataString([IO.Path]::GetFileName($sourceUri.AbsolutePath))
if (-not $sourceUri.IsAbsoluteUri -or
    $sourceUri.Scheme -ne 'https' -or
    $sourceUri.Host -ne 'www.php.net' -or
    $sourceFileName -notmatch "^php-$escapedVersion\.tar\.xz$") {
    throw "SourceUrl must name php-$PhpVersion.tar.xz on https://www.php.net."
}

function Get-Architecture {
    $osArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    $processArchitecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
    if ($processArchitecture -ne $osArchitecture) {
        throw "Run ci/build-php.ps1 from native $osArchitecture PowerShell; the current process is $processArchitecture."
    }

    $architecture = $osArchitecture.ToString()
    switch ($architecture) {
        'Arm64' {
            return [pscustomobject] @{
                Name = 'arm64'
                PeMachine = [uint16] 0xaa64
                PhpMachine = 'ARM64'
                VcVars = 'arm64'
            }
        }
        'X64' {
            return [pscustomobject] @{
                Name = 'x86_64'
                PeMachine = [uint16] 0x8664
                PhpMachine = 'AMD64'
                VcVars = 'x64'
            }
        }
        default {
            throw "Unsupported native Windows architecture '$architecture'."
        }
    }
}

function Get-PeMachine {
    param([Parameter(Mandatory)] [string] $Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "PE file '$Path' does not exist."
    }
    $stream = [IO.File]::OpenRead($Path)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5a4d) {
            throw "'$Path' has no DOS executable header."
        }
        $stream.Position = 0x3c
        $peOffset = $reader.ReadUInt32()
        if ($peOffset -gt $stream.Length - 6) {
            throw "'$Path' has an invalid PE header offset."
        }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "'$Path' has no PE signature."
        }
        return $reader.ReadUInt16()
    }
    finally {
        $stream.Dispose()
    }
}

function Assert-PeMachine {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [uint16] $Expected
    )

    $actual = Get-PeMachine -Path $Path
    if ($actual -ne $Expected) {
        throw "'$Path' has PE machine 0x$($actual.ToString('X4')); expected native machine 0x$($Expected.ToString('X4'))."
    }
}

function Get-NativeSystemExecutable {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [uint16] $ExpectedMachine
    )

    $systemDirectory = Join-Path $env:SystemRoot 'System32'
    $path = Join-Path $systemDirectory $Name
    Assert-PeMachine -Path $path -Expected $ExpectedMachine
    return $path
}

function Assert-ManagedPath {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $Root,
        [Parameter(Mandatory)] [string] $Name
    )

    $fullPath = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $fullRoot = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $rootPrefix = $fullRoot + [IO.Path]::DirectorySeparatorChar
    if ($fullPath.Equals($fullRoot, [StringComparison]::OrdinalIgnoreCase) -or
        -not $fullPath.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "$Name must be a child of '$fullRoot'."
    }
    return $fullPath
}

function Remove-ManagedDirectory {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $Root
    )

    $verifiedPath = Assert-ManagedPath -Path $Path -Root $Root -Name 'Directory'
    if (-not (Test-Path -LiteralPath $verifiedPath)) {
        return
    }
    $item = Get-Item -LiteralPath $verifiedPath -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to remove reparse point '$verifiedPath'."
    }
    Remove-Item -LiteralPath $verifiedPath -Recurse -Force
}

function Test-FileHash {
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

function Get-VerifiedSource {
    param(
        [Parameter(Mandatory)] [Uri] $Uri,
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $ExpectedSha256
    )

    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        if (Test-FileHash -Path $Path -ExpectedSha256 $ExpectedSha256) {
            Write-Host "Using verified PHP source archive '$Path'."
            return
        }
        Remove-Item -LiteralPath $Path -Force
    }

    $downloadPath = "$Path.download-$([Guid]::NewGuid().ToString('N'))"
    try {
        Invoke-WebRequest -Uri $Uri -OutFile $downloadPath -MaximumRetryCount 3 -RetryIntervalSec 2 | Out-Null
        if (-not (Test-FileHash -Path $downloadPath -ExpectedSha256 $ExpectedSha256)) {
            $actual = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash.ToLowerInvariant()
            throw "PHP source SHA256 mismatch: expected $ExpectedSha256, got $actual."
        }
        Move-Item -LiteralPath $downloadPath -Destination $Path
    }
    finally {
        if (Test-Path -LiteralPath $downloadPath) {
            Remove-Item -LiteralPath $downloadPath -Force
        }
    }
}

function Replace-RequiredText {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $Before,
        [Parameter(Mandatory)] [string] $After,
        [Parameter(Mandatory)] [string] $Description
    )

    $text = [IO.File]::ReadAllText($Path)
    $matches = [regex]::Matches($text, [regex]::Escape($Before)).Count
    if ($matches -ne 1) {
        throw "Expected exactly one $Description site in '$Path', found $matches."
    }
    [IO.File]::WriteAllText($Path, $text.Replace($Before, $After), [Text.UTF8Encoding]::new($false))
}

function Find-VisualStudio {
    $visualStudioRoot = Join-Path $env:SystemDrive 'Program Files\Microsoft Visual Studio'
    if (-not (Test-Path -LiteralPath $visualStudioRoot -PathType Container)) {
        throw "Visual Studio was not found below '$visualStudioRoot'."
    }

    $candidates = foreach ($versionDirectory in Get-ChildItem -LiteralPath $visualStudioRoot -Directory) {
        foreach ($editionDirectory in Get-ChildItem -LiteralPath $versionDirectory.FullName -Directory) {
            $vcVars = Join-Path $editionDirectory.FullName 'VC\Auxiliary\Build\vcvarsall.bat'
            if (Test-Path -LiteralPath $vcVars -PathType Leaf) {
                $rank = if ($versionDirectory.Name -match '^\d{4}$') {
                    switch ($versionDirectory.Name) {
                        '2022' { 17 }
                        '2019' { 16 }
                        default { [int] $versionDirectory.Name }
                    }
                }
                elseif ($versionDirectory.Name -match '^\d+$') {
                    [int] $versionDirectory.Name
                }
                else {
                    0
                }
                [pscustomobject] @{ Path = $vcVars; Rank = $rank }
            }
        }
    }
    $selected = $candidates | Sort-Object Rank -Descending | Select-Object -First 1
    if ($null -eq $selected) {
        throw "Visual Studio vcvarsall.bat was not found below '$visualStudioRoot'."
    }
    return $selected.Path
}

function Invoke-Batch {
    param(
        [Parameter(Mandatory)] [string] $NativeCmd,
        [Parameter(Mandatory)] [string] $WorkingDirectory,
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [string[]] $Lines
    )

    $batchPath = Join-Path $WorkingDirectory ".rapira-$Name-$([Guid]::NewGuid().ToString('N')).cmd"
    try {
        [IO.File]::WriteAllLines($batchPath, @('@echo off', 'setlocal') + $Lines, [Text.ASCIIEncoding]::new())
        & $NativeCmd /d /c $batchPath
        if ($LASTEXITCODE -ne 0) {
            throw "$Name failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        if (Test-Path -LiteralPath $batchPath) {
            Remove-Item -LiteralPath $batchPath -Force
        }
    }
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
        return [pscustomobject] @{
            ExitCode = $process.ExitCode
            Stdout = $stdoutTask.GetAwaiter().GetResult()
            Stderr = $stderrTask.GetAwaiter().GetResult()
        }
    }
    finally {
        $process.Dispose()
    }
}

function Assert-PhpInstall {
    param(
        [Parameter(Mandatory)] [string] $Root,
        [Parameter(Mandatory)] [object] $Architecture
    )

    $markerPath = Join-Path $Root '.rapira-php.json'
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        throw "PHP install marker '$markerPath' is missing."
    }
    $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    if ([string] $marker.php_version -cne $PhpVersion -or
        [string] $marker.source_sha256 -cne $SourceSha256 -or
        [string] $marker.architecture -cne $Architecture.Name) {
        throw "PHP install marker '$markerPath' does not match this build."
    }

    $runtime = Join-Path $Root 'runtime'
    $devel = Join-Path $Root 'devel'
    $phpExe = Join-Path $runtime 'php.exe'
    $phpDll = Join-Path $runtime 'php8ts.dll'
    $phpLib = Join-Path $devel 'lib\php8ts.lib'
    $phpHeader = Join-Path $devel 'include\main\php.h'
    $versionHeader = Join-Path $devel 'include\main\php_version.h'
    $configHeader = Join-Path $devel 'include\main\config.w32.h'
    $phpLicense = Join-Path $Root 'PHP-LICENSE.txt'
    foreach ($requiredFile in @($phpExe, $phpDll, $phpLib, $phpHeader, $versionHeader, $configHeader, $phpLicense)) {
        if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
            throw "The PHP install is missing '$requiredFile'."
        }
    }

    Assert-PeMachine -Path $phpExe -Expected $Architecture.PeMachine
    Assert-PeMachine -Path $phpDll -Expected $Architecture.PeMachine
    foreach ($dll in Get-ChildItem -LiteralPath $runtime -Recurse -File -Filter '*.dll') {
        Assert-PeMachine -Path $dll.FullName -Expected $Architecture.PeMachine
    }

    $versionHeaderText = Get-Content -LiteralPath $versionHeader -Raw
    if ($versionHeaderText -notmatch ('(?m)^#define PHP_VERSION "{0}"\r?$' -f $escapedVersion)) {
        throw "The PHP headers do not report PHP $PhpVersion."
    }
    $configHeaderText = Get-Content -LiteralPath $configHeader -Raw
    $phpBuildArchitecture = $Architecture.Name.Replace('x86_64', 'x64')
    if ($configHeaderText -notmatch ('(?m)^#define PHP_BUILD_ARCH "{0}"\r?$' -f $phpBuildArchitecture)) {
        throw "The PHP headers do not describe a native $($Architecture.Name) build."
    }

    $identity = Invoke-Php -PhpExe $phpExe -WorkingDirectory $runtime -Arguments @(
        '-n',
        '-r',
        'echo PHP_VERSION, "|", PHP_ZTS ? "1" : "0", "|", PHP_INT_SIZE, "|", PHP_OS_FAMILY, "|", php_uname("m");'
    )
    if ($identity.ExitCode -ne 0) {
        throw "php.exe identity check failed with exit code $($identity.ExitCode): $($identity.Stderr.Trim())"
    }
    $expectedIdentity = "$PhpVersion|1|8|Windows|$($Architecture.PhpMachine)"
    if ($identity.Stdout.Trim() -cne $expectedIdentity) {
        throw "Unexpected PHP runtime identity '$($identity.Stdout.Trim())'; expected '$expectedIdentity'."
    }

    $moduleArguments = [Collections.Generic.List[string]]::new()
    $moduleArguments.Add('-n')
    $moduleArguments.Add('-d')
    $moduleArguments.Add("extension_dir=$(Join-Path $runtime 'ext')")
    foreach ($extension in $requiredExtensionModules) {
        $dll = Join-Path $runtime "ext\php_$extension.dll"
        if (Test-Path -LiteralPath $dll -PathType Leaf) {
            $moduleArguments.Add('-d')
            $moduleArguments.Add("extension=php_$extension.dll")
        }
    }
    $opcacheDll = Join-Path $runtime 'ext\php_opcache.dll'
    if (Test-Path -LiteralPath $opcacheDll -PathType Leaf) {
        $moduleArguments.Add('-d')
        $moduleArguments.Add('zend_extension=php_opcache.dll')
    }
    $moduleArguments.Add('-m')
    $modulesResult = Invoke-Php -PhpExe $phpExe -WorkingDirectory $runtime -Arguments $moduleArguments.ToArray()
    if ($modulesResult.ExitCode -ne 0) {
        throw "PHP module check failed with exit code $($modulesResult.ExitCode): $($modulesResult.Stderr.Trim())"
    }
    $modules = @($modulesResult.Stdout -split '\r?\n' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    foreach ($module in $requiredModules) {
        if (-not ($modules -ccontains $module)) {
            throw "The native PHP build is missing required module '$module'."
        }
    }
    foreach ($coverageDriver in @('xdebug', 'pcov')) {
        if ($modules -contains $coverageDriver) {
            throw "PHP coverage driver '$coverageDriver' is enabled; it is incompatible with the embed tests."
        }
    }

    return [pscustomobject] @{
        Runtime = $runtime
        Devel = $devel
    }
}

function Find-NativeLlvm {
    param([Parameter(Mandatory)] [object] $Architecture)

    $clangCommand = Get-Command clang.exe -ErrorAction SilentlyContinue
    $clangCommandDirectory = if ($null -eq $clangCommand) { $null } else { Split-Path -Parent ([string] $clangCommand.Source) }
    $candidates = @(
        (Join-Path $env:SystemDrive 'Program Files\LLVM\bin'),
        $clangCommandDirectory
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique
    foreach ($directory in $candidates) {
        $clang = Join-Path $directory 'clang.exe'
        $libclang = Join-Path $directory 'libclang.dll'
        if ((Test-Path -LiteralPath $clang -PathType Leaf) -and
            (Test-Path -LiteralPath $libclang -PathType Leaf)) {
            try {
                Assert-PeMachine -Path $clang -Expected $Architecture.PeMachine
                Assert-PeMachine -Path $libclang -Expected $Architecture.PeMachine
                return [pscustomobject] @{ Directory = $directory; Clang = $clang }
            }
            catch {
                continue
            }
        }
    }
    throw "A native LLVM clang.exe and libclang.dll pair was not found for $($Architecture.Name)."
}

function Write-GitHubEnvironment {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [string] $Value
    )

    if ([string]::IsNullOrWhiteSpace($env:GITHUB_ENV)) {
        return
    }
    [IO.File]::AppendAllText(
        $env:GITHUB_ENV,
        "$Name=$Value$([Environment]::NewLine)",
        [Text.UTF8Encoding]::new($false)
    )
}

function Publish-PhpEnvironment {
    param(
        [Parameter(Mandatory)] [object] $Install,
        [Parameter(Mandatory)] [object] $Architecture
    )

    $llvm = Find-NativeLlvm -Architecture $Architecture
    $rustFlags = "-L native=$($Install.Devel)\lib"
    $env:PHP_FULL = $PhpVersion
    $env:PHP_DEVEL_DIR = $Install.Devel
    $env:PHP_RUNTIME = $Install.Runtime
    $env:RUSTFLAGS = $rustFlags
    $env:LIBCLANG_PATH = $llvm.Directory
    $env:CLANG_PATH = $llvm.Clang
    $env:RAPIRA_REQUIRE_EXTS = $requiredExtensions
    $env:PATH = "$($Install.Runtime);$env:PATH"

    Write-GitHubEnvironment -Name 'PHP_FULL' -Value $PhpVersion
    Write-GitHubEnvironment -Name 'PHP_DEVEL_DIR' -Value $Install.Devel
    Write-GitHubEnvironment -Name 'PHP_RUNTIME' -Value $Install.Runtime
    Write-GitHubEnvironment -Name 'RUSTFLAGS' -Value $rustFlags
    Write-GitHubEnvironment -Name 'LIBCLANG_PATH' -Value $llvm.Directory
    Write-GitHubEnvironment -Name 'CLANG_PATH' -Value $llvm.Clang
    Write-GitHubEnvironment -Name 'RAPIRA_REQUIRE_EXTS' -Value $requiredExtensions
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_PATH)) {
        [IO.File]::AppendAllText(
            $env:GITHUB_PATH,
            "$($Install.Runtime)$([Environment]::NewLine)",
            [Text.UTF8Encoding]::new($false)
        )
    }
}

$architecture = Get-Architecture
$tempRoot = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    [IO.Path]::GetTempPath()
}
else {
    $env:RUNNER_TEMP
}
$tempRoot = [IO.Path]::GetFullPath($tempRoot).TrimEnd('\', '/')
$InstallDirectory = Assert-ManagedPath -Path $InstallDirectory -Root $tempRoot -Name 'InstallDirectory'

if (Test-Path -LiteralPath $InstallDirectory) {
    try {
        $install = Assert-PhpInstall -Root $InstallDirectory -Architecture $architecture
        Publish-PhpEnvironment -Install $install -Architecture $architecture
        Write-Host "Using cached PHP $PhpVersion ZTS $($architecture.Name) at '$InstallDirectory'."
        return
    }
    catch {
        Write-Warning "Discarding invalid cached PHP install: $($_.Exception.Message)"
        Remove-ManagedDirectory -Path $InstallDirectory -Root $tempRoot
    }
}

$nativeCmd = Get-NativeSystemExecutable -Name 'cmd.exe' -ExpectedMachine $architecture.PeMachine
$nativeTar = Get-NativeSystemExecutable -Name 'tar.exe' -ExpectedMachine $architecture.PeMachine
$vcVars = Find-VisualStudio
$sourceRootBase = Join-Path $tempRoot 'rapira-php-source'
$archiveDirectory = Join-Path $sourceRootBase 'archives'
$workParent = Join-Path $sourceRootBase 'work'
New-Item -ItemType Directory -Path $archiveDirectory, $workParent -Force | Out-Null
$sourceArchive = Join-Path $archiveDirectory "php-$PhpVersion-$SourceSha256.tar.xz"
Get-VerifiedSource -Uri $sourceUri -Path $sourceArchive -ExpectedSha256 $SourceSha256

$workRoot = Join-Path $workParent "$PhpVersion-$($architecture.Name)-$([Guid]::NewGuid().ToString('N'))"
$extractRoot = Join-Path $workRoot 'extract'
$dependencyRoot = Join-Path $workRoot 'dependencies'
$stageRoot = Join-Path $workRoot 'install'
New-Item -ItemType Directory -Path $extractRoot, $dependencyRoot, $stageRoot -Force | Out-Null
$buildSucceeded = $false

try {
    Write-Host "Extracting PHP $PhpVersion source with native $($architecture.Name) tar."
    & $nativeTar -xf $sourceArchive -C $extractRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Native tar extraction failed with exit code $LASTEXITCODE."
    }
    $sourceRoot = Join-Path $extractRoot "php-$PhpVersion"
    if (-not (Test-Path -LiteralPath $sourceRoot -PathType Container)) {
        throw "PHP source archive did not contain 'php-$PhpVersion'."
    }

    $confUtils = Join-Path $sourceRoot 'win32\build\confutils.js'
    $confUtilsText = [IO.File]::ReadAllText($confUtils)
    $crlf = [string] [char] 13 + [char] 10
    $lf = [string] [char] 10
    $tab = [string] [char] 9
    $newline = if ($confUtilsText.Contains($crlf)) { $crlf } else { $lf }
    $checkFunction = "function check_binary_tools_sdk()$newline{"
    $checkFunctionWithBypass = $checkFunction + $newline + $tab + 'return;'
    Replace-RequiredText -Path $confUtils -Before $checkFunction -After $checkFunctionWithBypass -Description 'binary tools SDK bypass'

    if ($PhpVersion.StartsWith('8.4.', [StringComparison]::Ordinal)) {
        $exifPatch = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'php-src-8.4-exif.patch') -Raw
        $exifBefore = @(
            "if(PHP_EXIF != 'no')",
            '{',
            ($tab + "if(ADD_EXTENSION_DEP('exif', 'mbstring'))"),
            ($tab + '{'),
            ($tab + $tab + 'AC_DEFINE(''HAVE_EXIF'', 1, "Define to 1 if the PHP extension ''exif'' is available.");'),
            '',
            ($tab + $tab + 'EXTENSION(''exif'', ''exif.c'', null, ''/DZEND_ENABLE_STATIC_TSRMLS_CACHE=1'');'),
            ($tab + '}'),
            '}'
        ) -join $newline
        $exifAfter = @(
            "if(PHP_EXIF != 'no') {",
            ($tab + 'AC_DEFINE(''HAVE_EXIF'', 1, "Define to 1 if the PHP extension ''exif'' is available.");'),
            ($tab + 'EXTENSION(''exif'', ''exif.c'', null, ''/DZEND_ENABLE_STATIC_TSRMLS_CACHE=1'');'),
            ($tab + "ADD_EXTENSION_DEP('exif', 'mbstring', true);"),
            '}'
        ) -join $newline
        if (-not $exifPatch.Contains("-if(PHP_EXIF != 'no')") -or
            -not $exifPatch.Contains('+' + $tab + "ADD_EXTENSION_DEP('exif', 'mbstring', true);")) {
            throw 'ci/php-src-8.4-exif.patch does not contain the expected dependency change.'
        }
        Replace-RequiredText -Path (Join-Path $sourceRoot 'ext\exif\config.w32') -Before $exifBefore -After $exifAfter -Description 'PHP 8.4 optional mbstring dependency'
    }

    $env:RAPIRA_VCVARS = $vcVars
    $env:RAPIRA_VCVARS_ARCH = $architecture.VcVars
    $env:RAPIRA_PHP_SOURCE = $sourceRoot
    $env:RAPIRA_PHP_BUILD = $dependencyRoot
    $env:RAPIRA_TOOL_PROBE = Join-Path $workRoot 'tools'
    New-Item -ItemType Directory -Path $env:RAPIRA_TOOL_PROBE -Force | Out-Null

    Invoke-Batch -NativeCmd $nativeCmd -WorkingDirectory $workRoot -Name 'buildconf' -Lines @(
        'call "%RAPIRA_VCVARS%" %RAPIRA_VCVARS_ARCH%',
        'if errorlevel 1 exit /b %errorlevel%',
        'cd /d "%RAPIRA_PHP_SOURCE%"',
        'call buildconf.bat',
        'if errorlevel 1 exit /b %errorlevel%'
    )

    $configureJs = Join-Path $sourceRoot 'configure.js'
    Replace-RequiredText -Path $configureJs -Before ($tab + $tab + "ERROR('bison is required')") -After ($tab + $tab + "DEFINE('BISON', '')") -Description 'missing bison fallback'
    Replace-RequiredText -Path $configureJs -Before ($tab + $tab + "ERROR('sed is required')") -After ($tab + $tab + "DEFINE('SED', '')") -Description 'missing sed fallback'
    Replace-RequiredText -Path $configureJs -Before ($tab + $tab + "ERROR('re2c is required')") -After ($tab + $tab + "DEFINE('RE2C', '')") -Description 'missing re2c fallback'

    if ($PhpVersion.StartsWith('8.5.', [StringComparison]::Ordinal) -and $architecture.Name -eq 'arm64') {
        $simdPatch = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'php-src-8.5-arm64.patch') -Raw
        $before = '#elif defined(__aarch64__) || defined(_M_ARM64)'
        $after = '#elif (defined(__aarch64__) || defined(_M_ARM64)) && !defined(_MSC_VER)'
        if (-not $simdPatch.Contains("-$before") -or -not $simdPatch.Contains("+$after")) {
            throw 'ci/php-src-8.5-arm64.patch does not contain the expected SIMD change.'
        }
        Replace-RequiredText -Path (Join-Path $sourceRoot 'Zend\zend_simd.h') -Before $before -After $after -Description 'PHP 8.5 MSVC ARM64 SIMD fallback'
    }

    Invoke-Batch -NativeCmd $nativeCmd -WorkingDirectory $workRoot -Name 'tool-probe' -Lines @(
        'call "%RAPIRA_VCVARS%" %RAPIRA_VCVARS_ARCH%',
        'if errorlevel 1 exit /b %errorlevel%',
        'where cl.exe > "%RAPIRA_TOOL_PROBE%\cl.txt"',
        'if errorlevel 1 exit /b %errorlevel%',
        'where nmake.exe > "%RAPIRA_TOOL_PROBE%\nmake.txt"',
        'if errorlevel 1 exit /b %errorlevel%',
        'where link.exe > "%RAPIRA_TOOL_PROBE%\link.txt"',
        'if errorlevel 1 exit /b %errorlevel%',
        'where lib.exe > "%RAPIRA_TOOL_PROBE%\lib.txt"',
        'if errorlevel 1 exit /b %errorlevel%',
        'where cscript.exe > "%RAPIRA_TOOL_PROBE%\cscript.txt"',
        'if errorlevel 1 exit /b %errorlevel%'
    )
    foreach ($toolName in @('cl', 'nmake', 'link', 'lib', 'cscript')) {
        $toolPath = Get-Content -LiteralPath (Join-Path $env:RAPIRA_TOOL_PROBE "$toolName.txt") |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            Select-Object -First 1
        if ([string]::IsNullOrWhiteSpace($toolPath)) {
            throw "Visual Studio did not resolve native tool '$toolName.exe'."
        }
        Assert-PeMachine -Path $toolPath.Trim() -Expected $architecture.PeMachine
    }

    $flagFile = Join-Path $PSScriptRoot 'php-configure-flags.txt'
    $configureFlags = @(Get-Content -LiteralPath $flagFile |
        ForEach-Object { $_.Trim() } |
        Where-Object { $_ -and $_ -notmatch '^#' })
    if (@($configureFlags | Where-Object { $_ -eq '--with-php-build={PHP_BUILD}' }).Count -ne 1) {
        throw "Expected one --with-php-build={PHP_BUILD} entry in '$flagFile'."
    }
    if ($PhpVersion.StartsWith('8.5.', [StringComparison]::Ordinal)) {
        $configureFlags = @($configureFlags | Where-Object { $_ -ne '--enable-opcache' })
    }
    $quotedFlags = $configureFlags |
        ForEach-Object { '"' + $_.Replace('{PHP_BUILD}', '%RAPIRA_PHP_BUILD%') + '"' }
    $configureCommand = 'call configure.bat ' + ($quotedFlags -join ' ')
    Invoke-Batch -NativeCmd $nativeCmd -WorkingDirectory $workRoot -Name 'configure' -Lines @(
        'call "%RAPIRA_VCVARS%" %RAPIRA_VCVARS_ARCH%',
        'if errorlevel 1 exit /b %errorlevel%',
        'cd /d "%RAPIRA_PHP_SOURCE%"',
        $configureCommand,
        'if errorlevel 1 exit /b %errorlevel%'
    )

    $generatedFiles = @(
        'Zend\zend_ini_parser.c',
        'Zend\zend_ini_parser.h',
        'Zend\zend_language_parser.c',
        'Zend\zend_language_parser.h',
        'Zend\zend_ini_scanner.c',
        'Zend\zend_ini_scanner_defs.h',
        'Zend\zend_language_scanner.c',
        'Zend\zend_language_scanner_defs.h',
        'sapi\phpdbg\phpdbg_parser.c',
        'sapi\phpdbg\phpdbg_parser.h',
        'sapi\phpdbg\phpdbg_lexer.c'
    )
    $generatedTimestamp = [DateTime]::Now.AddMinutes(1)
    foreach ($relativePath in $generatedFiles) {
        $generatedPath = Join-Path $sourceRoot $relativePath
        if (-not (Test-Path -LiteralPath $generatedPath -PathType Leaf)) {
            throw "The release source archive is missing generated file '$relativePath'."
        }
        (Get-Item -LiteralPath $generatedPath).LastWriteTime = $generatedTimestamp
    }

    Invoke-Batch -NativeCmd $nativeCmd -WorkingDirectory $workRoot -Name 'build' -Lines @(
        'call "%RAPIRA_VCVARS%" %RAPIRA_VCVARS_ARCH%',
        'if errorlevel 1 exit /b %errorlevel%',
        'cd /d "%RAPIRA_PHP_SOURCE%"',
        'nmake /nologo',
        'if errorlevel 1 exit /b %errorlevel%',
        'nmake /nologo build-devel',
        'if errorlevel 1 exit /b %errorlevel%'
    )

    $phpDlls = @(Get-ChildItem -LiteralPath $sourceRoot -Recurse -File -Filter 'php8ts.dll' |
        Where-Object { $_.Directory.Name -eq 'Release_TS' })
    if ($phpDlls.Count -ne 1) {
        throw "Expected exactly one Release_TS\php8ts.dll, found $($phpDlls.Count)."
    }
    $buildDirectory = $phpDlls[0].Directory.FullName
    $develRoots = @(Get-ChildItem -LiteralPath $buildDirectory -Directory -Filter "php-$PhpVersion-devel-*" |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'lib\php8ts.lib') -PathType Leaf })
    if ($develRoots.Count -ne 1) {
        throw "Expected exactly one PHP development tree, found $($develRoots.Count)."
    }

    $runtimeStage = Join-Path $stageRoot 'runtime'
    $runtimeExtStage = Join-Path $runtimeStage 'ext'
    $develStage = Join-Path $stageRoot 'devel'
    New-Item -ItemType Directory -Path $runtimeStage, $runtimeExtStage, $develStage -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $buildDirectory 'php.exe') -Destination $runtimeStage
    foreach ($dll in Get-ChildItem -LiteralPath $buildDirectory -File -Filter '*.dll') {
        $destination = if ($dll.Name -like 'php_*.dll') { $runtimeExtStage } else { $runtimeStage }
        Copy-Item -LiteralPath $dll.FullName -Destination $destination
    }
    Get-ChildItem -LiteralPath $develRoots[0].FullName -Force |
        Copy-Item -Destination $develStage -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $sourceRoot 'LICENSE') -Destination (Join-Path $stageRoot 'PHP-LICENSE.txt')
    [pscustomobject] @{
        php_version = $PhpVersion
        source_sha256 = $SourceSha256
        architecture = $architecture.Name
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stageRoot '.rapira-php.json') -Encoding utf8NoBOM

    $null = Assert-PhpInstall -Root $stageRoot -Architecture $architecture
    New-Item -ItemType Directory -Path (Split-Path -Parent $InstallDirectory) -Force | Out-Null
    Move-Item -LiteralPath $stageRoot -Destination $InstallDirectory
    $buildSucceeded = $true
}
finally {
    if ($buildSucceeded) {
        Remove-ManagedDirectory -Path $workRoot -Root $tempRoot
    }
    else {
        Write-Warning "PHP build files remain at '$workRoot'."
    }
}

$install = Assert-PhpInstall -Root $InstallDirectory -Architecture $architecture
Publish-PhpEnvironment -Install $install -Architecture $architecture
Write-Host "Built PHP $PhpVersion ZTS $($architecture.Name) from verified source at '$InstallDirectory'."
