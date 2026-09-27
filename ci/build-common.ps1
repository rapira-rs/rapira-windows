function Get-Architecture {
    $osArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    $processArchitecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
    if ($processArchitecture -ne $osArchitecture) {
        throw "Run from native $osArchitecture PowerShell; the current process is $processArchitecture."
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
            Write-Host "Using verified archive '$Path'."
            return
        }
        Remove-Item -LiteralPath $Path -Force
    }

    $downloadPath = "$Path.download-$([Guid]::NewGuid().ToString('N'))"
    try {
        Invoke-WebRequest -Uri $Uri -OutFile $downloadPath -MaximumRetryCount 3 -RetryIntervalSec 2 | Out-Null
        if (-not (Test-FileHash -Path $downloadPath -ExpectedSha256 $ExpectedSha256)) {
            $actual = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash.ToLowerInvariant()
            throw "Archive SHA256 mismatch: expected $ExpectedSha256, got $actual."
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
