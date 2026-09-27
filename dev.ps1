#Requires -Version 7.0
# Configure the native Windows tools and run a development task.
# .\dev.ps1 -Devel C:\php\php-devel -Runtime C:\php\php-runtime -Task build
# .\dev.ps1 -Runtime C:\php\php-runtime -Task stubs -PhpSrc C:\src\php-src
# .\dev.ps1 -Devel C:\php\php-devel -Task clangd
param(
    [ValidateSet('build', 'clippy', 'test', 'test_e2e', 'coverage', 'stubs', 'grpc_fixtures', 'clangd')]
    [string]$Task = 'build',
    [string]$Devel,
    [string]$Runtime,
    [string]$Llvm = 'C:\Program Files\LLVM\bin',
    [string]$PhpSrc,
    [string]$TestFilter,
    [switch]$NoRun,
    [switch]$Release
)
$ErrorActionPreference = 'Stop'

$runtimeOsArchitecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
$processArchitecture = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
if ($processArchitecture -ne $runtimeOsArchitecture) {
    throw "Run dev.ps1 from native $runtimeOsArchitecture PowerShell; the current process is $processArchitecture."
}

$nativeArchitecture = $null
try {
    $nativeArchitecture = (Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment').PROCESSOR_ARCHITECTURE
} catch {
    $nativeArchitecture = $null
}
if ([string]::IsNullOrWhiteSpace($nativeArchitecture)) {
    $nativeArchitecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
}
switch ($nativeArchitecture.ToUpperInvariant()) {
    'ARM64' {
        $osArchitecture = 'Arm64'
        $target = 'aarch64-pc-windows-msvc'
        $vsArchitecture = 'arm64'
        $phpBuildArchitecture = 'arm64'
        $peMachine = 0xAA64
    }
    { $_ -in @('AMD64', 'X64') } {
        $osArchitecture = 'X64'
        $target = 'x86_64-pc-windows-msvc'
        $vsArchitecture = 'amd64'
        $phpBuildArchitecture = 'x64'
        $peMachine = 0x8664
    }
    default {
        throw "Unsupported Windows architecture: $nativeArchitecture"
    }
}

function Assert-File {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required file not found: $Path"
    }
}

function Invoke-Tool {
    param([string]$Command, [string[]]$Arguments)
    & $Command @Arguments
    $toolExitCode = $LASTEXITCODE
    if ($toolExitCode -ne 0) {
        exit $toolExitCode
    }
}

