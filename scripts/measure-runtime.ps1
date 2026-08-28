[CmdletBinding()]
param(
  [string]$Executable = "target\release\code-hangar-desktop.exe",
  [Parameter(Mandatory = $true)][string]$EvidenceDir,
  [ValidateRange(1, 10)][int]$Runs = 3,
  [ValidateRange(1, 120)][int]$WindowTimeoutSeconds = 30,
  [ValidateRange(250, 10000)][int]$StableDelayMilliseconds = 1500,
  [ValidateRange(1000, 120000)][int]$MaxResponsiveMilliseconds = 15000,
  [ValidateRange(64, 4096)][int]$MaxTreePrivateMiB = 768
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance"))
$EvidenceDir = [System.IO.Path]::GetFullPath($EvidenceDir)
$allowedEvidencePrefix = $acceptanceRoot.TrimEnd("\") + "\"
if (-not $EvidenceDir.StartsWith($allowedEvidencePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "EvidenceDir must stay under $acceptanceRoot"
}
New-Item -ItemType Directory -Path $EvidenceDir -Force | Out-Null

$expectedExecutable = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "target\release\code-hangar-desktop.exe"))
$Executable = if ([System.IO.Path]::IsPathRooted($Executable)) {
  [System.IO.Path]::GetFullPath($Executable)
} else {
  [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Executable))
}
if (-not $Executable.Equals($expectedExecutable, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Runtime acceptance only permits the repository release executable: $expectedExecutable"
}
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
  throw "Release executable not found: $Executable"
}

$existing = @(Get-Process -Name "code-hangar-desktop" -ErrorAction SilentlyContinue)
if ($existing.Count -gt 0) {
  $ids = ($existing | ForEach-Object { $_.Id }) -join ", "
  throw "Close the existing Code Hangar process before measuring. Existing PID(s): $ids. The benchmark will not stop them."
}

function Get-Median {
  param([double[]]$Values)
  $sorted = @($Values | Sort-Object)
  if ($sorted.Count -eq 0) { return $null }
  $middle = [int][Math]::Floor($sorted.Count / 2)
  if ($sorted.Count % 2 -eq 1) { return [double]$sorted[$middle] }
  return ([double]$sorted[$middle - 1] + [double]$sorted[$middle]) / 2
}

function Get-ProcessTreeSnapshot {
  param([int]$RootId)

  $inventory = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name)
  $ids = [System.Collections.Generic.HashSet[int]]::new()
  [void]$ids.Add($RootId)
  $added = $true
  while ($added) {
    $added = $false
    foreach ($item in $inventory) {
      if ($ids.Contains([int]$item.ParentProcessId) -and $ids.Add([int]$item.ProcessId)) {
        $added = $true
      }
    }
  }

  $snapshots = [System.Collections.Generic.List[object]]::new()
  foreach ($id in $ids) {
    $process = Get-Process -Id $id -ErrorAction SilentlyContinue
    if ($null -ne $process) {
      $process.Refresh()
      $snapshots.Add([pscustomobject]@{
        pid             = $process.Id
        name            = $process.ProcessName
        workingSetBytes = [long]$process.WorkingSet64
        privateBytes    = [long]$process.PrivateMemorySize64
      })
    }
  }
  return @($snapshots | Sort-Object pid)
}

$results = [System.Collections.Generic.List[object]]::new()
for ($run = 1; $run -le $Runs; $run++) {
  $profileRoot = Join-Path $EvidenceDir ("runtime-profile-{0}" -f $run)
  $roaming = Join-Path $profileRoot "AppData\Roaming"
  $local = Join-Path $profileRoot "AppData\Local"
  $codexHome = Join-Path $profileRoot ".codex"
  New-Item -ItemType Directory -Path $roaming, $local, $codexHome -Force | Out-Null

  $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $Executable
  $startInfo.WorkingDirectory = $repoRoot
  $startInfo.UseShellExecute = $false
  $startInfo.Environment["APPDATA"] = $roaming
  $startInfo.Environment["LOCALAPPDATA"] = $local
  $startInfo.Environment["USERPROFILE"] = $profileRoot
  $startInfo.Environment["HOME"] = $profileRoot
  $startInfo.Environment["CODEX_HOME"] = $codexHome
  [void]$startInfo.Environment.Remove("CODEHANGAR_ENABLE_FINAL_REMOVE")

  $process = $null
  $timer = [System.Diagnostics.Stopwatch]::StartNew()
  $responsiveMs = $null
  $tree = @()
  $status = "PASS"
  $errorMessage = $null
  $closeRequested = $false
  $closedGracefully = $false
  $forcedCleanup = $false
  try {
    $process = [System.Diagnostics.Process]::Start($startInfo)
    if ($null -eq $process) { throw "Process start returned no process handle." }

    while ($timer.Elapsed.TotalSeconds -lt $WindowTimeoutSeconds) {
      $process.Refresh()
      if ($process.HasExited) {
        throw "Process exited with code $($process.ExitCode) before showing a responsive window."
      }
      if ($process.MainWindowHandle -ne [IntPtr]::Zero -and $process.Responding) {
        $responsiveMs = [int]$timer.ElapsedMilliseconds
        break
      }
      Start-Sleep -Milliseconds 25
    }
    if ($null -eq $responsiveMs) {
      throw "No responsive main window appeared within $WindowTimeoutSeconds seconds."
    }

    Start-Sleep -Milliseconds $StableDelayMilliseconds
    $process.Refresh()
    if ($process.HasExited) { throw "Process exited before the stable memory sample." }
    $tree = @(Get-ProcessTreeSnapshot -RootId $process.Id)
    if ($tree.Count -eq 0) { throw "The process tree was empty at the stable sample." }
  } catch {
    $status = "FAIL"
    $errorMessage = $_.Exception.Message
  } finally {
    $timer.Stop()
    if ($null -ne $process) {
      $process.Refresh()
      if (-not $process.HasExited) {
        $closeRequested = $process.CloseMainWindow()
        if ($closeRequested) {
          $closedGracefully = $process.WaitForExit(10000)
        }
        if (-not $process.HasExited) {
          Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
          [void]$process.WaitForExit(5000)
          $forcedCleanup = $true
        }
      } else {
        $closedGracefully = $true
      }
    }
  }

  if ($forcedCleanup -and $status -eq "PASS") {
    $status = "FAIL"
    $errorMessage = "The application did not exit after CloseMainWindow and required exact-PID cleanup."
  }
  $treeWorkingSet = [long](($tree | Measure-Object -Property workingSetBytes -Sum).Sum)
  $treePrivate = [long](($tree | Measure-Object -Property privateBytes -Sum).Sum)
  $results.Add([pscustomobject]@{
    run                 = $run
    status              = $status
    error               = $errorMessage
    pid                 = if ($null -ne $process) { $process.Id } else { $null }
    responsiveWindowMs  = $responsiveMs
    sampleDelayMs       = $StableDelayMilliseconds
    treeWorkingSetBytes = $treeWorkingSet
    treePrivateBytes    = $treePrivate
    processCount        = $tree.Count
    processes           = $tree
    closeRequested      = $closeRequested
    closedGracefully    = $closedGracefully
    forcedCleanup       = $forcedCleanup
    profile             = [System.IO.Path]::GetRelativePath($repoRoot, $profileRoot)
  })
}

