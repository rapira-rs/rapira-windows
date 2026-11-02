#Requires -Version 5.1
param(
    [Parameter(Mandatory)] [string] $Bundle,
    [string] $Bundle84
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$installer = Join-Path $repo 'install.ps1'
$source = Get-Content -LiteralPath $installer -Raw
$arch = switch ((Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture) {
    9 { 'x86_64' }
    12 { 'arm64' }
    default { throw 'The installer test requires native x64 or ARM64 Windows.' }
}
$temp = Join-Path ([IO.Path]::GetTempPath()) "rapira install $([Guid]::NewGuid().ToString('N'))"
$savedUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$savedProcessPath = $env:Path
$savedLocalAppData = $env:LOCALAPPDATA
$cases = @(
    @{ name = 'default install through Invoke-Expression'; minor = '8.5'; defaults = $true; failure = $null },
    @{ name = 'existing PATH entry'; minor = '8.5'; defaults = $false; failure = $null },
    @{ name = 'checksum mismatch'; minor = '8.5'; defaults = $false; failure = 'checksum'; message = 'checksum mismatch' },
    @{ name = 'missing archive'; minor = '8.5'; defaults = $false; failure = 'asset'; message = 'no matching archive and checksum assets' },
    @{ name = 'existing destination'; minor = '8.5'; defaults = $false; failure = 'existing'; message = 'already exists' }
)
if ($Bundle84) {
    $cases += @{ name = 'PHP 8.4 selection'; minor = '8.4'; defaults = $false; failure = $null }
}
$bundles = @{ '8.5' = $Bundle }
if ($Bundle84) { $bundles['8.4'] = $Bundle84 }
function Invoke-RestMethod {
    param([string] $Uri, [hashtable] $Headers, [switch] $UseBasicParsing)
    if ($Uri -ne 'https://api.github.com/repos/rapira-rs/rapira-windows/releases/latest') {
        throw "Unexpected metadata URL: $Uri"
    }
    return $script:release
}
function Invoke-WebRequest {
    param([string] $Uri, [string] $OutFile, [hashtable] $Headers, [switch] $UseBasicParsing)
    $name = [IO.Path]::GetFileName(([Uri] $Uri).AbsolutePath)
    if ($Uri -ne "https://github.com/rapira-rs/rapira-windows/releases/download/v9.8.7/$name") {
        throw "Unexpected download URL: $Uri"
    }
    $file = Join-Path $script:temp $name
    if ($OutFile) { Copy-Item -LiteralPath $file -Destination $OutFile }
    else { return @{ Content = Get-Content -LiteralPath $file -Raw } }
}
try {
    New-Item -ItemType Directory $temp | Out-Null
    $assets = @()
    $hashes = @()
    foreach ($minor in $bundles.Keys) {
        $version = (Get-Content (Join-Path $bundles[$minor] 'PHP_VERSION.txt') -Raw).Trim()
        if (-not $version.StartsWith("$minor.")) { throw "Expected a PHP $minor bundle, got $version." }
        $stem = "rapira-v9.8.7-php$minor-windows-$arch"
        Copy-Item -LiteralPath $bundles[$minor] -Destination (Join-Path $temp $stem) -Recurse
        $archive = Join-Path $temp "$stem.zip"
        Compress-Archive -LiteralPath (Join-Path $temp $stem) -DestinationPath $archive
        $hashes += "$((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant())  $stem.zip"
        $assets += @{ name = "$stem.zip"; browser_download_url = "https://github.com/rapira-rs/rapira-windows/releases/download/v9.8.7/$stem.zip" }
    }
    $sums = "rapira-v9.8.7-windows-$arch-SHA256SUMS.txt"
    $assets += @{ name = $sums; browser_download_url = "https://github.com/rapira-rs/rapira-windows/releases/download/v9.8.7/$sums" }
    foreach ($case in $cases) {
        $env:LOCALAPPDATA = Join-Path $temp $case.name
        $destination = Join-Path $env:LOCALAPPDATA 'Programs\Rapira'
        $userPath = 'C:\existing one;C:\existing two'
        $processPath = $savedProcessPath
        if ($case.name -eq 'existing PATH entry') {
            $userPath += ";$destination"
            $processPath += ";$destination"
        }
        [Environment]::SetEnvironmentVariable('Path', $userPath, 'User')
        $env:Path = $processPath
        $script:release = @{ tag_name = 'v9.8.7'; assets = $assets }
        $hashes | Set-Content (Join-Path $temp $sums) -Encoding ascii
        if ($case.failure -eq 'checksum') {
            $hashes -replace '^[0-9a-f]{64}', ('0' * 64) | Set-Content (Join-Path $temp $sums) -Encoding ascii
        }
        if ($case.failure -eq 'asset') { $script:release.assets = @($assets | Where-Object { $_.name -eq $sums }) }
        if ($case.failure -eq 'existing') {
            New-Item -ItemType Directory $destination -Force | Out-Null
            'keep this configuration' | Set-Content (Join-Path $destination 'php.ini')
        }
        $failure = $null
        try {
            & {
                if ($case.defaults) { $source | Invoke-Expression }
                else { & ([ScriptBlock]::Create($source)) -PhpVersion $case.minor -InstallDirectory $destination }
            }
        } catch { $failure = $_ }
        if ($case.failure) {
            if (-not $failure) { throw "$($case.name): installation did not fail." }
            if ($failure.Exception.Message -notlike "*$($case.message)*") { throw $failure }
            if ($case.failure -eq 'existing') {
                if (@(Get-ChildItem -LiteralPath $destination).Count -ne 1 -or (Get-Content (Join-Path $destination 'php.ini') -Raw).Trim() -ne 'keep this configuration') {
                    throw 'The existing configuration changed.'
                }
            } elseif (Test-Path $destination) { throw "$($case.name): the destination exists after failure." }
            if ($env:Path -ne $processPath -or [Environment]::GetEnvironmentVariable('Path', 'User') -ne $userPath) {
                throw "$($case.name): PATH changed after failure."
            }
        } else {
            if ($failure) { throw $failure }
            foreach ($path in @($env:Path, [Environment]::GetEnvironmentVariable('Path', 'User'))) {
                if (@($path -split ';' | Where-Object { $_ -eq $destination }).Count -ne 1) {
                    throw "$($case.name): the install directory must occur once in PATH."
                }
            }
            if ($env:Path -notlike "*$savedProcessPath*" -or [Environment]::GetEnvironmentVariable('Path', 'User') -notlike '*C:\existing one;C:\existing two*') {
                throw "$($case.name): existing PATH entries changed."
            }
            if ((Get-Command rapira.exe).Source -ne (Join-Path $destination 'rapira.exe')) {
                throw "$($case.name): PATH does not select the installed executable."
            }
            $extensions = @(Get-Content (Join-Path $PSScriptRoot 'windows-extensions.txt') | ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notmatch '^#' })
            ConvertTo-Json -InputObject $extensions | Set-Content (Join-Path $temp 'extensions.json')
            @'
<?php
$handler = static function (): void {
    foreach (json_decode(file_get_contents(__DIR__ . '/extensions.json')) as $extension) {
        if (!extension_loaded($extension)) { throw new RuntimeException('Missing extension: ' . $extension); }
    }
    echo PHP_VERSION, '|', PHP_ZTS;
};
while (\Rapira\handle_request($handler)) {}
'@ | Set-Content (Join-Path $temp 'worker.php') -Encoding ascii
            $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
            $listener.Start()
            $port = ([Net.IPEndPoint] $listener.LocalEndpoint).Port
            $listener.Stop()
            @"
[http]
listen = "127.0.0.1:$port"
[http.pool]
mode = "worker"
processes = 1
entrypoint = "worker.php"
"@ | Set-Content (Join-Path $temp 'rapira.toml') -Encoding ascii
            $start = [Diagnostics.ProcessStartInfo]::new((Get-Command rapira.exe).Source)
            $start.WorkingDirectory = $temp
            $start.UseShellExecute = $false
            $start.Arguments = 'serve "' + (Join-Path $temp 'rapira.toml') + '"'
            $start.EnvironmentVariables.Remove('PHPRC')
            $start.EnvironmentVariables.Remove('PHP_INI_SCAN_DIR')
            $process = [Diagnostics.Process]::Start($start)
            try {
                $response = $null
                $deadline = [DateTime]::UtcNow.AddSeconds(30)
                while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
                    try {
                        $response = Microsoft.PowerShell.Utility\Invoke-RestMethod "http://127.0.0.1:$port/" -TimeoutSec 2
                        break
                    } catch { Start-Sleep -Milliseconds 100 }
                }
                $version = (Get-Content (Join-Path $bundles[$case.minor] 'PHP_VERSION.txt') -Raw).Trim()
                if ($response -ne "$version|1") { throw "$($case.name): PHP or its extensions failed outside the install directory." }
            } finally {
                if (-not $process.HasExited) { $process.Kill() }
                $process.WaitForExit()
                $process.Dispose()
            }
        }
        Write-Host "PASS: $($case.name)"
    }
} finally {
    [Environment]::SetEnvironmentVariable('Path', $savedUserPath, 'User')
    $env:Path = $savedProcessPath
    $env:LOCALAPPDATA = $savedLocalAppData
    if (Test-Path $temp) { Remove-Item -LiteralPath $temp -Recurse -Force }
}
