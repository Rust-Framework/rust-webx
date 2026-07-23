<#
.SYNOPSIS
    发布 dmbit 站点到指定目录。

.DESCRIPTION
    完整发布流程：
      1. cargo build --release -p dmbit-host   （编译 host crate）
      2. 复制 dmbit-host.exe 到目标目录
      3. 复制 wwwroot/ 静态资源到目标目录
      4. 复制 appsettings.json 与 appsettings.Production.json
    生产环境使用 MySQL，开发期 SQLite 数据库文件 (app.db*) 不发布。

.PARAMETER Destination
    目标发布目录（必需）。如已存在，将被原地覆盖更新。

.PARAMETER SkipBuild
    跳过 cargo build，直接复用已有的 target\release\dmbit-host.exe。
    适合仅调整了 wwwroot 静态资源后的快速重发。

.PARAMETER Clean
    发布前清空目标目录（删除后重建），确保无残留旧文件。
    默认为增量覆盖。

.PARAMETER WorkspaceRoot
    workspace 根目录，默认基于脚本位置推断 (..\)。

.PARAMETER Production
    生成生产模式启动脚本 run.cmd（设置 APP_ENV=Production 与 DATABASE_URL 后启动 exe）。
    生产环境通过该脚本启动，框架据此自动加载 appsettings.Production.json overlay，
    并通过 DATABASE_URL 环境变量连接 MySQL。未指定此开关时，默认按 Development 启动（SQLite）。

.EXAMPLE
    .\publish.ps1 -Destination D:\deploy\dmbit
    编译并发布到 D:\deploy\dmbit。

.EXAMPLE
    .\publish.ps1 -Destination D:\deploy\dmbit -Production
    发布并生成生产启动脚本 run.cmd，双击即以 Production 模式运行。

.EXAMPLE
    .\publish.ps1 -Destination D:\deploy\dmbit -SkipBuild -Clean
    使用已编译的 exe 清空并重新发布静态资源。

.NOTES
    输出目录结构：
        <Destination>\
        dmbit-host.exe
        appsettings.json
        appsettings.Production.json
        wwwroot\          (admin / assets / index.html ...)
        run.cmd           (-Production 时生成，设置 APP_ENV=Production 启动)
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Destination,

    [switch]$SkipBuild,

    [switch]$Clean,

    [switch]$Production,

    [string]$WorkspaceRoot
)

$ErrorActionPreference = 'Stop'

# ---------- 路径推断 ----------
$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path
if ([string]::IsNullOrWhiteSpace($WorkspaceRoot)) {
    $WorkspaceRoot = Resolve-Path (Join-Path $ScriptDir '..') | Select-Object -ExpandProperty Path
}
$DmbitDir    = $ScriptDir
$TargetDir   = Join-Path $WorkspaceRoot 'target\release'
$ExePath     = Join-Path $TargetDir 'dmbit-host.exe'
$WwwrootSrc  = Join-Path $DmbitDir 'wwwroot'
$AppsettingsBase   = Join-Path $DmbitDir 'appsettings.json'
$AppsettingsProd   = Join-Path $DmbitDir 'appsettings.Production.json'

# ---------- 前置校验 ----------
if (-not (Test-Path $WwwrootSrc)) {
    throw "未找到 wwwroot 目录: $WwwrootSrc"
}
if (-not (Test-Path $AppsettingsBase)) {
    throw "未找到 appsettings.json: $AppsettingsBase"
}
if (-not (Test-Path $AppsettingsProd)) {
    throw "未找到 appsettings.Production.json: $AppsettingsProd"
}

# ---------- 目标目录处理 ----------
if ($Clean -and (Test-Path $Destination)) {
    Write-Host "[Clean] 清空目标目录: $Destination" -ForegroundColor Yellow
    Remove-Item -Path $Destination -Recurse -Force
}
if (-not (Test-Path $Destination)) {
    New-Item -Path $Destination -ItemType Directory -Force | Out-Null
}
$Destination = Resolve-Path $Destination | Select-Object -ExpandProperty Path

Write-Host ""
Write-Host "=== dmbit publish ===" -ForegroundColor Cyan
Write-Host "WorkspaceRoot : $WorkspaceRoot"
Write-Host "DmbitDir       : $DmbitDir"
Write-Host "Destination   : $Destination"
Write-Host "SkipBuild     : $SkipBuild"
Write-Host "Clean         : $Clean"
Write-Host "Production    : $Production"
Write-Host ""

