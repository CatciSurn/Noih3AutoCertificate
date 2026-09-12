param(
    [switch]$Test
)

$ErrorActionPreference = 'Stop'
$projectRoot = $PSScriptRoot
$oldLocation = Get-Location
$oldPath = $env:PATH
$oldRustc = $env:RUSTC
$oldRustdoc = $env:RUSTDOC

try {
    Set-Location -LiteralPath $projectRoot
    # Prefer the isolated toolchain when it was prepared for this project.
    # Otherwise use the developer's configured Cargo/MSVC or GNU toolchain.
    $portableBin = Join-Path $projectRoot '.build-tools\rustup\toolchains\1.85.0-x86_64-pc-windows-gnu\bin'
    if (Test-Path -LiteralPath (Join-Path $portableBin 'cargo.exe')) {
        $cargoCommand = Join-Path $portableBin 'cargo.exe'
        $env:RUSTC = Join-Path $portableBin 'rustc.exe'
        $env:RUSTDOC = Join-Path $portableBin 'rustdoc.exe'
        $env:PATH = "$portableBin;$oldPath"
    } else {
        $cargoCommand = (Get-Command cargo -ErrorAction Stop).Source
    }

    if ($Test) {
        & $cargoCommand test --offline --locked
        if ($LASTEXITCODE -ne 0) { throw 'Rust 测试未通过，已停止构建。' }
    }
    & $cargoCommand build --release --offline --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust 编译失败，请确认 Windows 链接器已安装。' }

    $distribution = Join-Path $projectRoot 'dist'
    New-Item -ItemType Directory -Path $distribution -Force | Out-Null
    $executable = Join-Path $projectRoot 'target\release\nioh3-save-manager.exe'
    Copy-Item -LiteralPath $executable -Destination (Join-Path $distribution '双击我.exe') -Force
    Copy-Item -LiteralPath $executable -Destination (Join-Path $projectRoot '双击我.exe') -Force
    foreach ($name in @('HandMakeSave', 'MagicMakeSave', 'CustomSave')) {
        $source = Join-Path $projectRoot $name
        Copy-Item -LiteralPath $source -Destination $distribution -Recurse -Force
    }
    $toolName = 'Nioh3SaveCertificateTool'
    $toolDestination = Join-Path $distribution $toolName
    New-Item -ItemType Directory -Path $toolDestination -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot "$toolName\双击我改签v0.5.exe") -Destination $toolDestination -Force
    # The helper supplies these files itself, so a distribution needs only empty folders.
    foreach ($name in @('你的存档扔里面', '别人存档扔里面')) {
        New-Item -ItemType Directory -Path (Join-Path $toolDestination $name) -Force | Out-Null
    }
    foreach ($name in @('image.png', 'README.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot $name) -Destination $distribution -Force
    }
    Write-Host "构建完成：$distribution\双击我.exe"
    Write-Host '项目根目录的 双击我.exe 也已更新为 Rust 版。'
} finally {
    Set-Location -LiteralPath $oldLocation.Path
    $env:PATH = $oldPath
    $env:RUSTC = $oldRustc
    $env:RUSTDOC = $oldRustdoc
}
