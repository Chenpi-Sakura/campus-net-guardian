# 从锁定的 Cargo 依赖及其随包许可证生成离线可读的第三方声明。
[CmdletBinding()]
param(
    [string]$OutputFile = "output\THIRD_PARTY_NOTICES.txt",
    [string]$Target
)
$ErrorActionPreference = "Stop"
$repositoryPath = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location -LiteralPath $repositoryPath
if (-not $Target) {
    $versionDetails = & rustc -vV
    if ($LASTEXITCODE) { throw "无法读取 Rust host triple" }
    $Target = (($versionDetails | Select-String '^host: ').Line -replace '^host: ', '').Trim()
}
if (-not $Target) { throw "无法确定许可证清单对应的目标平台" }
$metadataText = & cargo metadata --locked --format-version 1 --filter-platform $Target
if ($LASTEXITCODE) { throw "读取已锁定的 Cargo 依赖失败" }
$metadata = ($metadataText | Out-String) | ConvertFrom-Json
$workspaceIds = @($metadata.workspace_members)
$packages = @($metadata.packages | Where-Object { $_.id -notin $workspaceIds } | Sort-Object name, version)
# 部分 crate 未随包附带工作区许可证。只补充已核实的固定上游原文，
# 并校验内容摘要；依赖版本变化后必须重新检查，不能猜测许可证内容。
$upstreamLicenses = @{
    'clipboard-win@5.4.3' = @{
        License = 'BSL-1.0'
        Url = 'https://raw.githubusercontent.com/DoumanAsh/clipboard-win/ed27008ae5d1f10cfbb266de039f0fb35fd05a56/LICENSE'
        Sha256 = 'C9BFF75738922193E67FA726FA225535870D2AA1059F91452C411736284AD566'
    }
    'profiling@1.0.18' = @{
        License = 'MIT OR Apache-2.0'
        Url = 'https://raw.githubusercontent.com/aclysma/profiling/8271551172eb6fa4cba47369aedd93790c623df9/LICENSE-MIT'
        Sha256 = 'C8167FDEEED46D3F244D3F85C5BF998CE889343691C32BE2C61A8BC4B5C08333'
    }
    'svg_fmt@0.4.5' = @{
        License = 'MIT/Apache-2.0'
        Url = 'https://raw.githubusercontent.com/nical/rust_debug/93f414cd572b5c2781df93dbdc068ed13443d05a/LICENSE'
        Sha256 = '7E98BED7F100747AAAE7D82AB94FBE6B76FD6B84B09A3AAA413BFF8196D0FBB1'
    }
    'hexf-parse@0.2.1' = @{
        License = 'CC0-1.0'
        Url = 'https://creativecommons.org/publicdomain/zero/1.0/legalcode.txt'
        Sha256 = 'A2010F343487D3F7618AFFE54F789F5487602331C0A8D03F49E9A7C547CF0499'
    }
    'float_next_after@1.0.0' = @{
        License = 'MIT'
        Url = 'https://gitlab.com/bronsonbdevost/next_afterf/-/raw/6f555d52667acda6237f88588a3d1f4e8a840c7e/LICENSE'
        Sha256 = '0604D5817110DACC5C7D9B7DE6B58B962794603D4823C357157BD5DEC4198BC5'
    }
    'lyon@1.0.19' = @{
        License = 'MIT OR Apache-2.0'
        Url = 'https://raw.githubusercontent.com/nical/lyon/56219c95c658d4f39de7b190d98e57d5773ba241/LICENSE-MIT'
        Sha256 = '80F275E90D799911ED3830A7F242A2EF5A4ADE2092FE0AA07BFB2D2CF2F2B95E'
    }
    'lyon_algorithms@1.0.22' = @{
        License = 'MIT OR Apache-2.0'
        Url = 'https://raw.githubusercontent.com/nical/lyon/db98d26a1a90076a7a894262dcb51117eef643c8/LICENSE-MIT'
        Sha256 = '80F275E90D799911ED3830A7F242A2EF5A4ADE2092FE0AA07BFB2D2CF2F2B95E'
    }
    'lyon_geom@1.0.20' = @{
        License = 'MIT OR Apache-2.0'
        Url = 'https://raw.githubusercontent.com/nical/lyon/db98d26a1a90076a7a894262dcb51117eef643c8/LICENSE-MIT'
        Sha256 = '80F275E90D799911ED3830A7F242A2EF5A4ADE2092FE0AA07BFB2D2CF2F2B95E'
    }
    'lyon_path@1.0.19' = @{
        License = 'MIT OR Apache-2.0'
        Url = 'https://raw.githubusercontent.com/nical/lyon/56219c95c658d4f39de7b190d98e57d5773ba241/LICENSE-MIT'
        Sha256 = '80F275E90D799911ED3830A7F242A2EF5A4ADE2092FE0AA07BFB2D2CF2F2B95E'
    }
    'lyon_tessellation@1.0.23' = @{
        License = 'MIT OR Apache-2.0'
        Url = 'https://raw.githubusercontent.com/nical/lyon/db98d26a1a90076a7a894262dcb51117eef643c8/LICENSE-MIT'
        Sha256 = '80F275E90D799911ED3830A7F242A2EF5A4ADE2092FE0AA07BFB2D2CF2F2B95E'
    }
}
function Get-SupplementalLicense($Package) {
    $entry = $upstreamLicenses["$($Package.name)@$($Package.version)"]
    if (-not $entry -or $Package.license -ne $entry.License) {
        throw "依赖 $($Package.name) $($Package.version) 缺少可分发许可证文本，停止打包"
    }
    $cacheDirectory = Join-Path $repositoryPath '.codex_tmp\third-party-licenses'
    [void][System.IO.Directory]::CreateDirectory($cacheDirectory)
    $cachePath = Join-Path $cacheDirectory "$($Package.name)-$($Package.version).txt"
    if (-not (Test-Path -LiteralPath $cachePath -PathType Leaf)) {
        Invoke-WebRequest -Uri $entry.Url -OutFile $cachePath -UseBasicParsing
    }
    if ((Get-FileHash -LiteralPath $cachePath -Algorithm SHA256).Hash -ne $entry.Sha256) {
        throw "依赖 $($Package.name) $($Package.version) 的补充许可证摘要不匹配，停止打包"
    }
    return [pscustomobject]@{ Path = $cachePath; Source = $entry.Url }
}
$icedPackage = $packages | Where-Object { $_.name -eq 'iced' -and $_.repository -eq 'https://github.com/iced-rs/iced' -and $_.license -eq 'MIT' } | Select-Object -First 1
$icedLicense = if ($icedPackage) { Join-Path (Split-Path -Parent $icedPackage.manifest_path) 'LICENSE' } else { $null }
$text = [System.Text.StringBuilder]::new()
[void]$text.AppendLine("CUIT Helper — Third-party notices")
[void]$text.AppendLine("Dependencies are resolved from Cargo.lock. Original license and notice text follows.")
[void]$text.AppendLine("Target: $Target")
[void]$text.AppendLine("This list includes build-time dependencies; it does not imply all listed code is present in the executable.")
foreach ($package in $packages) {
    $packageRoot = Split-Path -Parent $package.manifest_path
    $licenseFiles = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($file in Get-ChildItem -LiteralPath $packageRoot -File -Recurse) {
        if ($file.Name -match '(^|[_-])(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)([._-].*)?$') {
            [void]$licenseFiles.Add($file.FullName)
        }
    }
    if ($package.license_file) {
        $declared = if ([System.IO.Path]::IsPathRooted($package.license_file)) { $package.license_file } else { Join-Path $packageRoot $package.license_file }
        if (Test-Path -LiteralPath $declared -PathType Leaf) { [void]$licenseFiles.Add($declared) }
    }
    if ($package.name -match '^iced_' -and $package.repository -eq 'https://github.com/iced-rs/iced' -and $package.license -eq 'MIT') {
        if (-not $icedLicense -or -not (Test-Path -LiteralPath $icedLicense -PathType Leaf)) {
            throw 'Iced 工作区共享许可证缺失，停止打包'
        }
        [void]$licenseFiles.Add($icedLicense)
    }
    $supplemental = $null
    if (-not $licenseFiles.Count) {
        $supplemental = Get-SupplementalLicense $package
        [void]$licenseFiles.Add($supplemental.Path)
    }
    [void]$text.AppendLine()
    [void]$text.AppendLine("================================================================")
    [void]$text.AppendLine("$($package.name) $($package.version)")
    [void]$text.AppendLine("SPDX: $($package.license)")
    [void]$text.AppendLine("Source: $($package.repository)")
    if ($supplemental) { [void]$text.AppendLine("License text: $($supplemental.Source)") }
    foreach ($filePath in ($licenseFiles | Sort-Object)) {
        [void]$text.AppendLine()
        [void]$text.AppendLine("--- $([System.IO.Path]::GetFileName($filePath)) ---")
        [void]$text.AppendLine([System.IO.File]::ReadAllText($filePath))
    }
}
$destination = [System.IO.Path]::GetFullPath((Join-Path $repositoryPath $OutputFile))
[void][System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($destination))
[System.IO.File]::WriteAllText($destination, $text.ToString(), [System.Text.UTF8Encoding]::new($false))
Write-Host "Collected license texts for $($packages.Count) dependencies: $destination"
