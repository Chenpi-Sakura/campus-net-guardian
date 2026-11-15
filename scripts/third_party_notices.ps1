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
$text = [System.Text.StringBuilder]::new()
[void]$text.AppendLine("Campus Net Guardian — Third-party notices")
[void]$text.AppendLine("Dependencies are resolved from Cargo.lock. Original license and notice text follows.")
[void]$text.AppendLine("Target: $Target")
[void]$text.AppendLine("This list includes build-time dependencies; it does not imply all listed code is present in the executable.")
foreach ($package in $packages) {
    $packageRoot = Split-Path -Parent $package.manifest_path
    $licenseFiles = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($file in Get-ChildItem -LiteralPath $packageRoot -File) {
        if ($file.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)([._-].*)?$') {
            [void]$licenseFiles.Add($file.FullName)
        }
    }
    if ($package.license_file) {
        $declared = if ([System.IO.Path]::IsPathRooted($package.license_file)) { $package.license_file } else { Join-Path $packageRoot $package.license_file }
        if (Test-Path -LiteralPath $declared -PathType Leaf) { [void]$licenseFiles.Add($declared) }
    }
    if (-not $licenseFiles.Count) { throw "依赖 $($package.name) $($package.version) 缺少可分发许可证文本，停止打包" }
    [void]$text.AppendLine()
    [void]$text.AppendLine("================================================================")
    [void]$text.AppendLine("$($package.name) $($package.version)")
    [void]$text.AppendLine("SPDX: $($package.license)")
    [void]$text.AppendLine("Source: $($package.repository)")
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
