#Requires -Version 7.0
param([Parameter(Mandatory)] [string] $Target)
$ErrorActionPreference = 'Stop'

$cargo = (Get-Command cargo -CommandType Application).Source
function Invoke-Cargo {
    & $cargo @args
    if ($LASTEXITCODE) { throw "cargo $($args -join ' ') failed: $LASTEXITCODE" }
}

$settings = & cargo llvm-cov show-env --pwsh --target $Target
if ($LASTEXITCODE) { throw 'cargo llvm-cov show-env failed' }
Invoke-Expression ($settings -join "`n")
$env:CARGO_TARGET_DIR = $env:CARGO_LLVM_COV_TARGET_DIR
$env:LLVM_PROFILE_FILE = Join-Path $env:CARGO_TARGET_DIR 'rapira-%p-%m.profraw'
Invoke-Cargo @('llvm-cov', 'clean', '--workspace')
Invoke-Cargo @('test', '--locked', '--workspace', '--target', $Target)
Invoke-Cargo @('build', '--locked', '--bin', 'rapira', '--target', $Target)
$env:RAPIRA_BIN = Join-Path $env:CARGO_TARGET_DIR "$Target\debug\rapira.exe"
Invoke-Cargo @('test', '--locked', '-p', 'tests', '--test', 'e2e', '--features', 'e2e', '--target', $Target, '--', '--test-threads=1')
# Forced exit tests can leave partial profiles. Merge the valid profiles.
# https://llvm.org/docs/CommandGuide/llvm-profdata.html#cmdoption-llvm-profdata-merge-failure-mode
Invoke-Cargo @('llvm-cov', 'report', '--workspace', '--target', $Target, '--failure-mode', 'all', '--lcov', '--output-path', 'lcov.info', '--ignore-filename-regex', '(crates[/\\]tests[/\\]|bindings\.rs$)')
