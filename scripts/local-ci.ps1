param(
  [switch]$SkipTauriBuild,
  [switch]$CoreOnly,
  [switch]$AgentAutomation,
  # Perf gate, STAGE 1 (non-blocking): time each step and, on a green run, write a machine-local
  # baseline to .local/perf-baseline.json (gitignored - never committed). Default off, so normal
  # CI behavior is unchanged.
  [switch]$Measure,
  # Perf gate, STAGE 2 (blocking): time each step and compare against the machine-local baseline,
  # failing the run on a GROSS regression only (generous tolerance, so normal build-cache / machine-
  # load variance never trips it - it catches a catastrophic slowdown, not noise). Bootstraps a
  # baseline on the first run when none exists. Refresh the baseline deliberately with -Measure.
  [switch]$PerfGate
)

$ErrorActionPreference = "Stop"
$script:StepTimings = @()

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$cargoBins = @(
  (Join-Path $env:USERPROFILE ".cargo\bin"),
  (Join-Path $env:USERPROFILE ".local\cargo\bin")
)

foreach ($cargoBin in $cargoBins) {
  if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
  }
}

function Run-Step {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Label,
    [Parameter(Mandatory = $true)]
    [scriptblock]$Command
  )

  Write-Host ""
  Write-Host "==> $Label" -ForegroundColor Cyan
  $stepStart = Get-Date
  & $Command
  $exit = $LASTEXITCODE
  if ($Measure -or $PerfGate) {
    $elapsedMs = [int]((Get-Date) - $stepStart).TotalMilliseconds
    $script:StepTimings += [pscustomobject]@{ name = $Label; elapsedMs = $elapsedMs }
    Write-Host ("    {0} ms" -f $elapsedMs) -ForegroundColor DarkGray
  }
  if ($exit -ne 0) {
    throw "$Label failed with exit code $exit"
  }
}

# Pure regression check (no I/O), so the gate logic stays testable. A step is a regression only when
# it is BOTH more than $Tolerance x its baseline AND grew by more than $FloorMs ms - the floor keeps
# noise on fast steps (50 ms -> 150 ms) from ever tripping the gate; only slow steps that blow up do.
function Get-PerfRegressions {
  param(
    [Parameter(Mandatory = $true)] $BaselineSteps,
    [Parameter(Mandatory = $true)] $CurrentSteps,
    [double]$Tolerance = 2.0,
    [int]$FloorMs = 5000
  )
  $regressions = @()
  foreach ($step in $CurrentSteps) {
    $base = $BaselineSteps | Where-Object { $_.name -eq $step.name } | Select-Object -First 1
    if ($null -eq $base) { continue }
    if ($step.elapsedMs -gt ($base.elapsedMs * $Tolerance) -and ($step.elapsedMs - $base.elapsedMs) -gt $FloorMs) {
      $regressions += [pscustomobject]@{ name = $step.name; baselineMs = [int]$base.elapsedMs; nowMs = [int]$step.elapsedMs }
    }
  }
  return $regressions
}

# Write the machine-local baseline (hostname + git sha stamped, never compared across machines and
# never committed). Returns the baseline object. Shared by -Measure and the -PerfGate bootstrap.
function Write-PerfBaseline {
  param([Parameter(Mandatory = $true)][string]$Path)
  $gitSha = ""
  try { $gitSha = (git rev-parse --short HEAD 2>$null) } catch { $gitSha = "" }
  $baseline = [pscustomobject]@{
    recordedAt = (Get-Date).ToString("o")
    machine    = $env:COMPUTERNAME
    gitSha     = $gitSha
    totalMs    = ($script:StepTimings | Measure-Object -Property elapsedMs -Sum).Sum
    steps      = $script:StepTimings
  }
  $baseline | ConvertTo-Json -Depth 5 | Set-Content -Path $Path -Encoding UTF8
  return $baseline
}

# The connected-AI-app (MCP) crates always link hangar-api with agent_automation
# (the server cannot function without authenticated dispatch). They must NOT be
# pulled into the core/mutation feature lanes, or those lanes would stop proving
# core/Local isolation. They are excluded here and covered by their own lane
# below; Local/core isolation is separately guaranteed by
# check-no-outbound-deps.mjs (a targeted cargo-tree over code-hangar-desktop).
$connectedAppCrates = @(
  "--exclude", "hangar-mcp",
  "--exclude", "code-hangar-mcp",
  "--exclude", "hangar-appconfig"
)

Run-Step "npm run check" { npm.cmd run check }
Run-Step "frontend Local edition isolation" { npm.cmd --workspace apps/desktop run build:local }
Run-Step "cargo fmt" { cargo fmt --all --check }
Run-Step "sandbox lifecycle validator self-test" { pwsh -NoProfile -File scripts/sandbox-lifecycle.ps1 -SelfTest }
Run-Step "cargo test core" { cargo test --workspace $connectedAppCrates --no-default-features --features core }
Run-Step "cargo clippy core" { cargo clippy --workspace $connectedAppCrates --all-targets --no-default-features --features core -- -D warnings }

if (-not $CoreOnly) {
  Run-Step "cargo test mutation" { cargo test --workspace $connectedAppCrates --no-default-features --features mutation }
  Run-Step "cargo clippy mutation" { cargo clippy --workspace $connectedAppCrates --all-targets --no-default-features --features mutation -- -D warnings }
}

if ($AgentAutomation) {
  Run-Step "cargo test agent automation" { cargo test --workspace $connectedAppCrates --no-default-features --features agent_automation }
  Run-Step "cargo clippy agent automation" { cargo clippy --workspace $connectedAppCrates --all-targets --no-default-features --features agent_automation -- -D warnings }
  Run-Step "cargo build connected-app server release" { cargo build -p code-hangar-mcp --release }
}