# ---------- 1. 编译 ----------
if (-not $SkipBuild) {
    Write-Host "[1/5] cargo build --release -p dmbit-host" -ForegroundColor Green
    Push-Location $WorkspaceRoot
    try {
        & cargo build --release -p dmbit-host
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build 失败，退出码 $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
} else {
    Write-Host "[1/5] 跳过编译 (-SkipBuild)" -ForegroundColor DarkGray
}

if (-not (Test-Path $ExePath)) {
    throw "未找到编译产物: $ExePath（请去掉 -SkipBuild 重新编译）"
}

# ---------- 2. 复制可执行文件 ----------
Write-Host "[2/5] 复制 dmbit-host.exe" -ForegroundColor Green
Copy-Item -Path $ExePath -Destination $Destination -Force

# ---------- 3. 复制 wwwroot ----------
Write-Host "[3/5] 同步 wwwroot/" -ForegroundColor Green
$WwwrootDest = Join-Path $Destination 'wwwroot'
if (Test-Path $WwwrootDest) {
    Get-ChildItem -Path $WwwrootDest -Force | Remove-Item -Recurse -Force
} else {
    New-Item -Path $WwwrootDest -ItemType Directory -Force | Out-Null
}

$robocopyArgs = @(
    $WwwrootSrc,
    $WwwrootDest,
    '/E',
    '/NFL',
    '/NDL',
    '/NJH',
    '/NP',
    '/MT:8',
    '/R:1',
    '/W:1'
)
& robocopy @robocopyArgs | Out-Null
if ($LASTEXITCODE -ge 8) {
    throw "robocopy 失败，退出码 $LASTEXITCODE"
}
$global:LASTEXITCODE = 0

# ---------- 4. 复制配置文件 ----------
Write-Host "[4/5] 复制配置文件 (appsettings.json + Production)" -ForegroundColor Green
Copy-Item -Path $AppsettingsBase -Destination $Destination -Force
Copy-Item -Path $AppsettingsProd -Destination $Destination -Force

# ---------- 5. 生成生产启动脚本 ----------
if ($Production) {
    Write-Host "[5/5] 生成生产启动脚本 run.cmd (APP_ENV=Production)" -ForegroundColor Green
    $runCmdPath = Join-Path $Destination 'run.cmd'
    $runCmdContent = @(
        '@echo off',
        'rem 自动生成：设置 APP_ENV=Production 后启动 dmbit-host.exe',
        'rem SQLite 数据库文件 app.db 位于 exe 同级目录',
        'set APP_ENV=Production',
        'rem set JWT_SECRET=your-strong-secret-min-32-chars',
        '"%~dp0dmbit-host.exe"',
        'pause'
    ) -join "`r`n"
    [System.IO.File]::WriteAllText($runCmdPath, $runCmdContent, [System.Text.Encoding]::Default)
} else {
    Write-Host "[5/5] 跳过生产启动脚本 (-Production 未指定)" -ForegroundColor DarkGray
}

# ---------- 摘要 ----------
Write-Host ""
Write-Host "=== 发布完成 ===" -ForegroundColor Cyan
$ExeItem = Get-Item (Join-Path $Destination 'dmbit-host.exe')
$WwwrootSize = (Get-ChildItem -Path $WwwrootDest -Recurse -File | Measure-Object -Property Length -Sum).Sum
Write-Host ("exe      : {0} ({1:N0} bytes)" -f $ExeItem.Name, $ExeItem.Length)
Write-Host ("wwwroot  : {0:N0} bytes" -f $WwwrootSize)
Write-Host ""
Write-Host "目标目录结构:" -ForegroundColor DarkGray
Get-ChildItem -Path $Destination | ForEach-Object {
    $tag = if ($_.PSIsContainer) { '[DIR] ' } else { '      ' }
    Write-Host "  $tag$($_.Name)" -ForegroundColor DarkGray
}
Write-Host ""
Write-Host "运行方式:" -ForegroundColor Yellow
if ($Production) {
    Write-Host "  生产模式: 双击 run.cmd（或命令行执行 run.cmd）" -ForegroundColor Green
    Write-Host "    => 框架读 APP_ENV=Production，合并 appsettings.Production.json，使用 SQLite (app.db)"
    Write-Host "    => 生产环境需在 run.cmd 中设置 JWT_SECRET（min 32 chars）" -ForegroundColor Yellow
} else {
    Write-Host "  开发模式: cd `"$Destination`" && .\dmbit-host.exe"
    Write-Host "    => APP_ENV 未设置，默认 Development，使用 SQLite"
    Write-Host "  切换生产: 设置 APP_ENV=Production 后再启动 exe，或带 -Production 重新发布生成 run.cmd" -ForegroundColor DarkGray
}
Write-Host ""
