# 对比旧 Python GUI 与 Rust 原生 GUI。所有被测进程使用合成离线状态。
# 指标覆盖整个进程树：WorkingSet64 / PrivateMemorySize64；单位 MiB。
[CmdletBinding()]
param(
    [string]$RustExe = "target\release\campus-guardian.exe",
    [string]$PythonExe = ".venv\Scripts\pythonw.exe",
    [string]$PythonBaseline = ".codex_tmp\memory_baseline.py",
    [string]$BaselineRoot = ".codex_tmp\python-baseline",
    [ValidateRange(3, 120)][int]$WarmupSeconds = 10,
    [ValidateRange(5, 300)][int]$Samples = 20,
    [ValidateRange(200, 5000)][int]$IntervalMilliseconds = 1000,
    [ValidateRange(1, 10)][int]$Repetitions = 3,
    [string]$OutputFile = "output\memory-comparison.json"
)
$ErrorActionPreference = "Stop"
$repository = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location -LiteralPath $repository
$rustPath = (Resolve-Path -LiteralPath $RustExe).Path
$pythonPath = (Resolve-Path -LiteralPath $PythonExe).Path
$baselinePath = (Resolve-Path -LiteralPath $PythonBaseline).Path
$baselineRootPath = (Resolve-Path -LiteralPath $BaselineRoot).Path

function Get-TreeIds([int]$RootId, [datetime]$RootStarted) {
    # 在每次采样时重新枚举，包含延迟创建的子进程。
    $allProcesses = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, CreationDate)
    $rootEntry = $allProcesses | Where-Object { $_.ProcessId -eq $RootId } | Select-Object -First 1
    # PID 会重用，不能把另一进程或旧 ParentProcessId 下的存活进程算成本次子进程。
    if (-not $rootEntry.CreationDate -or [Math]::Abs(($rootEntry.CreationDate.ToUniversalTime() - $RootStarted.ToUniversalTime()).TotalMilliseconds) -gt 1) {
        return @()
    }
    $treeIds = [System.Collections.Generic.HashSet[int]]::new()
    [void]$treeIds.Add($RootId)
    $creationDates = @{ $RootId = $rootEntry.CreationDate }
    do {
        $added = $false
        foreach ($entry in $allProcesses) {
            if ($treeIds.Contains([int]$entry.ParentProcessId) -and $entry.CreationDate -and $entry.CreationDate -ge $creationDates[[int]$entry.ParentProcessId] -and $treeIds.Add([int]$entry.ProcessId)) {
                $creationDates[[int]$entry.ProcessId] = $entry.CreationDate
                $added = $true
            }
        }
    } while ($added)
    return @($treeIds)
}

function Get-Median($Values) {
    $ordered = @($Values | Sort-Object)
    $middle = [int][Math]::Floor($ordered.Count / 2)
    if ($ordered.Count % 2) { return [double]$ordered[$middle] }
    return ([double]$ordered[$middle - 1] + [double]$ordered[$middle]) / 2
}