$passing = @($results | Where-Object status -eq "PASS")
$responsiveValues = @($passing | ForEach-Object { [double]$_.responsiveWindowMs })
$privateValues = @($passing | ForEach-Object { [double]$_.treePrivateBytes })
$workingSetValues = @($passing | ForEach-Object { [double]$_.treeWorkingSetBytes })
$thresholdFailures = [System.Collections.Generic.List[string]]::new()
if ($passing.Count -ne $Runs) {
  $thresholdFailures.Add("Only $($passing.Count) of $Runs runtime measurements completed cleanly.")
}
if ($responsiveValues.Count -gt 0 -and ($responsiveValues | Measure-Object -Maximum).Maximum -gt $MaxResponsiveMilliseconds) {
  $thresholdFailures.Add("Responsive-window maximum exceeded ${MaxResponsiveMilliseconds}ms.")
}
$maxPrivateBytes = [long]$MaxTreePrivateMiB * 1024 * 1024
if ($privateValues.Count -gt 0 -and ($privateValues | Measure-Object -Maximum).Maximum -gt $maxPrivateBytes) {
  $thresholdFailures.Add("Process-tree private memory exceeded ${MaxTreePrivateMiB}MiB.")
}

$report = [pscustomobject]@{
  schemaVersion = 1
  measuredAt = (Get-Date).ToString("o")
  measurement = "fresh-profile time to responsive main window; process-tree memory after a fixed settle delay"
  executable = [System.IO.Path]::GetRelativePath($repoRoot, $Executable)
  executableSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Executable).Hash.ToLowerInvariant()
  gitCommit = (git -C $repoRoot rev-parse HEAD).Trim()
  limits = [pscustomobject]@{
    maxResponsiveWindowMs = $MaxResponsiveMilliseconds
    maxTreePrivateBytes = $maxPrivateBytes
  }
  summary = [pscustomobject]@{
    runs = $Runs
    passingRuns = $passing.Count
    responsiveWindowMedianMs = Get-Median $responsiveValues
    responsiveWindowMaxMs = if ($responsiveValues.Count -gt 0) { [long](($responsiveValues | Measure-Object -Maximum).Maximum) } else { $null }
    treeWorkingSetMedianBytes = Get-Median $workingSetValues
    treeWorkingSetMaxBytes = if ($workingSetValues.Count -gt 0) { [long](($workingSetValues | Measure-Object -Maximum).Maximum) } else { $null }
    treePrivateMedianBytes = Get-Median $privateValues
    treePrivateMaxBytes = if ($privateValues.Count -gt 0) { [long](($privateValues | Measure-Object -Maximum).Maximum) } else { $null }
  }
  status = if ($thresholdFailures.Count -eq 0) { "PASS" } else { "FAIL" }
  thresholdFailures = @($thresholdFailures)
  results = @($results)
}
$outputPath = Join-Path $EvidenceDir "runtime-performance.json"
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $outputPath -Encoding utf8

if ($thresholdFailures.Count -gt 0) {
  throw "Runtime performance acceptance failed: $($thresholdFailures -join ' ') Evidence: $outputPath"
}

Write-Host "Runtime performance acceptance passed. Evidence: $outputPath" -ForegroundColor Green
Write-Host ("Responsive median/max: {0}/{1} ms; process-tree private median/max: {2:N1}/{3:N1} MiB" -f `
  $report.summary.responsiveWindowMedianMs,
  $report.summary.responsiveWindowMaxMs,
  ($report.summary.treePrivateMedianBytes / 1MB),
  ($report.summary.treePrivateMaxBytes / 1MB))
