#Requires -Version 7.0
param(
    [Parameter(Mandatory)] [string] $Binary,
    [Parameter(Mandatory)] [string] $PhpInstall,
    [Parameter(Mandatory)] [string] $Destination
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$runtime = Join-Path $PhpInstall 'runtime'
$identity = Get-Content (Join-Path $PhpInstall '.rapira-php.json') -Raw | ConvertFrom-Json
$extensions = @(Get-Content (Join-Path $PSScriptRoot 'windows-extensions.txt') |
    ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notmatch '^#' })
$ext = Join-Path $Destination 'ext'
New-Item -ItemType Directory $ext -Force | Out-Null
Copy-Item $Binary (Join-Path $Destination 'rapira.exe')
foreach ($file in @('README.md', 'UPSTREAM.md', 'LICENSE')) {
    Copy-Item (Join-Path $repo $file) $Destination
}
Copy-Item (Join-Path $repo 'examples') $Destination -Recurse
Copy-Item (Join-Path $PhpInstall 'PHP-LICENSE.txt') $Destination
Copy-Item (Join-Path $PhpInstall 'share') $Destination -Recurse
Copy-Item (Join-Path $PhpInstall '.rapira-php.json') (Join-Path $Destination 'PHP_BUILD.json')
$identity.php_version | Set-Content (Join-Path $Destination 'PHP_VERSION.txt') -Encoding ascii
Copy-Item (Join-Path $runtime '*.dll') $Destination
Copy-Item (Join-Path $runtime 'ext\*.dll') $ext

$ini = @('extension_dir=ext')
if (Test-Path (Join-Path $ext 'php_opcache.dll')) {
    $ini += 'zend_extension=php_opcache.dll'
}
foreach ($extension in $extensions) {
    if (-not (Test-Path (Join-Path $ext "php_$extension.dll"))) {
        throw "Missing bundled extension: $extension"
    }
    $ini += "extension=php_$extension.dll"
}
$ini | Set-Content (Join-Path $Destination 'php.ini') -Encoding ascii
& (Join-Path $PSScriptRoot 'test-bundle.ps1') -Bundle $Destination
