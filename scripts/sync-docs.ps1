<#
.SYNOPSIS
    Copy ecosystem docs into rust-webx/docs/ for standalone deploy bundles.

.DESCRIPTION
    OPTIONAL — only needed when publishing a standalone Linux bundle without
    the full Rust-Framework monorepo checkout.

    During monorepo development, DocService reads sibling repo docs live via
    runtime path resolution (see docbit/README.md). Run this script before
    `docbit/publish.ps1` so the bundle includes a self-contained docs/ mirror.

    Source layout:
      rust-dix/docs/rust-dix/           -> docs/rust-dix/
      rust-ef/docs/rust-ef/             -> docs/rust-ef/
      rust-webx/docs/rust-webx/         -> (in place, skip)
      rust-agent-framework/docs/        -> docs/rust-agent-framework/
      rust-gpui-rml/docs/               -> docs/rust-gpui-rml/

.EXAMPLE
    .\scripts\sync-docs.ps1
    Sync all five project doc trees.
#>
[CmdletBinding()]
param(
    [string]$FrameworkRoot = "",
    [string]$DocsDest = ""
)

$ErrorActionPreference = "Stop"

if (-not $FrameworkRoot) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    if (-not $scriptDir) { $scriptDir = $PSScriptRoot }
    $FrameworkRoot = (Resolve-Path (Join-Path $scriptDir "..\..")).Path
}
if (-not $DocsDest) {
    $DocsDest = Join-Path (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)) "docs"
    if (-not (Test-Path $DocsDest)) {
        $DocsDest = Join-Path (Resolve-Path (Join-Path $FrameworkRoot "rust-webx")).Path "docs"
    }
}

function Sync-DocTree {
    param(
        [string]$Source,
        [string]$Dest,
        [string]$Label
    )
    if (-not (Test-Path $Source)) {
        Write-Warning "Skip $Label — source not found: $Source"
        return
    }
    if (Test-Path $Dest) {
        Remove-Item -Recurse -Force $Dest
    }
    New-Item -ItemType Directory -Path $Dest -Force | Out-Null
    Copy-Item -Path (Join-Path $Source "*") -Destination $Dest -Recurse -Force
    Write-Host "Synced $Label -> $Dest"
}

Write-Host "Framework root: $FrameworkRoot"
Write-Host "Docs dest:      $DocsDest"

Sync-DocTree `
    -Source (Join-Path $FrameworkRoot "rust-dix\docs\rust-dix") `
    -Dest (Join-Path $DocsDest "rust-dix") `
    -Label "rust-dix"

Sync-DocTree `
    -Source (Join-Path $FrameworkRoot "rust-ef\docs\rust-ef") `
    -Dest (Join-Path $DocsDest "rust-ef") `
    -Label "rust-ef"

Sync-DocTree `
    -Source (Join-Path $FrameworkRoot "rust-agent-framework\docs") `
    -Dest (Join-Path $DocsDest "rust-agent-framework") `
    -Label "rust-agent-framework"

Sync-DocTree `
    -Source (Join-Path $FrameworkRoot "rust-gpui-rml\docs") `
    -Dest (Join-Path $DocsDest "rust-gpui-rml") `
    -Label "rust-gpui-rml"

# rust-gpui-rml docs live at docs/ — no copy needed
$webxDocs = Join-Path $DocsDest "rust-webx"
if (Test-Path $webxDocs) {
    Write-Host "rust-webx docs already present at $webxDocs"
} else {
    Write-Warning "rust-webx docs missing at $webxDocs"
}

# Copy logos into doc folders when available (for sync_portfolio_assets)
$logoCopies = @(
    @{ Src = Join-Path $FrameworkRoot "rust-dix\assets\logo.svg"; Dest = Join-Path $DocsDest "rust-dix\logo.svg" },
    @{ Src = Join-Path $FrameworkRoot "rust-gpui-rml\demo\assets\logo.svg"; Dest = Join-Path $DocsDest "rust-gpui-rml\logo.svg" }
)
foreach ($copy in $logoCopies) {
    if (Test-Path $copy.Src) {
        Copy-Item $copy.Src $copy.Dest -Force
        Write-Host "Copied logo -> $($copy.Dest)"
    }
}

Write-Host "Done. Run: cargo run -p docbit-host"
