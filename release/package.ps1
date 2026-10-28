# 锐境 Vivid 发行包打包脚本
#
# 用法：在仓库根目录执行
#     powershell -ExecutionPolicy Bypass -File release\package.ps1
#
# 产物：dist\Vivid-vX.Y.Z.zip（免安装，解压即用）
#
# 这个脚本存在的主要理由是**授权文件不能靠人记**：
# 程序内嵌了 SIL OFL 授权的思源圆体，OFL 要求随分发附上完整许可证与版权声明。
# 少放一个文件就是违规，而人在赶发版的时候最容易漏这种事。
# 所以这里把它做成硬性断言：文件缺失直接中止，不打包。

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

Write-Host ""
Write-Host "=== 锐境 Vivid 打包 ===" -ForegroundColor Cyan
Write-Host ""

# ---------------------------------------------------------------
# 1. 程序不能正在运行，否则 cargo 覆盖不了 exe
# ---------------------------------------------------------------
$running = @(Get-Process | Where-Object { $_.ProcessName -eq "Vivid" })
if ($running.Count -gt 0) {
    Write-Host "[中止] Vivid.exe 正在运行，先关掉它。" -ForegroundColor Red
    Write-Host "       编译时无法覆盖正在运行的 exe。"
    exit 1
}

# ---------------------------------------------------------------
# 2. 版本号取自 workspace 的 Cargo.toml，不手写
#    （手写迟早和 exe 里的版本对不上，而更新检查靠的正是那个版本）
# ---------------------------------------------------------------
$cargo = Get-Content "Cargo.toml" -Raw -Encoding UTF8
if ($cargo -notmatch '(?s)\[workspace\.package\].*?version\s*=\s*"([^"]+)"') {
    Write-Host "[中止] 从 Cargo.toml 里读不出版本号。" -ForegroundColor Red
    exit 1
}
$version = $matches[1]
Write-Host "版本：$version"

# ---------------------------------------------------------------
# 3. 出 release 构建
# ---------------------------------------------------------------
Write-Host ""
Write-Host "编译中…"
cargo build --release -p vivid-gui
if ($LASTEXITCODE -ne 0) {
    Write-Host "[中止] 编译失败。" -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------
# 4. 必需文件清单
#
#    每一项都写了「少了会怎样」。OFL 那两个不是可选项。
# ---------------------------------------------------------------
$required = @(
    @{ Src = "target\release\Vivid.exe";                     Name = "Vivid.exe";        Why = "主程序" }
    @{ Src = "crates\vivid-gui\assets\OFL.txt";              Name = "OFL.txt";          Why = "SIL OFL 要求随分发附上完整许可证。少了就是违规。" }
    @{ Src = "crates\vivid-gui\assets\FONT-LICENSE.md";      Name = "FONT-LICENSE.md";  Why = "内嵌字体的版权与授权声明，同属 OFL 的分发义务。" }
    @{ Src = "release\使用说明.txt";                          Name = "使用说明.txt";      Why = "用户拿到手不知道怎么用。" }
    @{ Src = "LICENSE";                                      Name = "LICENSE";           Why = "本体 GPL-3.0-only 许可证全文。" }
    @{ Src = "NOTICE";                                       Name = "NOTICE";            Why = "原作项目、作者与源码网址，以及保留声明的说明。" }
    @{ Src = "THIRD_PARTY_NOTICES.md";                        Name = "THIRD_PARTY_NOTICES.md"; Why = "第三方组件与素材来源声明。" }
    @{ Src = "crates\vivid-gui\assets\third-party-licenses.json"; Name = "third-party-licenses.json"; Why = "锁定版本的第三方组件许可证全文。" }
)

foreach ($f in $required) {
    if (-not (Test-Path $f.Src)) {
        Write-Host ""
        Write-Host "[中止] 缺少必需文件：$($f.Src)" -ForegroundColor Red
        Write-Host "       $($f.Why)"
        exit 1
    }
}

# ---------------------------------------------------------------
# 5. 组装
# ---------------------------------------------------------------
$stage = "dist\Vivid-v$version"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

foreach ($f in $required) {
    $dst = Join-Path $stage $f.Name
    if ($f.Src -like "*.txt" -or $f.Src -like "*.md") {
        # 文本文件转成带 BOM 的 UTF-8：不带 BOM 的中文在旧版记事本里是乱码，
        # 而用户多半就是用记事本打开使用说明的。
        $text = Get-Content $f.Src -Raw -Encoding UTF8
        $text | Out-File -FilePath $dst -Encoding utf8 -NoNewline
    } else {
        Copy-Item $f.Src $dst
    }
}

# ---------------------------------------------------------------
# 6. 打包
# ---------------------------------------------------------------
$zip = "dist\Vivid-v$version.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$stage\*" -DestinationPath $zip -Force

# ---------------------------------------------------------------
# 7. 复查：把实际打进去的东西列出来，人能一眼核对
# ---------------------------------------------------------------
Write-Host ""
Write-Host "=== 包内实际内容 ===" -ForegroundColor Cyan
Get-ChildItem $stage | ForEach-Object {
    "{0,-20} {1,10:N0} 字节" -f $_.Name, $_.Length
}

$zipSize = (Get-Item $zip).Length / 1MB
Write-Host ""
Write-Host ("完成：{0}  ({1:N1} MB)" -f $zip, $zipSize) -ForegroundColor Green
Write-Host ""
Write-Host "接下来（详见 release\发布流程.md）：" -ForegroundColor Yellow
Write-Host "  1. 到 GitHub 建 Release，tag 用 v$version，附件传上面这个 zip"
Write-Host "  2. 更新公开仓库 main 分支根目录的 update.json，version 改成 $version"
Write-Host "  3. 跑一次 cargo run -p vivid-core --example check_update 确认能读到"
Write-Host ""
