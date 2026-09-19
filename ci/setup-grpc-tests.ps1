#Requires -Version 7.0
param([string]$Directory)
$ErrorActionPreference = 'Stop'

$architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ([Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne $architecture) {
    throw "Run this script from native $architecture PowerShell."
}
$target = switch ($architecture) {
    'Arm64' { 'ARM64' }
    'X64' { 'x64' }
    default { throw "Unsupported architecture: $architecture" }
}
$machine = if ($target -eq 'ARM64') { 0xaa64 } else { 0x8664 }

function Assert-NativeBinary([string]$Path) {
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($Path))
    try {
        $reader.BaseStream.Position = 0x3c
        $reader.BaseStream.Position = $reader.ReadInt32() + 4
        if ($reader.ReadUInt16() -ne $machine) {
            throw "The binary does not match native $architecture tools: $Path"
        }
    } finally {
        $reader.Dispose()
    }
}

if ($env:RAPIRA_PROTOC -and $env:RAPIRA_PROTOBUF_PHP) {
    $protoc = $env:RAPIRA_PROTOC
    $php = $env:RAPIRA_PROTOBUF_PHP
} else {
    if (-not $Directory) {
        $tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
        $Directory = Join-Path $tempRoot "rapira-grpc-tools-$target"
    }
    $Directory = [IO.Path]::GetFullPath($Directory)
    $source = Join-Path $Directory 'protobuf-36.2'
    $build = Join-Path $Directory 'build'
    $protoc = Join-Path $build 'Release\protoc.exe'
    $php = Join-Path $source 'php\src'
    if (-not (Test-Path -LiteralPath $protoc)) {
        New-Item -ItemType Directory -Path $Directory -Force | Out-Null
        $archive = Join-Path $Directory 'protobuf-36.2.zip'
        if (-not (Test-Path -LiteralPath $archive)) {
            Invoke-WebRequest 'https://github.com/protocolbuffers/protobuf/releases/download/v36.2/protobuf-36.2.zip' -OutFile $archive
        }
        $expected = 'ec61440905b46d42dc58ce7b490f0469263a21f93407f741c97e9b6b09533eb6'
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ine $expected) {
            throw "Protobuf source checksum mismatch: $archive"
        }
        if (-not (Test-Path -LiteralPath (Join-Path $source 'CMakeLists.txt'))) {
            Expand-Archive -LiteralPath $archive -DestinationPath $Directory
        }
        $command = Get-Command cmake.exe -ErrorAction SilentlyContinue
        $cmake = if ($command) { $command.Source } else {
            Get-ChildItem -LiteralPath 'C:\Program Files\Microsoft Visual Studio' -Filter cmake.exe -Recurse |
                Select-Object -First 1 -ExpandProperty FullName
        }
        if (-not $cmake) { throw 'Install the native Visual Studio CMake tools.' }
        Assert-NativeBinary $cmake
        & $cmake -S $source -B $build -A $target -T "host=$target" -Dprotobuf_BUILD_TESTS=OFF -Dprotobuf_BUILD_LIBUPB=OFF -Dprotobuf_WITH_ZLIB=OFF -Dprotobuf_INSTALL=OFF
        if ($LASTEXITCODE -ne 0) { throw 'Protobuf configuration failed.' }
        & $cmake --build $build --config Release --target protoc --parallel 4
        if ($LASTEXITCODE -ne 0) { throw 'Protobuf compiler build failed.' }
    }
}

Assert-NativeBinary $protoc
& $protoc --version
if ($LASTEXITCODE -ne 0) { throw 'The protobuf compiler did not start.' }
if (-not (Test-Path -LiteralPath (Join-Path $php 'Google\Protobuf\Internal\Message.php'))) {
    throw "The PHP protobuf library was not found: $php"
}
$env:RAPIRA_PROTOC = (Resolve-Path -LiteralPath $protoc).ProviderPath
$env:RAPIRA_PROTOBUF_PHP = (Resolve-Path -LiteralPath $php).ProviderPath
if ($env:GITHUB_ENV) {
    "RAPIRA_PROTOC=$env:RAPIRA_PROTOC" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
    "RAPIRA_PROTOBUF_PHP=$env:RAPIRA_PROTOBUF_PHP" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
