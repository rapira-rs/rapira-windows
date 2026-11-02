#Requires -Version 5.1
param(
    [Parameter(Mandatory)] [string] $Bundle,
    [Parameter(Mandatory)] [string] $E2e
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
$version = (Get-Content (Join-Path $Bundle 'PHP_VERSION.txt') -Raw).Trim()
if ($version -notmatch '^(8\.[45])\.') { throw "Expected a PHP 8.4 or 8.5 bundle, got $version." }
$minor = $Matches[1]
$temp = Join-Path ([IO.Path]::GetTempPath()) "rapira install $([Guid]::NewGuid().ToString('N'))"
$environment = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
$rawValue = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
$savedUserPath = $environment.GetValue('Path', $null, $rawValue)
$savedUserPathKind = if ($null -ne $savedUserPath) { $environment.GetValueKind('Path') }
$savedProcessPath = $env:Path
$savedLocalAppData = $env:LOCALAPPDATA
$savedRapiraBin = $env:RAPIRA_BIN
$cases = @(
    @{ name = 'expandable PATH'; path = '%USERPROFILE%\bin;%LOCALAPPDATA%\Programs\other'; kind = 'ExpandString'; defaults = $false; failure = $null },
    @{ name = "PHP $minor installation"; path = 'C:\existing one;C:\existing two'; kind = 'String'; defaults = ($minor -eq '8.5'); failure = $null },
    @{ name = 'literal PATH'; path = '%USERPROFILE%\literal'; kind = 'String'; defaults = $false; failure = $null },
    @{ name = 'absent PATH'; path = $null; kind = 'String'; defaults = $false; failure = $null },
    @{ name = 'existing PATH entry'; path = '%USERPROFILE%\bin'; kind = 'ExpandString'; defaults = $false; failure = $null },
    @{ name = 'checksum mismatch'; path = '%USERPROFILE%\bin'; kind = 'ExpandString'; defaults = $false; failure = 'checksum'; message = 'checksum mismatch' },
    @{ name = 'missing archive'; path = '%USERPROFILE%\bin'; kind = 'ExpandString'; defaults = $false; failure = 'asset'; message = 'no matching archive and checksum assets' },
    @{ name = 'existing destination'; path = '%USERPROFILE%\bin'; kind = 'ExpandString'; defaults = $false; failure = 'existing'; message = 'already exists' }
)
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
    $stem = "rapira-v9.8.7-php$minor-windows-$arch"
    Copy-Item -LiteralPath $Bundle -Destination (Join-Path $temp $stem) -Recurse
    $archive = Join-Path $temp "$stem.zip"
    Compress-Archive -LiteralPath (Join-Path $temp $stem) -DestinationPath $archive
    $hash = "$((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant())  $stem.zip"
    $assets = @(@{ name = "$stem.zip"; browser_download_url = "https://github.com/rapira-rs/rapira-windows/releases/download/v9.8.7/$stem.zip" })
    $sums = "rapira-v9.8.7-windows-$arch-SHA256SUMS.txt"
    $assets += @{ name = $sums; browser_download_url = "https://github.com/rapira-rs/rapira-windows/releases/download/v9.8.7/$sums" }
    foreach ($case in $cases) {
        $env:LOCALAPPDATA = Join-Path $temp $case.name
        $destination = Join-Path $env:LOCALAPPDATA 'Programs\Rapira'
        $userPath = $case.path
        $processPath = $savedProcessPath
        if ($case.name -eq 'existing PATH entry') {
            $userPath += ";$destination"
            $processPath += ";$destination"
        }
        $pathKind = [Microsoft.Win32.RegistryValueKind] $case.kind
        if ($null -eq $userPath) { $environment.DeleteValue('Path', $false) }
        else { $environment.SetValue('Path', $userPath, $pathKind) }
        $env:Path = $processPath
        $script:release = @{ tag_name = 'v9.8.7'; assets = $assets }
        $hash | Set-Content (Join-Path $temp $sums) -Encoding ascii
        if ($case.failure -eq 'checksum') {
            $hash -replace '^[0-9a-f]{64}', ('0' * 64) | Set-Content (Join-Path $temp $sums) -Encoding ascii
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
                else { & ([ScriptBlock]::Create($source)) -PhpVersion $minor -InstallDirectory $destination }
            }
        } catch { $failure = $_ }
        $actualUserPath = $environment.GetValue('Path', $null, $rawValue)
        if ($case.failure) {
            if (-not $failure) { throw "$($case.name): installation did not fail." }
            if ($failure.Exception.Message -notlike "*$($case.message)*") { throw $failure }
            if ($case.failure -eq 'existing') {
                if (@(Get-ChildItem -LiteralPath $destination).Count -ne 1 -or (Get-Content (Join-Path $destination 'php.ini') -Raw).Trim() -ne 'keep this configuration') {
                    throw 'The existing configuration changed.'
                }
            } elseif (Test-Path $destination) { throw "$($case.name): the destination exists after failure." }
            if ($env:Path -ne $processPath -or $actualUserPath -ne $userPath -or $environment.GetValueKind('Path') -ne $pathKind) {
                throw "$($case.name): PATH changed after failure."
            }
        } else {
            if ($failure) { throw $failure }
            $expectedUserPath = if ($case.name -eq 'existing PATH entry') { $userPath } else { "$destination;$userPath" }
            $expectedProcessPath = if ($case.name -eq 'existing PATH entry') { $processPath } else { "$destination;$processPath" }
            if ($actualUserPath -ne $expectedUserPath -or $environment.GetValueKind('Path') -ne $pathKind) {
                throw "$($case.name): raw PATH or its registry type changed."
            }
            if ($env:Path -ne $expectedProcessPath) { throw "$($case.name): process PATH is incorrect." }
            if ((Get-Command rapira.exe).Source -ne (Join-Path $destination 'rapira.exe')) {
                throw "$($case.name): PATH does not select the installed executable."
            }
            $env:RAPIRA_BIN = (Get-Command rapira.exe).Source
            & $E2e installation::installed_bundle_uses_its_php_and_extensions --exact --ignored
            if ($LASTEXITCODE -ne 0) { throw "$($case.name): installed bundle test failed." }
        }
        Write-Host "PASS: $($case.name)"
    }
} finally {
    if ($null -eq $savedUserPath) { $environment.DeleteValue('Path', $false) }
    else { $environment.SetValue('Path', $savedUserPath, $savedUserPathKind) }
    $environment.Dispose()
    if ('RapiraInstallerEnvironment' -as [type]) {
        $result = [UIntPtr]::Zero
        [RapiraInstallerEnvironment]::SendMessageTimeout([IntPtr] 0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref] $result) | Out-Null
    }
    $env:Path = $savedProcessPath
    $env:LOCALAPPDATA = $savedLocalAppData
    $env:RAPIRA_BIN = $savedRapiraBin
    if (Test-Path $temp) { Remove-Item -LiteralPath $temp -Recurse -Force }
}