# Dedicated lane for the feature-gated connected-AI-app surface. These crates carry
# their own feature wiring (they pull hangar-api/agent_automation themselves), so
# they build without the workspace feature flags.
Run-Step "cargo test connected-app surface" { cargo test -p hangar-mcp -p code-hangar-mcp -p hangar-appconfig }
Run-Step "cargo clippy connected-app surface" { cargo clippy -p hangar-mcp -p code-hangar-mcp -p hangar-appconfig --all-targets -- -D warnings }

if (-not $SkipTauriBuild) {
  if ($AgentAutomation) {
    $bundleStartedAt = Get-Date
    Run-Step "tauri build agent automation" { npm.cmd --workspace apps/desktop run tauri:build:agent }
    Run-Step "verify agent release artifacts" {
      $desktopExe = Join-Path $repoRoot "target\release\code-hangar-desktop.exe"
      $mcpExe = Join-Path $repoRoot "target\release\code-hangar-mcp.exe"
      $nsisDir = Join-Path $repoRoot "target\release\bundle\nsis"
      if (-not (Test-Path $desktopExe)) {
        throw "Missing desktop executable: $desktopExe"
      }
      if (-not (Test-Path $mcpExe)) {
        throw "Missing connected-app server executable: $mcpExe"
      }
      $connectorInstaller = Get-ChildItem -Path $nsisDir -Filter "Code Hangar AI Connector_*_x64-setup.exe" -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -ge $bundleStartedAt } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
      if ($null -eq $connectorInstaller) {
        throw "Missing freshly built AI Connector installer in $nsisDir. The agent automation build must use tauri.connector.conf.json."
      }
    }
  } elseif ($CoreOnly) {
    Run-Step "tauri build core" { npm.cmd --workspace apps/desktop run tauri:build }
  } else {
    $bundleStartedAt = Get-Date
    Run-Step "tauri build mutation" { npm.cmd --workspace apps/desktop run tauri:build:mutation }
    Run-Step "verify local release artifacts" {
      $nsisDir = Join-Path $repoRoot "target\release\bundle\nsis"
      $localInstaller = Get-ChildItem -Path $nsisDir -Filter "Code Hangar_*_x64-setup.exe" -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike "Code Hangar AI Connector_*" -and $_.LastWriteTime -ge $bundleStartedAt } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
      if ($null -eq $localInstaller) {
        throw "Missing freshly built Local edition installer in $nsisDir."
      }
    }
  }
}

if ($Measure -or $PerfGate) {
  $localDir = Join-Path $repoRoot ".local"
  if (-not (Test-Path $localDir)) {
    New-Item -ItemType Directory -Path $localDir | Out-Null
  }
  $baselinePath = Join-Path $localDir "perf-baseline.json"
  $totalNow = ($script:StepTimings | Measure-Object -Property elapsedMs -Sum).Sum

  if ($PerfGate) {
    # STAGE 2 (blocking): compare against the existing baseline and fail on a gross regression.
    if (-not (Test-Path $baselinePath)) {
      Write-Host ""
      Write-Host "Perf gate: no baseline yet - bootstrapping one from this green run. Re-run with -PerfGate to enforce." -ForegroundColor Yellow
      [void](Write-PerfBaseline -Path $baselinePath)
    } else {
      $baseline = Get-Content $baselinePath -Raw | ConvertFrom-Json
      if ($baseline.machine -ne $env:COMPUTERNAME) {
        Write-Host ""
        Write-Host ("Perf gate: baseline is from '{0}', this is '{1}' - skipping (baselines are per-machine; record one here with -Measure)." -f $baseline.machine, $env:COMPUTERNAME) -ForegroundColor Yellow
      } else {
        $tolerance = 2.0
        $totalTolerance = 1.5
        $regressions = Get-PerfRegressions -BaselineSteps $baseline.steps -CurrentSteps $script:StepTimings -Tolerance $tolerance
        $totalLimit = [int]($baseline.totalMs * $totalTolerance)
        $totalRegressed = $totalNow -gt $totalLimit
        if ($regressions.Count -gt 0 -or $totalRegressed) {
          Write-Host ""
          Write-Host "Perf gate FAILED - gross regression vs baseline:" -ForegroundColor Red
          foreach ($r in $regressions) {
            $factor = [math]::Round($r.nowMs / [math]::Max(1, $r.baselineMs), 2)
            Write-Host ("  {0}: {1} ms -> {2} ms ({3}x baseline)" -f $r.name, $r.baselineMs, $r.nowMs, $factor) -ForegroundColor Red
          }
          if ($totalRegressed) {
            Write-Host ("  TOTAL: {0} ms -> {1} ms (limit {2} ms)" -f $baseline.totalMs, $totalNow, $totalLimit) -ForegroundColor Red
          }
          Write-Host "If this slowdown is expected, refresh the baseline with -Measure." -ForegroundColor Yellow
          throw "Perf gate failed: gross performance regression vs baseline."
        }
        Write-Host ""
        Write-Host ("Perf gate passed - every step within {0}x of baseline (total {1} ms vs baseline {2} ms, limit {3} ms)." -f $tolerance, $totalNow, $baseline.totalMs, $totalLimit) -ForegroundColor Green
      }
    }
  }

  if ($Measure) {
    # STAGE 1: (re)record the machine-local baseline from this green run.
    $baseline = Write-PerfBaseline -Path $baselinePath
    Write-Host ""
    Write-Host ("Perf baseline written to {0} ({1} steps, {2} ms total)." -f $baselinePath, $script:StepTimings.Count, $baseline.totalMs) -ForegroundColor Green
  }
}

Write-Host ""
Write-Host "Local CI passed." -ForegroundColor Green
