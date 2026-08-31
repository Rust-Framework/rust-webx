<#
.SYNOPSIS
    Copy ecosystem documentation trees into a target docs/ directory.

.DESCRIPTION
    Shared copy logic used by docbit/publish.ps1 (bundle) and scripts/sync-docs.ps1
    (optional local staging). Sources are sibling repos under the Rust-Framework
    monorepo root; rust-webx handbook always comes from the workspace.

    Mappings:
      {framework}/rust-dix/docs/rust-dix           -> {dest}/rust-dix
      {framework}/rust-ef/docs/rust-ef             -> {dest}/rust-ef
      {framework}/rust-agent-framework/docs        -> {dest}/rust-agent-framework
      {framework}/rust-gpui-rml/docs               -> {dest}/rust-gpui-rml
      {workspace}/docs/rust-webx                   -> {dest}/rust-webx

.PARAMETER DocsDest
    Target docs/ directory (e.g. publish bundle or rust-webx/docs for preview).

.PARAMETER WorkspaceRoot
    rust-webx workspace root (contains docs/rust-webx/).

.PARAMETER FrameworkRoot
    Rust-Framework monorepo root. Defaults to RUST_FRAMEWORK_ROOT or parent of workspace.
#>
function Get-FrameworkRoot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$WorkspaceRoot
    )

    if ($env:RUST_FRAMEWORK_ROOT -and (Test-Path -LiteralPath $env:RUST_FRAMEWORK_ROOT)) {
        return (Resolve-Path -LiteralPath $env:RUST_FRAMEWORK_ROOT).Path
    }

    return (Resolve-Path (Join-Path $WorkspaceRoot '..')).Path
}

function Copy-DocTree {
    param(
        [string]$Source,
        [string]$Dest,
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Source)) {
        Write-Warning "Skip $Label — source not found: $Source"
        return
    }

    $srcFull = (Resolve-Path -LiteralPath $Source).Path
    if (Test-Path -LiteralPath $Dest) {
        $destFull = (Resolve-Path -LiteralPath $Dest).Path
        if ($srcFull -eq $destFull) {
            Write-Warning "Skip $Label — refusing to copy a tree onto itself: $srcFull"
            return
        }
    }

    if (Test-Path -LiteralPath $Dest) {
        Remove-Item -LiteralPath $Dest -Recurse -Force
    }

    New-Item -ItemType Directory -Path $Dest -Force | Out-Null
    Copy-Item -Path (Join-Path $Source '*') -Destination $Dest -Recurse -Force

    # serde_json / some parsers reject UTF-8 BOM; strip from all copied .json/.md.
    $stripped = 0
    Get-ChildItem -LiteralPath $Dest -Recurse -Include *.json, *.md -File -ErrorAction SilentlyContinue | ForEach-Object {
        $bytes = [System.IO.File]::ReadAllBytes($_.FullName)
        if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
            [System.IO.File]::WriteAllBytes($_.FullName, [byte[]]$bytes[3..($bytes.Length - 1)])
            $stripped++
        }
    }
    if ($stripped -gt 0) {
        Write-Host "Stripped UTF-8 BOM from $stripped json/md file(s) under $Dest"
    }

    Write-Host "Copied $Label -> $Dest"
}

function Copy-EcosystemDocs {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string]$DocsDest,

        [Parameter(Mandatory = $true)]
        [string]$WorkspaceRoot,

        [string]$FrameworkRoot = ""
    )

    if ([string]::IsNullOrWhiteSpace($FrameworkRoot)) {
        $FrameworkRoot = Get-FrameworkRoot -WorkspaceRoot $WorkspaceRoot
    } else {
        $FrameworkRoot = (Resolve-Path -LiteralPath $FrameworkRoot).Path
    }

    $WorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot).Path
    New-Item -ItemType Directory -Path $DocsDest -Force | Out-Null

    Write-Host "Framework root: $FrameworkRoot"
    Write-Host "Workspace root: $WorkspaceRoot"
    Write-Host "Docs dest:      $DocsDest"

    Copy-DocTree `
        -Source (Join-Path $FrameworkRoot 'rust-dix\docs\rust-dix') `
        -Dest (Join-Path $DocsDest 'rust-dix') `
        -Label 'rust-dix'

    Copy-DocTree `
        -Source (Join-Path $FrameworkRoot 'rust-ef\docs\rust-ef') `
        -Dest (Join-Path $DocsDest 'rust-ef') `
        -Label 'rust-ef'

    Copy-DocTree `
        -Source (Join-Path $FrameworkRoot 'rust-agent-framework\docs') `
        -Dest (Join-Path $DocsDest 'rust-agent-framework') `
        -Label 'rust-agent-framework'

    Copy-DocTree `
        -Source (Join-Path $FrameworkRoot 'rust-gpui-rml\docs') `
        -Dest (Join-Path $DocsDest 'rust-gpui-rml') `
        -Label 'rust-gpui-rml'

    Copy-DocTree `
        -Source (Join-Path $WorkspaceRoot 'docs\rust-webx') `
        -Dest (Join-Path $DocsDest 'rust-webx') `
        -Label 'rust-webx'

    $logoCopies = @(
        @{
            Src  = Join-Path $FrameworkRoot 'rust-dix\assets\logo.svg'
            Dest = Join-Path $DocsDest 'rust-dix\logo.svg'
        },
        @{
            Src  = Join-Path $FrameworkRoot 'rust-gpui-rml\demo\assets\logo.svg'
            Dest = Join-Path $DocsDest 'rust-gpui-rml\logo.svg'
        }
    )
    foreach ($copy in $logoCopies) {
        if (Test-Path -LiteralPath $copy.Src) {
            $destDir = Split-Path -Parent $copy.Dest
            if (-not (Test-Path -LiteralPath $destDir)) {
                New-Item -ItemType Directory -Path $destDir -Force | Out-Null
            }
            Copy-Item -LiteralPath $copy.Src -Destination $copy.Dest -Force
            Write-Host "Copied logo -> $($copy.Dest)"
        }
    }
}
