#Requires -Version 5.1
param(
    [ValidateSet('8.4', '8.5')] [string] $PhpVersion = '8.5',
    [string] $InstallDirectory = (Join-Path $env:LOCALAPPDATA 'Programs\Rapira')
)

& {
    $ErrorActionPreference = 'Stop'
    if ([Environment]::OSVersion.Platform -ne 'Win32NT') {
        throw 'This installer requires Windows.'
    }
    $InstallDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($InstallDirectory)
    if (Test-Path -LiteralPath $InstallDirectory) {
        throw "The install directory already exists: $InstallDirectory. Select another directory or move the existing installation."
    }
    # Architecture values: https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-processor#properties
    $arch = switch ((Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture) {
        9 { 'x86_64' }
        12 { 'arm64' }
        default { throw 'Rapira requires x64 or ARM64 Windows.' }
    }
    $release = Invoke-RestMethod 'https://api.github.com/repos/rapira-rs/rapira-windows/releases/latest'
    $stem = "rapira-$($release.tag_name)-php$PhpVersion-windows-$arch"
    $archiveName = "$stem.zip"
    $sumsName = "rapira-$($release.tag_name)-windows-$arch-SHA256SUMS.txt"
    $archiveAsset = @($release.assets | Where-Object { $_.name -eq $archiveName })
    $sumsAsset = @($release.assets | Where-Object { $_.name -eq $sumsName })
    if ($archiveAsset.Count -ne 1 -or $sumsAsset.Count -ne 1) {
        throw "The release has no matching archive and checksum assets for PHP $PhpVersion on $arch."
    }

    $temp = Join-Path ([IO.Path]::GetTempPath()) "rapira-install-$([Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory $temp | Out-Null
    try {
        Write-Host "Downloading Rapira $($release.tag_name) with PHP $PhpVersion for $arch..."
        $archive = Join-Path $temp $archiveName
        $sums = Join-Path $temp $sumsName
        Invoke-WebRequest $archiveAsset[0].browser_download_url -OutFile $archive -UseBasicParsing
        Invoke-WebRequest $sumsAsset[0].browser_download_url -OutFile $sums -UseBasicParsing
        $hashLine = @(Get-Content -LiteralPath $sums | Where-Object {
            $_ -match "^[0-9a-fA-F]{64}  $([regex]::Escape($archiveName))$"
        })
        if ($hashLine.Count -ne 1) { throw "The checksum file has no unique entry for $archiveName." }
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $hashLine[0].Substring(0, 64)) {
            throw "SHA256 checksum mismatch for $archiveName."
        }
        Expand-Archive -LiteralPath $archive -DestinationPath $temp
        $bundle = Join-Path $temp $stem
        $binary = Join-Path $bundle 'rapira.exe'
        & $binary --version
        if ($LASTEXITCODE -ne 0) {
            throw 'Rapira could not start. Install the matching Microsoft Visual C++ Redistributable: https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist'
        }
        $ini = Join-Path $bundle 'php.ini'
        $extensionDirectory = (Join-Path $InstallDirectory 'ext').Replace('\', '/')
        $settings = (Get-Content -LiteralPath $ini -Raw).Replace('extension_dir=ext', "extension_dir=`"$extensionDirectory`"")
        [IO.File]::WriteAllText($ini, $settings, [Text.UTF8Encoding]::new($false))
        [IO.Directory]::CreateDirectory((Split-Path $InstallDirectory -Parent)) | Out-Null
        Copy-Item -LiteralPath $bundle -Destination $InstallDirectory -Recurse

        $environment = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            $userPath = $environment.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if ($InstallDirectory -notin ($userPath -split ';')) {
                $kind = if ($null -eq $userPath) { [Microsoft.Win32.RegistryValueKind]::String } else { $environment.GetValueKind('Path') }
                if (-not ('RapiraInstallerEnvironment' -as [type])) {
                    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class RapiraInstallerEnvironment {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint message, UIntPtr wparam, string lparam, uint flags, uint timeout, out UIntPtr result);
}
'@
                }
                $environment.SetValue('Path', "$InstallDirectory;$userPath", $kind)
                # Notify applications of the environment change: https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-settingchange
                $result = [UIntPtr]::Zero
                [RapiraInstallerEnvironment]::SendMessageTimeout([IntPtr] 0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref] $result) | Out-Null
            }
        } finally { $environment.Dispose() }
        if ($InstallDirectory -notin ($env:Path -split ';')) {
            $env:Path = "$InstallDirectory;$env:Path"
        }
        Write-Host "Installed Rapira in $InstallDirectory."
        Write-Host 'Run: rapira serve C:\app\rapira.toml'
    } finally {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}
