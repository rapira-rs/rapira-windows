#Requires -Version 7.0
param([Parameter(Mandatory)] [string] $InstallDirectory)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'build-common.ps1')

# The PHP SDK has no ARM64 dependency packages.
# https://downloads.php.net/~windows/php-sdk/deps/
function Build-Arm64Dependencies {
    param(
        [Parameter(Mandatory)] [string] $Root,
        [Parameter(Mandatory)] [string] $Sources
    )

    function Run {
        param([string] $Command, [string[]] $Arguments)
        $executable = @(Get-Command $Command -CommandType Application)[0].Source
        $timer = [Diagnostics.Stopwatch]::StartNew()
        Write-Host "Running $Command $($Arguments -join ' ')"
        & $executable @Arguments
        if ($LASTEXITCODE) { throw "$Command failed with exit code $LASTEXITCODE." }
        Write-Host "$Command completed in $([int]$timer.Elapsed.TotalSeconds) seconds."
    }

    $manifest = Get-Content (Join-Path $PSScriptRoot 'php-dependencies-arm64.json') -Raw | ConvertFrom-Json -AsHashtable
    foreach ($entry in $manifest.GetEnumerator()) {
        $archive = Join-Path $archiveDirectory "$($entry.Key)-$($entry.Value.sha256).tar"
        Get-VerifiedSource -Uri $entry.Value.url -Path $archive -ExpectedSha256 $entry.Value.sha256
        $source = Join-Path $sources $entry.Key
        New-Item -ItemType Directory $source -Force | Out-Null
        Run $nativeTar @('-xf', $archive, '-C', $source, '--strip-components=1')
    }
    foreach ($directory in @('bin', 'lib', 'include', 'share\licenses')) {
        New-Item -ItemType Directory (Join-Path $Root $directory) -Force | Out-Null
    }

    $vs = [IO.Path]::GetFullPath((Join-Path (Split-Path $vcVars -Parent) '..\..\..'))
    Import-Module (Join-Path $vs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-no_logo -arch=arm64 -host_arch=arm64' | Out-Null
    foreach ($tool in @('cl.exe', 'link.exe', 'lib.exe', 'nmake.exe', 'armasm64.exe', 'cmake.exe')) {
        Assert-PeMachine -Path (Get-Command $tool).Source -Expected 0xaa64
    }
    $msbuild = Join-Path $vs 'MSBuild\Current\Bin\arm64\MSBuild.exe'
    Assert-PeMachine -Path $msbuild -Expected 0xaa64
    $sdk = $env:WindowsSDKVersion.TrimEnd('\')
    $toolset = if ($env:VisualStudioVersion -like '18.*') { 'v145' } else { 'v143' }
    $env:INCLUDE = "$Root\include;$env:INCLUDE"
    $env:LIB = "$Root\lib;$env:LIB"
    $env:PATH = "$Root\bin;$env:PATH"

    function Msbuild {
        param([string] $Project, [string[]] $Properties = @())
        Run $msbuild (@($Project, '/nologo', '/m', '/verbosity:minimal', '/p:Configuration=Release', '/p:Platform=ARM64',
            '/p:PreferredToolArchitecture=ARM64', "/p:PlatformToolset=$toolset", "/p:WindowsTargetPlatformVersion=$sdk") + $Properties)
    }

    function Cmake {
        param([string] $Name, [string[]] $Options = @())
        $build = Join-Path $sources "$Name-build"
        Run cmake (@('-S', (Join-Path $sources $Name), '-B', $build, '-G', 'NMake Makefiles',
            '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL',
            '-DCMAKE_POLICY_VERSION_MINIMUM=3.5', '-DBUILD_SHARED_LIBS=ON',
            "-DCMAKE_INSTALL_PREFIX=$Root", "-DCMAKE_PREFIX_PATH=$Root") + $Options)
        Run cmake @('--build', $build)
        Run cmake @('--install', $build)
    }

    # OpenSSL and PostgreSQL use Perl during the build. Build its host executable natively too.
    $perlInstall = Join-Path $sources 'perl-install'
    Push-Location (Join-Path $sources 'perl\win32')
    try {
        Run nmake @('/nologo', 'CCTYPE=MSVC143', "INST_TOP=$perlInstall")
        Run nmake @('/nologo', 'CCTYPE=MSVC143', "INST_TOP=$perlInstall", 'installbare')
    } finally { Pop-Location }
    $env:PATH = "$perlInstall\bin;$env:PATH"
    Assert-PeMachine -Path (Get-Command perl.exe).Source -Expected 0xaa64

    Cmake zlib @('-DZLIB_BUILD_SHARED=OFF', '-DZLIB_BUILD_TESTING=OFF')
    Copy-Item (Join-Path $Root 'lib\zs.lib') (Join-Path $Root 'lib\zlib_a.lib')
    Copy-Item (Join-Path $Root 'lib\zs.lib') (Join-Path $Root 'lib\zlib.lib')
    Push-Location (Join-Path $sources 'openssl')
    try {
        Replace-RequiredText (Join-Path $sources 'openssl\Configurations\windows-makefile.tmpl') `
            '( platform->sharedlib_import($_), platform->staticlib($_) )' `
            '( platform->sharedlib_import($_) // platform->staticlib($_) )' 'OpenSSL shared library selection'
        # PostgreSQL runs openssl.exe to select its OpenSSL build options.
        Run perl @('Configure', 'VC-WIN64-ARM', 'shared', 'no-tests', 'no-docs', 'no-asm', "--prefix=$Root", '--libdir=lib')
        Run nmake @('/nologo', 'install_sw')
    } finally { Pop-Location }

    Cmake libonig @('-DBUILD_SHARED_LIBS=OFF', '-DBUILD_TEST=OFF', '-DINSTALL_DOCUMENTATION=OFF')
    Copy-Item (Join-Path $Root 'lib\onig.lib') (Join-Path $Root 'lib\onig_a.lib')
    Cmake libssh2 @('-DCRYPTO_BACKEND=OpenSSL', '-DBUILD_EXAMPLES=OFF', '-DBUILD_TESTING=OFF')
    Cmake nghttp2 @('-DENABLE_LIB_ONLY=ON', '-DBUILD_STATIC_LIBS=OFF', '-DBUILD_TESTING=OFF')
    Cmake libcurl @('-DBUILD_CURL_EXE=OFF', '-DBUILD_TESTING=OFF', '-DCURL_USE_OPENSSL=ON',
        '-DCURL_USE_SCHANNEL=OFF', '-DCURL_USE_LIBSSH2=ON', '-DUSE_NGHTTP2=ON',
        '-DCURL_USE_LIBPSL=OFF', '-DCURL_BROTLI=OFF', '-DCURL_ZSTD=OFF')
    Copy-Item (Join-Path $Root 'lib\libcurl_imp.lib') (Join-Path $Root 'lib\libcurl.lib')

    $iconv = Join-Path $sources 'libiconv'
    Msbuild (Join-Path $iconv 'MSVC17\libiconv_dll\libiconv_dll.vcxproj') @("/p:OutDir=$Root\bin\")
    Msbuild (Join-Path $iconv 'MSVC17\libiconv_static\libiconv_static.vcxproj')
    Copy-Item (Join-Path $iconv 'MSVC17\ARM64\lib\libiconv_a.lib') (Join-Path $Root 'lib')
    Copy-Item (Join-Path $iconv 'source\include\iconv.h') (Join-Path $Root 'include')
    $gettext = Join-Path $sources 'gettext'
    Replace-RequiredText (Join-Path $gettext 'source\gettext-runtime\config.h') `
        '#define uintmax_t unsigned __int64' '#include <stdint.h>' 'gettext standard integer type'
    Msbuild (Join-Path $gettext 'MSVC17\libintl_dll\libintl_dll.vcxproj')
    Copy-Item (Join-Path $gettext 'MSVC17\libintl_dll\ARM64\Release\libintl.dll') (Join-Path $Root 'bin')
    Copy-Item (Join-Path $gettext 'MSVC17\libintl_dll\ARM64\Release\libintl.lib') (Join-Path $Root 'lib')
    Copy-Item (Join-Path $gettext 'source\gettext-runtime\intl\libgnuintl.h') (Join-Path $Root 'include\libintl.h')

    # PHP calls xmlDllMain to release libxml2 state when an interpreter thread exits.
    Cmake libxml2 @('-DBUILD_SHARED_LIBS=OFF', '-DCMAKE_C_FLAGS=/DLIBXML_STATIC_FOR_DLL',
        '-DLIBXML2_WITH_PROGRAMS=OFF', '-DLIBXML2_WITH_TESTS=OFF',
        '-DLIBXML2_WITH_PYTHON=OFF', '-DLIBXML2_WITH_LZMA=OFF', '-DLIBXML2_WITH_LEGACY=ON',
        '-DLIBXML2_WITH_FTP=ON', "-DIconv_LIBRARY=$Root\lib\libiconv_a.lib")
    Copy-Item (Join-Path $Root 'lib\libxml2s.lib') (Join-Path $Root 'lib\libxml2_a.lib')

    Push-Location (Join-Path $sources 'sqlite3')
    try {
        Run cl @('/nologo', '/O2', '/MD', '/LD', '/DSQLITE_API=__declspec(dllexport)',
            '/DSQLITE_ENABLE_COLUMN_METADATA', '/DSQLITE_ENABLE_FTS5', '/DSQLITE_ENABLE_RTREE',
            '/DSQLITE_THREADSAFE=1', 'sqlite3.c', '/link', "/OUT:$Root\bin\libsqlite3.dll", "/IMPLIB:$Root\lib\libsqlite3.lib")
        Copy-Item sqlite3.h,sqlite3ext.h (Join-Path $Root 'include')
    } finally { Pop-Location }

    # Use the current Winlibs headers with the ARM64 ABI and assembler source.
    # https://github.com/libffi/libffi/tree/v3.8.0/src/aarch64
    $ffi = Join-Path $sources 'libffi'
    $ffiHeader = Join-Path $ffi 'include\ffi.h'
    $text = [IO.File]::ReadAllText($ffiHeader).Replace('X86_WIN64', 'AARCH64')
    [IO.File]::WriteAllText($ffiHeader, $text)
    $config = Join-Path $ffi 'fficonfig.h'
    $text = [IO.File]::ReadAllText($config).Replace('(sizeof(double))', '8').Replace('(sizeof(long double))', '8').Replace('(sizeof(size_t))', '8')
    [IO.File]::WriteAllText($config, $text)
    Push-Location $ffi
    try {
        Run cl @('/nologo', '/c', '/O2', '/MD', '/DFFI_STATIC_BUILD', '/I.', '/Iinclude', '/Isrc\aarch64',
            'src\closures.c', 'src\prep_cif.c', 'src\raw_api.c', 'src\java_raw_api.c', 'src\tramp.c', 'src\types.c', 'src\aarch64\ffi.c')
        Invoke-Batch -NativeCmd $nativeCmd -WorkingDirectory $ffi -Name 'ffi-asm' -Lines @(
            "cd /d `"$ffi`"",
            'cl /nologo /EP /TC /I. /Iinclude /Isrc\aarch64 src\aarch64\win64_armasm.S > win64_armasm.asm',
            'if errorlevel 1 exit /b %errorlevel%',
            'armasm64 -nologo -o win64_armasm.obj win64_armasm.asm',
            'if errorlevel 1 exit /b %errorlevel%'
        )
        Run lib @('/nologo', "/OUT:$Root\lib\libffi.lib", 'closures.obj', 'prep_cif.obj', 'raw_api.obj',
            'java_raw_api.obj', 'tramp.obj', 'types.obj', 'ffi.obj', 'win64_armasm.obj')
        Copy-Item include\ffi.h,fficonfig.h,src\aarch64\ffitarget.h (Join-Path $Root 'include')
    } finally { Pop-Location }

    $pq = Join-Path $sources 'libpq'
    $solution = Join-Path $pq 'src\tools\msvc\Solution.pm'
    Replace-RequiredText $solution "(`$output =~ /^\/favor:<.+AMD64/m) ? 'x64' : 'Win32'" "'ARM64'" 'native PostgreSQL platform'
    Replace-RequiredText $solution 'USE_SSE42_CRC32C_WITH_RUNTIME_CHECK => 1' 'USE_SSE42_CRC32C_WITH_RUNTIME_CHECK => undef' 'PostgreSQL x86 CRC'
    Replace-RequiredText $solution 'USE_SLICING_BY_8_CRC32C => undef' 'USE_SLICING_BY_8_CRC32C => 1' 'PostgreSQL portable CRC'
    Replace-RequiredText (Join-Path $pq 'src\tools\msvc\MSBuildProject.pm') "'MachineX64'" "'MachineARM64'" 'PostgreSQL linker machine'
    Replace-RequiredText (Join-Path $pq 'src\tools\msvc\Mkvcbuild.pm') "if (`$vsVersion >= '9.00')" "if (`$solution->{platform} ne 'ARM64')" 'PostgreSQL CRC source selection'
    "`$config->{openssl} = '$($Root.Replace('\', '/'))';" | Set-Content (Join-Path $pq 'src\tools\msvc\config.pl')
    Push-Location $pq
    try {
        Run perl @('src\tools\msvc\mkvcbuild.pl')
        Msbuild 'libpq.vcxproj'
        Copy-Item Release\libpq\libpq.dll (Join-Path $Root 'bin')
        Copy-Item Release\libpq\libpq.lib (Join-Path $Root 'lib')
        $pqInclude = Join-Path $Root 'include\libpq'
        New-Item -ItemType Directory $pqInclude -Force | Out-Null
        Copy-Item src\include\pg_config.h,src\include\pg_config_ext.h,src\include\postgres_ext.h,src\include\libpq\*.h,src\interfaces\libpq\*.h $pqInclude
    } finally { Pop-Location }

    $icu = Join-Path $sources 'ICU'
    $dataProject = Join-Path $icu 'source\data\makedata.vcxproj'
    [xml]$project = Get-Content $dataProject -Raw
    $testReferences = @($project.SelectNodes("//*[local-name()='ProjectReference']") |
        Where-Object { $_.GetAttribute('Include').StartsWith('..\test\', [StringComparison]::Ordinal) })
    foreach ($reference in $testReferences) {
        $null = $reference.ParentNode.RemoveChild($reference)
    }
    $project.Save($dataProject)
    Msbuild (Join-Path $icu 'source\allinone\allinone.sln') @('/t:makedata')
    Copy-Item (Join-Path $icu 'binARM64\icu*.dll') (Join-Path $Root 'bin')
    Copy-Item (Join-Path $icu 'libARM64\icu*.lib') (Join-Path $Root 'lib')
    Copy-Item (Join-Path $icu 'include\*') (Join-Path $Root 'include') -Recurse -Force

    foreach ($name in $manifest.Keys | Where-Object { $_ -ne 'perl' }) {
        $licenses = Join-Path $Root "share\licenses\$name"
        New-Item -ItemType Directory $licenses -Force | Out-Null
        Get-ChildItem (Join-Path $sources $name) -File -Recurse |
            Where-Object { $_.Name -match '^(COPYING|LICENSE|LICENCE|COPYRIGHT)(\..*)?$' } |
            ForEach-Object {
                $relative = [IO.Path]::GetRelativePath((Join-Path $sources $name), $_.FullName)
                $destination = Join-Path $licenses $relative
                New-Item -ItemType Directory (Split-Path $destination -Parent) -Force | Out-Null
                Copy-Item $_.FullName $destination
            }
    }
    Copy-Item (Join-Path $PSScriptRoot 'php-dependencies-arm64.json') (Join-Path $Root 'share\sources.json')
}

$architecture = Get-Architecture
if ($architecture.Name -ne 'arm64') {
    throw 'Build the ARM64 dependencies on a native ARM64 host.'
}
$tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$InstallDirectory = Assert-ManagedPath -Path $InstallDirectory -Root $tempRoot -Name 'InstallDirectory'
$inputs = @($PSCommandPath, (Join-Path $PSScriptRoot 'build-common.ps1'), (Join-Path $PSScriptRoot 'php-dependencies-arm64.json'))
$fingerprint = ((Get-FileHash -LiteralPath $inputs -Algorithm SHA256).Hash -join '').ToLowerInvariant()
$marker = Join-Path $InstallDirectory '.rapira-dependencies'
if ((Test-Path $marker) -and (Get-Content $marker -Raw).Trim() -eq $fingerprint) {
    Write-Host "Using cached ARM64 dependencies at '$InstallDirectory'."
    return
}

Remove-ManagedDirectory -Path $InstallDirectory -Root $tempRoot
New-Item -ItemType Directory $InstallDirectory -Force | Out-Null
$nativeCmd = Get-NativeSystemExecutable -Name 'cmd.exe' -ExpectedMachine $architecture.PeMachine
$nativeTar = Get-NativeSystemExecutable -Name 'tar.exe' -ExpectedMachine $architecture.PeMachine
$vcVars = Join-Path (Find-VisualStudio) 'VC\Auxiliary\Build\vcvarsall.bat'
$sourceRoot = Join-Path $tempRoot 'rapira-php-source'
$archiveDirectory = Join-Path $sourceRoot 'archives'
$workRoot = Join-Path $sourceRoot "work\dependencies-arm64-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $archiveDirectory, $workRoot -Force | Out-Null
$buildSucceeded = $false
try {
    Build-Arm64Dependencies -Root $InstallDirectory -Sources (Join-Path $workRoot 'source')
    foreach ($dll in Get-ChildItem (Join-Path $InstallDirectory 'bin') -File -Filter '*.dll') {
        Assert-PeMachine -Path $dll.FullName -Expected $architecture.PeMachine
    }
    $fingerprint | Set-Content $marker -Encoding ascii
    $buildSucceeded = $true
} finally {
    if ($buildSucceeded) {
        Remove-ManagedDirectory -Path $workRoot -Root $tempRoot
    } else {
        Write-Warning "Dependency build files remain at '$workRoot'."
    }
}
Write-Host "Built ARM64 dependencies at '$InstallDirectory'."
