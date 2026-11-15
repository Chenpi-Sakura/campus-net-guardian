# 构建可独立运行的 Windows Release 包；只有显式 -Publish 才写入 GitHub。
[CmdletBinding()]
param(
    [string]$Repository = "Chenpi-Sakura/campus-net-guardian",
    [string]$Tag,
    [switch]$PackageOnly,
    [switch]$NoBuild,
    [switch]$Publish,
    [string]$NotesFile,
    [string]$Executable = "target\release\campus-guardian.exe"
)
$ErrorActionPreference = "Stop"
$repositoryPath = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location -LiteralPath $repositoryPath
$manifest = Get-Content -LiteralPath "native\Cargo.toml" -Raw
$versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) { throw "无法读取 Rust 包版本" }
$version = $versionMatch.Groups[1].Value
if (-not $Tag) { $Tag = "v$version" }
if ($Tag -cne "v$version") { throw "Tag $Tag 与 Cargo 版本 v$version 不一致" }
if ($Publish -and $PackageOnly) { throw "-Publish 与 -PackageOnly 不能同时使用" }
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw "GitHub 仓库名称无效" }

if (-not $NoBuild) {
    & cargo build --workspace --release --locked
    if ($LASTEXITCODE) { throw "Rust release 构建失败" }
}
$exePath = (Resolve-Path -LiteralPath $Executable).Path
$binary = [System.IO.BinaryReader]::new([System.IO.File]::OpenRead($exePath))
try {
    if ($binary.BaseStream.Length -lt 64 -or $binary.ReadUInt16() -ne 0x5a4d) { throw "发布文件不是 Windows EXE" }
    $binary.BaseStream.Position = 0x3c
    $peOffset = $binary.ReadUInt32()
    if ($peOffset -gt $binary.BaseStream.Length - 24) { throw "发布文件 PE 头无效" }
    $binary.BaseStream.Position = $peOffset
    if ($binary.ReadUInt32() -ne 0x00004550 -or $binary.ReadUInt16() -ne 0x8664) { throw "发布文件必须是 Windows x64 EXE" }
} finally { $binary.Dispose() }
$exeHash = (Get-FileHash -LiteralPath $exePath -Algorithm SHA256).Hash.ToLowerInvariant()
$memoryReport = Join-Path $repositoryPath "output\memory-comparison.json"
if (Test-Path -LiteralPath $memoryReport) {
    $measurement = Get-Content -LiteralPath $memoryReport -Raw | ConvertFrom-Json
    if ($measurement.rust_sha256 -cne $exeHash) { throw "内存报告对应另一份 EXE，需重新测量或移走过期报告" }
}
$releaseFolder = Join-Path $repositoryPath "output\release"
$packageName = "campus-guardian-$Tag-windows-x64"
$stageFolder = Join-Path $repositoryPath ("output\" + $packageName)
[void][System.IO.Directory]::CreateDirectory($releaseFolder)
[void][System.IO.Directory]::CreateDirectory($stageFolder)
& (Join-Path $PSScriptRoot "third_party_notices.ps1")
if (-not $?) { throw "第三方许可证收集失败" }

# 明确的白名单，绝不复制本地 config.yaml / gui_settings.json / 日志 / HAR。
Copy-Item -LiteralPath $exePath -Destination (Join-Path $stageFolder "campus-guardian.exe") -Force
Copy-Item -LiteralPath "config.example.yaml" -Destination $stageFolder -Force
Copy-Item -LiteralPath "README.md" -Destination $stageFolder -Force
if (-not (Test-Path -LiteralPath "LICENSE")) { throw "缺少 LICENSE，停止发布" }
Copy-Item -LiteralPath "LICENSE" -Destination $stageFolder -Force
Copy-Item -LiteralPath "output\THIRD_PARTY_NOTICES.txt" -Destination $stageFolder -Force
$expectedFiles = @("campus-guardian.exe", "config.example.yaml", "README.md", "LICENSE", "THIRD_PARTY_NOTICES.txt")
if (Test-Path -LiteralPath $memoryReport) {
    Copy-Item -LiteralPath $memoryReport -Destination $stageFolder -Force
    $expectedFiles += "memory-comparison.json"
}
$unexpectedFiles = @(Get-ChildItem -LiteralPath $stageFolder -Force | Where-Object { $_.Name -notin $expectedFiles -or $_.PSIsContainer })
if ($unexpectedFiles.Count) { throw "打包目录含有非白名单文件，停止发布：$($unexpectedFiles.Name -join ', ')" }
$zipPath = Join-Path $releaseFolder "$packageName.zip"
Compress-Archive -LiteralPath @(Get-ChildItem -LiteralPath $stageFolder -File | Select-Object -ExpandProperty FullName) -DestinationPath $zipPath -Force
$standalonePath = Join-Path $releaseFolder "campus-guardian.exe"
Copy-Item -LiteralPath $exePath -Destination $standalonePath -Force
$releaseAssets = @($standalonePath, $zipPath)
if (Test-Path -LiteralPath $memoryReport) {
    $reportAsset = Join-Path $releaseFolder "memory-comparison.json"
    Copy-Item -LiteralPath $memoryReport -Destination $reportAsset -Force
    $releaseAssets += $reportAsset
}
$hashFile = Join-Path $releaseFolder "SHA256SUMS.txt"
$hashLines = foreach ($file in $releaseAssets) {
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([System.IO.Path]::GetFileName($file))"
}
$hashLines | Set-Content -LiteralPath $hashFile -Encoding ascii
Write-Host "Packaged $zipPath"

if ($Publish) {
    # 在任何远端写入前验证目标 tag 确实对应本地已验证的 HEAD。
    $head = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE) { throw "无法读取 HEAD" }
    $tagCommit = (& git rev-parse "$Tag^{commit}" 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -or $tagCommit -cne $head) { throw "本地 Tag 必须指向已验证的 HEAD" }
    $dirty = (& git status --porcelain | Out-String).Trim()
    if ($dirty) { throw "工作树有未提交改动，停止发布" }
    $remoteTag = & gh api "repos/$Repository/git/ref/tags/$Tag" --jq '.object'
    if ($LASTEXITCODE) { throw "无法核对 GitHub 上的 Release Tag" }
    $remoteObject = ($remoteTag | Out-String) | ConvertFrom-Json
    if ($remoteObject.type -eq "tag") {
        $annotated = & gh api "repos/$Repository/git/tags/$($remoteObject.sha)" --jq '.object'
        if ($LASTEXITCODE) { throw "无法核对 GitHub 上的注释 Tag" }
        $remoteObject = ($annotated | Out-String) | ConvertFrom-Json
    }
    if ($remoteObject.type -ne "commit" -or $remoteObject.sha -cne $head) {
        throw "GitHub Tag 与本地已验证的 HEAD 不一致，停止发布"
    }
    $releaseArguments = @("release", "create", $Tag, "--repo", $Repository, "--verify-tag", "--title", "Campus Net Guardian $Tag")
    if ($NotesFile) { $releaseArguments += @("--notes-file", (Resolve-Path -LiteralPath $NotesFile).Path) }
    else { $releaseArguments += "--generate-notes" }
    $releaseArguments += $releaseAssets + @($hashFile)
    & gh @releaseArguments
    if ($LASTEXITCODE) { throw "GitHub Release 发布失败，保留本地包供恢复" }
}
