#Requires -Version 7.0
param([Parameter(Mandatory)] [string] $Bundle)
$ErrorActionPreference = 'Stop'
$Bundle = (Resolve-Path $Bundle).ProviderPath
$repo = Split-Path $PSScriptRoot -Parent
$smoke = Join-Path ([IO.Path]::GetTempPath()) "rapira-bundle-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $smoke | Out-Null
$expectedVersion = (Get-Content (Join-Path $Bundle 'PHP_VERSION.txt') -Raw).Trim()
$extensions = @(Get-Content (Join-Path $PSScriptRoot 'windows-extensions.txt') |
    ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notmatch '^#' })
ConvertTo-Json -InputObject $extensions | Set-Content (Join-Path $smoke 'extensions.json')
@'
<?php
$handler = static function (): void {
    foreach (json_decode(file_get_contents(__DIR__ . '/extensions.json')) as $extension) {
        if (!extension_loaded($extension)) {
            throw new RuntimeException('Missing extension: ' . $extension);
        }
    }
    header('content-type: text/plain');
    echo PHP_VERSION, '|', PHP_ZTS, '|rapira-windows-bundle-ok';
};
while (\Rapira\handle_request($handler)) {}
'@ | Set-Content (Join-Path $smoke 'worker.php') -Encoding ascii
Copy-Item (Join-Path $repo 'crates\tests\tests\e2e\fixtures\grpc\echo-worker.php') (Join-Path $smoke 'grpc.php')
Copy-Item (Join-Path $repo 'crates\tests\fixtures\grpc\echo.binpb') $smoke

function Free-Port {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { return ([Net.IPEndPoint]$listener.LocalEndpoint).Port }
    finally { $listener.Stop() }
}
$httpPort = Free-Port
$grpcPort = Free-Port
@"
[http]
listen = "127.0.0.1:$httpPort"
[http.pool]
processes = 2
mode = "worker"
entrypoint = "worker.php"
[grpc]
listen = "127.0.0.1:$grpcPort"
descriptor_set = "echo.binpb"
services = ["rapira.test.v1.EchoService"]
[grpc.pool]
processes = 2
entrypoint = "grpc.php"
"@ | Set-Content (Join-Path $smoke 'rapira.toml') -Encoding ascii

$start = [Diagnostics.ProcessStartInfo]::new((Join-Path $Bundle 'rapira.exe'))
$start.WorkingDirectory = $Bundle
$start.UseShellExecute = $false
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.ArgumentList.Add('serve')
$start.ArgumentList.Add((Join-Path $smoke 'rapira.toml'))
$start.Environment['PHPRC'] = Join-Path $Bundle 'php.ini'
$start.Environment['PHP_INI_SCAN_DIR'] = ''
# The loader must find PHP and its dependencies in the bundle.
$start.Environment['PATH'] = "$env:SystemRoot;$env:SystemRoot\System32"
$process = [Diagnostics.Process]::Start($start)
$stdout = $process.StandardOutput.ReadToEndAsync()
$stderr = $process.StandardError.ReadToEndAsync()
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $response = $null
    while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
        try {
            $response = Invoke-WebRequest "http://127.0.0.1:$httpPort/" -TimeoutSec 2
            break
        } catch { Start-Sleep -Milliseconds 100 }
    }
    if ($null -eq $response -or $response.Content.Trim() -ne "$expectedVersion|1|rapira-windows-bundle-ok") {
        throw 'The bundled HTTP pool did not return the expected PHP identity and extension check.'
    }
    $response = Invoke-RestMethod "http://127.0.0.1:$grpcPort/rapira.test.v1.EchoService/Echo" `
        -Method Post -ContentType 'application/json' -Headers @{ 'Connect-Protocol-Version' = '1' } `
        -Body '{"text":"bundle-grpc-ok"}' -TimeoutSec 10
    if ($response.text -ne 'bundle-grpc-ok') { throw 'The bundled gRPC pool did not echo the Connect request.' }
    Write-Host "Bundle passed HTTP, Connect, PHP $expectedVersion, and extension checks with a system-only PATH."
} finally {
    if (-not $process.HasExited) { $process.Kill() }
    $process.WaitForExit()
    Write-Host $stdout.GetAwaiter().GetResult()
    Write-Host $stderr.GetAwaiter().GetResult()
    $process.Dispose()
    Remove-Item $smoke -Recurse -Force
}
