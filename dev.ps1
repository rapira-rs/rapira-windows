#requires -Version 5
# dev.ps1 — set the PHP/LLVM build env, then run any cargo command.
#   .\dev.ps1 -Devel C:\php\php-8.4-devel-vs17-x64 -Runtime C:\php\php-8.4-ts-x64 build --release --bin rapira
#   .\dev.ps1 -Devel ... -Runtime ... test --workspace
param(
    [Parameter(Mandatory)][string]$Devel,                       # devel pack root (has \lib\php8ts.lib)
    [Parameter(Mandatory)][string]$Runtime,                     # binary zip root (has php8ts.dll)
    [string]$Llvm = 'C:\Program Files\LLVM\bin',                # libclang for bindgen
    [Parameter(ValueFromRemainingArguments)][string[]]$CargoArgs
)
$ErrorActionPreference = 'Stop'
$env:PHP_DEVEL_DIR = $Devel
$env:RUSTFLAGS     = "-L native=$Devel\lib"
$env:LIBCLANG_PATH = $Llvm
$env:PATH          = "$Runtime;$env:PATH"   # php8ts.dll for `cargo test` / `rapira serve`
cargo @CargoArgs