$runs = [System.Collections.Generic.List[object]]::new()
foreach ($scenario in @("visible", "tray")) {
    for ($repetition = 1; $repetition -le $Repetitions; $repetition++) {
        # 交替先后顺序，减轻固定运行顺序造成的系统状态偏差。
        $implementations = if ($repetition % 2) { @("python", "rust") } else { @("rust", "python") }
        foreach ($implementation in $implementations) {
            $lifetime = $WarmupSeconds + [int][Math]::Ceiling($Samples * $IntervalMilliseconds / 1000) + 60
            $readyPath = Join-Path ([System.IO.Path]::GetTempPath()) ("cng-ready-" + [guid]::NewGuid().ToString("N") + ".json")
            if ($implementation -eq "python") {
                $arguments = @(('"' + $baselinePath + '"'), "--scenario", $scenario, "--lifetime", [string]$lifetime, "--ready-file", ('"' + $readyPath + '"'))
                $program = $pythonPath
            } else {
                $arguments = @($(if ($scenario -eq "visible") { "--benchmark-ui" } else { "--benchmark-tray" }))
                $program = $rustPath
            }
            $process = Start-Process -FilePath $program -ArgumentList $arguments -WorkingDirectory $repository -WindowStyle Hidden -PassThru
            $processStarted = $process.StartTime
            try {
                Start-Sleep -Seconds $WarmupSeconds
                $process.Refresh()
                if ($process.HasExited) { throw "$implementation 在采样前退出，退出码 $($process.ExitCode)" }
                if ($implementation -eq "python" -and -not (Test-Path -LiteralPath $readyPath)) {
                    throw "Python GUI 未进入就绪状态，不能将启动失败作为低内存结果"
                }
                $uiProcess = $process
                if ($implementation -eq "python") {
                    $ready = Get-Content -LiteralPath $readyPath -Raw | ConvertFrom-Json
                    if (-not $ready.offline -or $ready.scenario -ne $scenario -or $ready.visible -ne ($scenario -eq "visible")) {
                        throw "Python 基线未进入指定的离线显示状态"
                    }
                    $uiProcess = Get-Process -Id $ready.pid -ErrorAction Stop
                }
                $measurements = [System.Collections.Generic.List[object]]::new()
                for ($sample = 0; $sample -lt $Samples; $sample++) {
                    $process.Refresh()
                    if ($process.HasExited) { throw "$implementation 在采样中提前退出" }
                    $uiProcess.Refresh()
                    $isVisible = $uiProcess.MainWindowHandle -ne [IntPtr]::Zero
                    if ($uiProcess.HasExited -or $isVisible -ne ($scenario -eq "visible")) {
                        throw "$implementation 的窗口状态与 $scenario 不一致（第 $repetition 轮，样本 $sample），停止采样；不会自动恢复用户关闭的窗口"
                    }
                    $treeIds = @(Get-TreeIds $process.Id $processStarted)
                    if (-not $treeIds.Count) { throw "无法可靠确认被测进程身份，停止采样" }
                    $tree = @(Get-Process -Id $treeIds -ErrorAction SilentlyContinue)
                    $workingBytes = ($tree | Measure-Object -Property WorkingSet64 -Sum).Sum
                    $privateBytes = ($tree | Measure-Object -Property PrivateMemorySize64 -Sum).Sum
                    $measurements.Add([ordered]@{
                        sample = $sample; process_count = $tree.Count
                        working_set_bytes = [long]$workingBytes; private_bytes = [long]$privateBytes
                    })
                    Start-Sleep -Milliseconds $IntervalMilliseconds
                }
                $run = [ordered]@{
                    implementation = $implementation; scenario = $scenario; repetition = $repetition
                    working_set_median_mib = [Math]::Round((Get-Median $measurements.working_set_bytes) / 1MB, 3)
                    working_set_peak_mib = [Math]::Round(($measurements.working_set_bytes | Measure-Object -Maximum).Maximum / 1MB, 3)
                    private_median_mib = [Math]::Round((Get-Median $measurements.private_bytes) / 1MB, 3)
                    private_peak_mib = [Math]::Round(($measurements.private_bytes | Measure-Object -Maximum).Maximum / 1MB, 3)
                    samples = @($measurements)
                }
                $runs.Add($run)
                Write-Host "$implementation $scenario #${repetition}: private=$($run.private_median_mib) MiB, working_set=$($run.working_set_median_mib) MiB"
            } finally {
                foreach ($treeId in @(Get-TreeIds $process.Id $processStarted)) {
                    Stop-Process -Id $treeId -ErrorAction SilentlyContinue
                }
                Remove-Item -LiteralPath $readyPath -ErrorAction SilentlyContinue
            }
        }
    }
}
$summary = foreach ($scenario in @("visible", "tray")) {
    $pythonRuns = @($runs | Where-Object { $_.scenario -eq $scenario -and $_.implementation -eq "python" })
    $rustRuns = @($runs | Where-Object { $_.scenario -eq $scenario -and $_.implementation -eq "rust" })
    $pythonPrivate = Get-Median $pythonRuns.private_median_mib
    $rustPrivate = Get-Median $rustRuns.private_median_mib
    [ordered]@{
        scenario = $scenario
        python_private_median_mib = $pythonPrivate
        rust_private_median_mib = $rustPrivate
        private_reduction_percent = [Math]::Round(100 * (1 - $rustPrivate / $pythonPrivate), 2)
        python_working_set_median_mib = Get-Median $pythonRuns.working_set_median_mib
        rust_working_set_median_mib = Get-Median $rustRuns.working_set_median_mib
    }
}
$result = [ordered]@{
    measured_at = [DateTimeOffset]::Now.ToString("o")
    os = [Environment]::OSVersion.VersionString
    architecture = $env:PROCESSOR_ARCHITECTURE
    rust_sha256 = (Get-FileHash -LiteralPath $rustPath -Algorithm SHA256).Hash.ToLowerInvariant()
    python_version = (& $pythonPath.Replace("pythonw.exe", "python.exe") --version 2>&1 | Out-String).Trim()
    baseline_gui_sha256 = (Get-FileHash -LiteralPath (Join-Path $baselineRootPath "src\campus_guardian\gui.py") -Algorithm SHA256).Hash.ToLowerInvariant()
    baseline_logo_sha256 = (Get-FileHash -LiteralPath (Join-Path $baselineRootPath "logo.png") -Algorithm SHA256).Hash.ToLowerInvariant()
    methodology = "Synthetic offline state; real UI/tray; each scenario is independently launched with panel visible or start-hidden to tray; complete process tree; sequential alternating order; private bytes are committed memory; MiB=1048576 bytes. No real network/authentication or startup peak measurement."
    warmup_seconds = $WarmupSeconds; samples_per_run = $Samples
    interval_milliseconds = $IntervalMilliseconds; repetitions = $Repetitions
    summary = @($summary); runs = @($runs)
}
$destination = [System.IO.Path]::GetFullPath((Join-Path $repository $OutputFile))
[void][System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($destination))
$result | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $destination -Encoding utf8
Write-Host "Saved $destination"
