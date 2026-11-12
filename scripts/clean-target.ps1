#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Report or reclaim disk space used by Cargo build artifacts in target/.

.DESCRIPTION
    Cargo never garbage-collects artifacts that belong to an old fingerprint, so
    every full rebuild (toolchain bump, Cargo.lock update, RUSTFLAGS change,
    switching feature sets) leaves its whole universe behind.  On Windows MSVC
    each linked binary also carries a .pdb, which made rust-webx's target/debug
    grow past 100 GB.

    Cargo.toml's "Dev/test build hygiene" profiles keep *new* builds small
    (~3 GB for the whole workspace instead of >100 GB through accumulation).
    When the directory has grown anyway, `-All` runs `cargo clean`; every
    artifact in target/ is regenerable.

    Deliberately NOT offered: "delete files older than N days".  Windows
    `fs::copy` preserves source timestamps, so build-script outputs copied out
    of the cargo registry look old even though Cargo considers them fresh;
    pruning them by age breaks builds (e.g. libsqlite3-sys' OUT_DIR/bindgen.rs).

.PARAMETER Incremental
    Delete target/debug/incremental only.  Safe: it is a pure compile cache.
    Not needed with the committed profile (incremental = false); useful after a
    CARGO_PROFILE_DEV_INCREMENTAL=true session.

.PARAMETER All
    Run `cargo clean` (deletes every build artifact; all of them are
    rebuildable and a full workspace test build takes a few minutes).

.EXAMPLE
    pwsh scripts/clean-target.ps1
    pwsh scripts/clean-target.ps1 -Incremental
    pwsh scripts/clean-target.ps1 -All
#>
[CmdletBinding(DefaultParameterSetName = 'Report')]
param(
    [Parameter(ParameterSetName = 'Incremental')]
    [switch]$Incremental,

    [Parameter(ParameterSetName = 'All')]
    [switch]$All
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path $PSScriptRoot -Parent
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepoRoot 'target' }

function Get-SizeMb([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    [math]::Round($sum / 1MB, 1)
}

function Show-Usage {
    'target: {0} - {1:N2} GB' -f $TargetDir, ((Get-SizeMb $TargetDir) / 1024)
    foreach ($rel in 'debug', 'release', 'x86_64-unknown-linux-gnu', 'doc', 'package') {
        $p = Join-Path $TargetDir $rel
        if (Test-Path -LiteralPath $p) {
            '  {0,-26} {1,10:N1} MB' -f $rel, (Get-SizeMb $p)
        }
    }
    foreach ($rel in 'debug\deps', 'debug\build', 'debug\incremental') {
        $p = Join-Path $TargetDir $rel
        if (Test-Path -LiteralPath $p) {
            '  {0,-26} {1,10:N1} MB' -f $rel, (Get-SizeMb $p)
        }
    }
    $big = Get-ChildItem -LiteralPath $TargetDir -Recurse -File -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Length -ge 50MB } |
        Sort-Object Length -Descending |
        Select-Object -First 10
    if ($big) {
        'largest artifacts (>= 50 MB):'
        $big | ForEach-Object {
            '  {0,8:N0} MB  {1}' -f ($_.Length / 1MB), $_.FullName.Substring($TargetDir.Length + 1)
        }
    }
}

switch ($PSCmdlet.ParameterSetName) {
    'All' {
        Show-Usage
        Push-Location $RepoRoot
        try { & cargo clean } finally { Pop-Location }
        'target after clean: {0:N1} MB' -f (Get-SizeMb $TargetDir)
    }
    'Incremental' {
        $dir = Join-Path $TargetDir 'debug\incremental'
        $beforeMb = Get-SizeMb $TargetDir
        if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
        'reclaimed {0:N1} MB (target now {1:N1} MB)' -f ($beforeMb - (Get-SizeMb $TargetDir)), (Get-SizeMb $TargetDir)
    }
    default { Show-Usage }
}