function Get-PeMachine {
    param([string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    $reader = New-Object System.IO.BinaryReader($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) {
            throw "Not a PE file: $Path"
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadInt32()
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "Invalid PE signature: $Path"
        }
        return $reader.ReadUInt16()
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Find-NativeVisualStudio {
    $root = Join-Path $env:SystemDrive 'Program Files\Microsoft Visual Studio'
    if (-not (Test-Path -LiteralPath $root -PathType Container)) {
        throw "Visual Studio was not found below: $root"
    }
    $candidates = foreach ($versionDirectory in Get-ChildItem -LiteralPath $root -Directory) {
        foreach ($editionDirectory in Get-ChildItem -LiteralPath $versionDirectory.FullName -Directory) {
            $devShell = Join-Path $editionDirectory.FullName 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll'
            if (Test-Path -LiteralPath $devShell -PathType Leaf) {
                $rank = if ($versionDirectory.Name -eq '2022') {
                    17
                } elseif ($versionDirectory.Name -eq '2019') {
                    16
                } elseif ($versionDirectory.Name -match '^\d+$') {
                    [int]$versionDirectory.Name
                } else {
                    0
                }
                [pscustomobject]@{ Path = $editionDirectory.FullName; Rank = $rank }
            }
        }
    }
    $selected = $candidates | Sort-Object Rank -Descending | Select-Object -First 1
    if ($null -eq $selected) {
        throw "Visual Studio developer shell was not found below: $root"
    }
    return $selected.Path
}

if ($Task -eq 'grpc_fixtures') {
    $go = (Get-Command go -CommandType Application).Source
    $expected = if ($osArchitecture -eq 'Arm64') { 'arm64' } else { 'amd64' }
    if ((& $go env GOHOSTARCH) -ne $expected) { throw "Go must run natively on $expected." }
    Push-Location (Join-Path $PSScriptRoot 'crates\tests')
    try {
        $buf = @('run', 'github.com/bufbuild/buf/cmd/buf@v1.73.0', 'build')
        Invoke-Tool $go ($buf + @('fixtures/grpc', '--as-file-descriptor-set', '-o', 'fixtures/grpc/echo.binpb'))
        Invoke-Tool $go ($buf + @('fixtures/grpc', '--as-file-descriptor-set', '--exclude-imports', '-o', 'fixtures/grpc/echo-no-imports.binpb'))
        Invoke-Tool $go ($buf + @('fixtures/grpc_options', '--as-file-descriptor-set', '--path', 'fixtures/grpc_options/ping.proto', '--exclude-imports', '-o', 'fixtures/grpc_options/ping-no-imports.binpb'))
    } finally { Pop-Location }
    exit 0
}

if ($Task -ne 'stubs') {
    if ([string]::IsNullOrWhiteSpace($Devel)) {
        throw 'Pass -Devel with a PHP devel pack that contains a ZTS import library.'
    }
    $phpTsLib = Join-Path $Devel 'lib\php8ts.lib'
    $phpDebugLib = Join-Path $Devel 'lib\php8ts_debug.lib'
    if (-not (Test-Path -LiteralPath $phpTsLib -PathType Leaf) -and
        -not (Test-Path -LiteralPath $phpDebugLib -PathType Leaf)) {
        throw "Required ZTS import library not found under: $Devel\lib"
    }
    $phpDebug = Test-Path -LiteralPath $phpDebugLib -PathType Leaf
    $Devel = (Resolve-Path -LiteralPath $Devel).ProviderPath
    $phpConfig = Join-Path $Devel 'include\main\config.w32.h'
    Assert-File $phpConfig
    $phpConfigText = [System.IO.File]::ReadAllText($phpConfig)
    if ($phpConfigText -notmatch ('(?m)^#define PHP_BUILD_ARCH "{0}"\r?$' -f $phpBuildArchitecture)) {
        throw "PHP devel architecture does not match native $osArchitecture tools: $Devel"
    }
}

$needsRuntime = $Task -ne 'clangd'
if ($needsRuntime) {
    if ([string]::IsNullOrWhiteSpace($Runtime)) {
        throw 'Pass -Runtime with a PHP ZTS runtime directory.'
    }
    $phpDll = Join-Path $Runtime 'php8ts.dll'
    Assert-File $phpDll
    $runtimeMachine = Get-PeMachine $phpDll
    if ($runtimeMachine -ne $peMachine) {
        throw "PHP runtime architecture does not match native $osArchitecture tools: $Runtime"
    }
    $Runtime = (Resolve-Path -LiteralPath $Runtime).ProviderPath
}

if ($Task -eq 'stubs') {
    $phpExe = Join-Path $Runtime 'php.exe'
    Assert-File $phpExe
    if ((Get-PeMachine $phpExe) -ne $peMachine) {
        throw "PHP executable architecture does not match native $osArchitecture tools: $phpExe"
    }
    if ([string]::IsNullOrWhiteSpace($PhpSrc)) {
        throw 'Pass -PhpSrc with a php-src checkout that contains build\gen_stub.php.'
    }
    $generator = Join-Path $PhpSrc 'build\gen_stub.php'
    Assert-File $generator
    $generator = (Resolve-Path -LiteralPath $generator).ProviderPath
    $stubFiles = @(Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'crates') -Filter '*.stub.php' -File -Recurse)
    if ($stubFiles.Count -eq 0) {
        throw 'No .stub.php files found in crates.'
    }
} else {
    Assert-File (Join-Path $Llvm 'libclang.dll')
    Assert-File (Join-Path $Llvm 'clang.exe')
    $llvmDirectory = (Resolve-Path -LiteralPath $Llvm).ProviderPath
    $clangPath = Join-Path $llvmDirectory 'clang.exe'
    if ((Get-PeMachine $clangPath) -ne $peMachine -or
        (Get-PeMachine (Join-Path $llvmDirectory 'libclang.dll')) -ne $peMachine) {
        throw "LLVM architecture does not match native $osArchitecture tools: $llvmDirectory"
    }
    $devShellArguments = "-no_logo -arch=$vsArchitecture -host_arch=$vsArchitecture"
    $vsInstall = Find-NativeVisualStudio
    $devShell = Join-Path $vsInstall 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll'
    Assert-File $devShell
    Import-Module $devShell
    Enter-VsDevShell -VsInstallPath $vsInstall -SkipAutomaticLocation -DevCmdArguments $devShellArguments | Out-Null
    $cl = (Get-Command cl.exe -CommandType Application).Source
    if ((Get-PeMachine $cl) -ne $peMachine) {
        throw "MSVC host tools do not match native $($osArchitecture): $cl"
    }
    if ($Task -ne 'clangd') {
        $env:PHP_DEVEL_DIR = $Devel
        $env:LIBCLANG_PATH = $llvmDirectory
        $env:CLANG_PATH = $clangPath
    }
}

if ($needsRuntime) {
    $env:PHP_RUNTIME = $Runtime
    $env:PATH = "$Runtime;$env:PATH"
}

Push-Location -LiteralPath $PSScriptRoot
try {
    if ($Task -eq 'stubs') {
        $stubgenDir = Join-Path $PSScriptRoot 'target\stubgen'
        New-Item -ItemType Directory -Path $stubgenDir -Force | Out-Null
        $localGenerator = Join-Path $stubgenDir 'gen_stub.php'
        Copy-Item -LiteralPath $generator -Destination $localGenerator -Force
        foreach ($stubFile in $stubFiles) {
            Invoke-Tool -Command $phpExe -Arguments @('-n', $localGenerator, '--force-regeneration', $stubFile.FullName)
        }
    } elseif ($Task -eq 'clangd') {
        $clangCl = Join-Path $llvmDirectory 'clang-cl.exe'
        Assert-File $clangCl

        $includeRoot = Join-Path $Devel 'include'
        $includeDirs = @(
            $includeRoot,
            (Join-Path $includeRoot 'main'),
            (Join-Path $includeRoot 'Zend'),
            (Join-Path $includeRoot 'TSRM'),
            (Join-Path $includeRoot 'ext'),
            (Join-Path $includeRoot 'win32')
        )
        foreach ($includeDir in $includeDirs) {
            if (-not (Test-Path -LiteralPath $includeDir -PathType Container)) {
                throw "Required include directory not found: $includeDir"
            }
        }

        $databaseDir = Join-Path $PSScriptRoot 'target\clangd'
        New-Item -ItemType Directory -Path $databaseDir -Force | Out-Null
        $utf8NoBom = New-Object System.Text.UTF8Encoding($false)

        $manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'crates\sapi\Cargo.toml') -Raw
        $versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"')
        if (-not $versionMatch.Success) {
            throw 'Could not read sapi package version from crates\sapi\Cargo.toml.'
        }
        $phpSysVersion = $versionMatch.Groups[1].Value

        $commonArguments = @(
            $clangCl,
            "--target=$target",
            '/nologo',
            '/TC',
            '/MD',
            '/X',
            '/clang:-Wno-visibility',
            "/DRAPIRA_VERSION=`"$phpSysVersion`"",
            '/DZTS',
            '/DZEND_WIN32=1',
            '/DPHP_WIN32=1',
            '/DWIN32=1',
            '/DWINDOWS=1',
            '/D_WINDOWS=1',
            '/D_MBCS=1',
            '/D_USE_MATH_DEFINES=1',
            "/DZEND_DEBUG=$([int]$phpDebug)",
            '/DPHP_HAVE_BUILTIN_SADDL_OVERFLOW=1',
            '/DPHP_HAVE_BUILTIN_SADDLL_OVERFLOW=1',
            '/DPHP_HAVE_BUILTIN_SSUBL_OVERFLOW=1',
            '/DPHP_HAVE_BUILTIN_SSUBLL_OVERFLOW=1',
            '/DPHP_HAVE_BUILTIN_SMULL_OVERFLOW=1',
            '/DPHP_HAVE_BUILTIN_SMULLL_OVERFLOW=1'
        )
        foreach ($includeDir in $includeDirs) {
            $commonArguments += "/I$includeDir"
        }
        $systemIncludeDirs = @($env:INCLUDE -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
        if ($systemIncludeDirs.Count -eq 0) {
            throw 'The Visual Studio developer shell did not provide system include directories.'
        }
        foreach ($includeDir in $systemIncludeDirs) {
            if (-not (Test-Path -LiteralPath $includeDir -PathType Container)) {
                throw "Visual Studio include directory not found: $includeDir"
            }
            $commonArguments += "/imsvc$((Resolve-Path -LiteralPath $includeDir).ProviderPath)"
        }

        $commonArguments += "/I$(Join-Path $PSScriptRoot 'crates\sapi')"
        $sources = Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'crates') -Filter '*.c' -File -Recurse
        $commands = foreach ($sourceFile in $sources) {
            $source = $sourceFile.FullName
            [ordered]@{
                directory = $PSScriptRoot
                file = $source
                arguments = @($commonArguments + $source)
            }
        }

        $database = Join-Path $databaseDir 'compile_commands.json'
        $json = ConvertTo-Json -InputObject @($commands) -Depth 4
        [System.IO.File]::WriteAllText($database, $json + [Environment]::NewLine, $utf8NoBom)
        Write-Output "Wrote $database"
    } else {
        $cargo = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
        if (-not $cargo) {
            $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
            $env:PATH = "$(Join-Path $cargoHome 'bin');$env:PATH"
            $cargo = Get-Command cargo -CommandType Application
        }
        $profile = if ($Release) { @('--release') } else { @() }
        $testOptions = @()
        if ($NoRun) { $testOptions += '--no-run' }
        if ($TestFilter) { $testOptions += $TestFilter }
        switch ($Task) {
            'clippy' {
                Invoke-Tool -Command $cargo.Source -Arguments @('clippy', '--locked', '--workspace', '--all-targets', '--target', $target, '--', '-D', 'warnings')
                Invoke-Tool -Command $cargo.Source -Arguments @('clippy', '--locked', '-p', 'tests', '--features', 'e2e', '--tests', '--target', $target, '--', '-D', 'warnings')
            }
            'build' {
                Invoke-Tool -Command $cargo.Source -Arguments (@('build', '--locked', '--target', $target) + $profile)
            }
            'test' {
                Invoke-Tool -Command $cargo.Source -Arguments (@('test', '--locked', '--workspace', '--target', $target) + $profile + $testOptions)
            }
            'test_e2e' {
                Invoke-Tool -Command $cargo.Source -Arguments (@('build', '--locked', '--bin', 'rapira', '--target', $target) + $profile)
                Invoke-Tool -Command $cargo.Source -Arguments (@('test', '--locked', '-p', 'tests', '--test', 'e2e', '--features', 'e2e', '--target', $target) + $profile + $testOptions + @('--', '--test-threads=1'))
            }
            'coverage' {
                Invoke-Tool -Command (Join-Path $PSHOME 'pwsh.exe') -Arguments @('-NoProfile', '-File', 'ci/coverage.ps1', '-Target', $target)
            }
        }
    }
} finally {
    Pop-Location
}
exit 0
