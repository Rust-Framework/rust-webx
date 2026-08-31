<#
.SYNOPSIS
    Stage ecosystem docs under rust-webx/docs/ for local preview (optional).

.DESCRIPTION
    Thin wrapper around Copy-EcosystemDocs. Use this only when you want a local
    mirror under rust-webx/docs/ (e.g. testing standalone layout). Daily monorepo
    development does NOT need this — DocService reads sibling repos live.

    For production bundles, use docbit/publish.ps1 which copies directly from
    source repos at publish time.

.EXAMPLE
    .\scripts\sync-docs.ps1
    Stage all five doc trees under rust-webx/docs/.
#>
[CmdletBinding()]
param(
    [string]$FrameworkRoot = "",
    [string]$DocsDest = ""
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $scriptDir) { $scriptDir = $PSScriptRoot }

. (Join-Path $scriptDir 'copy-ecosystem-docs.ps1')

$workspaceRoot = (Resolve-Path (Join-Path $scriptDir '..')).Path
if (-not $DocsDest) {
    $DocsDest = Join-Path $workspaceRoot 'docs'
}

$params = @{
    DocsDest      = $DocsDest
    WorkspaceRoot = $workspaceRoot
}
if ($FrameworkRoot) {
    $params['FrameworkRoot'] = $FrameworkRoot
}

Copy-EcosystemDocs @params
Write-Host 'Done. Run: cargo run -p docbit-host'
