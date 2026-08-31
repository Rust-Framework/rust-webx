<#
.SYNOPSIS
    发布 docbit 站点到指定目录。

.DESCRIPTION
    完整发布流程：
      1. cargo build --release -p docbit-host   （编译 host crate）
      2. 复制 docbit-host.exe 到目标目录
      3. 复制 wwwroot/ 静态资源到目标目录
      4. 复制 appsettings.json 与 appsettings.Production.json
      5. 从 monorepo 源仓库复制 docs/ 生态文档到 bundle
    开发与生产均使用 SQLite（`<app_base>/app.db`）；运行时数据库文件不发布。
    blog-data 已废弃，博客内容已迁移到数据库。

.PARAMETER Destination
    目标发布目录（必需）。如已存在，将被原地覆盖更新。

.PARAMETER SkipBuild
    跳过 cargo build，直接复用已有的 target\release\docbit-host.exe。
    适合仅调整了 wwwroot 静态资源后的快速重发。

.PARAMETER Clean
    发布前清空目标目录（删除后重建），确保无残留旧文件。
    默认为增量覆盖，保留目标目录中的 docs/ 等运行期数据。

.PARAMETER WorkspaceRoot
    workspace 根目录，默认基于脚本位置推断 (..\)。
    仅在跨仓库移动脚本时需要显式指定。

.PARAMETER Production
    生成生产模式启动脚本 run.cmd（设置 APP_ENV=Production 后启动 exe）。
    生产环境通过该脚本启动，框架据此自动加载 appsettings.Production.json overlay。
    未指定此开关时，默认按 Development 启动。

.PARAMETER Linux
    交叉编译 Linux ELF（x86_64-unknown-linux-gnu）并发布 docbit-host（无 .exe 后缀）。
    生成 run.sh 而非 run.cmd（需 -Production）。

.EXAMPLE
    .\publish.ps1 -Destination D:\deploy\docbit
    编译并发布到 D:\deploy\docbit。

.EXAMPLE
    .\publish.ps1 -Destination D:\deploy\docbit -Production
    发布并生成生产启动脚本 run.cmd，双击即以 Production 模式运行。

.EXAMPLE
    .\publish.ps1 -Destination D:\deploy\docbit -SkipBuild -Clean
    使用已编译的 exe 清空并重新发布静态资源。

.NOTES
    输出目录结构：
        <Destination>\
        docbit-host.exe
        appsettings.json
        appsettings.Production.json
        wwwroot\          (admin / assets / pages / index.html ...)
        docs\             (五项目文档，publish 时从源仓库复制)
        run.cmd           (-Production 时生成，设置 APP_ENV=Production 启动)
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Destination,

    [switch]$SkipBuild,

    [switch]$Clean,

    [switch]$Production,

    [switch]$Linux,

    [string]$WorkspaceRoot
)

$ErrorActionPreference = 'Stop'

# ---------- 路径推断 ----------
$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path
if ([string]::IsNullOrWhiteSpace($WorkspaceRoot)) {
    $WorkspaceRoot = Resolve-Path (Join-Path $ScriptDir '..') | Select-Object -ExpandProperty Path
}
$DocbitDir   = $ScriptDir
if ($Linux) {
    $TargetDir = Join-Path $WorkspaceRoot 'target\x86_64-unknown-linux-gnu\release'
    $ExePath   = Join-Path $TargetDir 'docbit-host'
    $ExeName   = 'docbit-host'
} else {
    $TargetDir = Join-Path $WorkspaceRoot 'target\release'
    $ExePath   = Join-Path $TargetDir 'docbit-host.exe'
    $ExeName   = 'docbit-host.exe'
}
$WwwrootSrc  = Join-Path $DocbitDir 'wwwroot'
$AppsettingsBase   = Join-Path $DocbitDir 'appsettings.json'
$AppsettingsProd   = Join-Path $DocbitDir 'appsettings.Production.json'

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
Write-Host "=== docbit publish ===" -ForegroundColor Cyan
Write-Host "WorkspaceRoot : $WorkspaceRoot"
Write-Host "DocbitDir     : $DocbitDir"
Write-Host "Destination   : $Destination"
Write-Host "SkipBuild     : $SkipBuild"
Write-Host "Clean         : $Clean"
Write-Host "Production    : $Production"
Write-Host "Linux         : $Linux"
Write-Host ""

# ---------- 1. 编译 ----------
if (-not $SkipBuild) {
    if ($Linux) {
        $tools = Join-Path $WorkspaceRoot '.tools'
        $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = Join-Path $tools 'zig-link.bat'
        $env:CC_x86_64_unknown_linux_gnu = Join-Path $tools 'zig-cc.bat'
        $env:CXX_x86_64_unknown_linux_gnu = Join-Path $tools 'zig-c++.bat'
        $env:AR_x86_64_unknown_linux_gnu = Join-Path $tools 'zig-ar.bat'
        Write-Host "[1/6] cargo build --release -p docbit-host --target x86_64-unknown-linux-gnu" -ForegroundColor Green
    } else {
        Write-Host "[1/6] cargo build --release -p docbit-host" -ForegroundColor Green
    }
    Push-Location $WorkspaceRoot
    try {
        if ($Linux) {
            & cargo build --release -p docbit-host --target x86_64-unknown-linux-gnu
        } else {
            & cargo build --release -p docbit-host
        }
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build 失败，退出码 $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
} else {
    Write-Host "[1/6] 跳过编译 (-SkipBuild)" -ForegroundColor DarkGray
}

if (-not (Test-Path $ExePath)) {
    throw "未找到编译产物: $ExePath（请去掉 -SkipBuild 重新编译）"
}

# ---------- 2. 复制可执行文件 ----------
Write-Host "[2/6] 复制 $ExeName" -ForegroundColor Green
Copy-Item -Path $ExePath -Destination (Join-Path $Destination $ExeName) -Force

# ---------- 3. 复制 wwwroot ----------
Write-Host "[3/6] 同步 wwwroot/" -ForegroundColor Green
$WwwrootDest = Join-Path $Destination 'wwwroot'
if (Test-Path $WwwrootDest) {
    # 增量覆盖：先清空目标 wwwroot 内容，再复制（保留 wwwroot 目录本身）
    Get-ChildItem -Path $WwwrootDest -Force | Remove-Item -Recurse -Force
} else {
    New-Item -Path $WwwrootDest -ItemType Directory -Force | Out-Null
}

# 使用 robocopy 同步目录（高效、保留结构、支持排除）
# 排除开发期文件
$robocopyArgs = @(
    $WwwrootSrc,
    $WwwrootDest,
    '/E',          # 包含空目录
    '/NFL',        # 不列出每个文件
    '/NDL',        # 不列出目录
    '/NJH',        # 不显示作业头
    '/NP',         # 不显示进度
    '/MT:8',       # 多线程
    '/R:1',        # 重试 1 次
    '/W:1'         # 重试等待 1s
)
& robocopy @robocopyArgs | Out-Null
# robocopy 退出码 0-7 视为成功，>=8 才是错误
if ($LASTEXITCODE -ge 8) {
    throw "robocopy 失败，退出码 $LASTEXITCODE"
}
# 重置 $LASTEXITCODE，避免后续判断受影响
$global:LASTEXITCODE = 0

# ---------- 4. 复制配置文件 ----------
Write-Host "[4/6] 复制配置文件 (appsettings.json + Production)" -ForegroundColor Green
Copy-Item -Path $AppsettingsBase -Destination $Destination -Force
Copy-Item -Path $AppsettingsProd -Destination $Destination -Force

# ---------- 5. 复制生态文档（直接从源仓库） ----------
Write-Host "[5/6] 复制 docs/ 生态文档（源仓库 -> bundle）" -ForegroundColor Green
$CopyDocsScript = Join-Path $WorkspaceRoot 'scripts\copy-ecosystem-docs.ps1'
if (-not (Test-Path $CopyDocsScript)) {
    throw "未找到共享复制脚本: $CopyDocsScript"
}
. $CopyDocsScript

$DocsDest = Join-Path $Destination 'docs'
if (Test-Path $DocsDest) {
    Remove-Item -Path $DocsDest -Recurse -Force
}
Copy-EcosystemDocs -DocsDest $DocsDest -WorkspaceRoot $WorkspaceRoot

# ---------- 6. 生成生产启动脚本 ----------
if ($Production) {
    if ($Linux) {
        Write-Host "[6/6] 生成生产启动脚本 run.sh (APP_ENV=Production)" -ForegroundColor Green
        $runShPath = Join-Path $Destination 'run.sh'
        $runShContent = @(
            '#!/usr/bin/env bash',
            'set -euo pipefail',
            'cd "$(dirname "$0")"',
            'export APP_ENV=Production',
            '# export JWT_SECRET=your-strong-secret-min-32-chars',
            'exec ./docbit-host'
        ) -join "`n"
        [System.IO.File]::WriteAllText($runShPath, $runShContent, [System.Text.UTF8Encoding]::new($false))
    } else {
        Write-Host "[6/6] 生成生产启动脚本 run.cmd (APP_ENV=Production)" -ForegroundColor Green
        $runCmdPath = Join-Path $Destination 'run.cmd'
        # 用 %~dp0 引用脚本所在目录，确保从任意位置启动都能定位 exe
        $runCmdContent = @(
            '@echo off',
            'rem 自动生成：设置 APP_ENV=Production 后启动 docbit-host.exe',
            'rem 框架据此加载 appsettings.Production.json overlay；数据库为同目录 app.db（SQLite）',
            'set APP_ENV=Production',
            'rem set JWT_SECRET=your-strong-secret-min-32-chars',
            '"%~dp0docbit-host.exe"',
            'pause'
        ) -join "`r`n"
        [System.IO.File]::WriteAllText($runCmdPath, $runCmdContent, [System.Text.Encoding]::Default)
    }
} else {
    Write-Host "[6/6] 跳过生产启动脚本 (-Production 未指定)" -ForegroundColor DarkGray
}

# ---------- 摘要 ----------
Write-Host ""
Write-Host "=== 发布完成 ===" -ForegroundColor Cyan
$ExeItem = Get-Item (Join-Path $Destination $ExeName)
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
    if ($Linux) {
        Write-Host "  生产模式: chmod +x run.sh docbit-host && ./run.sh" -ForegroundColor Green
        Write-Host "    => 框架读 APP_ENV=Production，合并 appsettings.Production.json，SQLite 数据库为 app.db"
        Write-Host "    => 部署前请在 run.sh 或环境中设置 JWT_SECRET（≥32 字符）" -ForegroundColor Yellow
    } else {
        Write-Host "  生产模式: 双击 run.cmd（或命令行执行 run.cmd）" -ForegroundColor Green
        Write-Host "    => 框架读 APP_ENV=Production，合并 appsettings.Production.json，SQLite 数据库为 app.db"
        Write-Host "    => 部署前请在 run.cmd 或环境中设置 JWT_SECRET（≥32 字符）" -ForegroundColor Yellow
    }
} else {
    if ($Linux) {
        Write-Host "  开发模式: cd `"$Destination`" && chmod +x docbit-host && ./docbit-host"
        Write-Host "    => APP_ENV 未设置，默认 Development，SQLite 数据库为 app.db"
        Write-Host "  切换生产: 带 -Production -Linux 重新发布生成 run.sh" -ForegroundColor DarkGray
    } else {
        Write-Host "  开发模式: cd `"$Destination`" && .\docbit-host.exe"
        Write-Host "    => APP_ENV 未设置，默认 Development，SQLite 数据库为 app.db"
        Write-Host "  切换生产: 设置 APP_ENV=Production 后再启动 exe，或带 -Production 重新发布生成 run.cmd" -ForegroundColor DarkGray
    }
}
Write-Host ""
