[CmdletBinding()]
param(
  [ValidateSet("Baseline", "RealData", "DataStress", "WslOff", "RuntimePerf", "McpHarness", "Gate3", "LocalProvider")]
  [string[]]$Lane = @("Baseline"),
  [string]$EvidenceDir
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance"))
if ([string]::IsNullOrWhiteSpace($EvidenceDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $EvidenceDir = Join-Path $acceptanceRoot "v0.1.1\$stamp"
}
$EvidenceDir = [System.IO.Path]::GetFullPath($EvidenceDir)
$allowedPrefix = $acceptanceRoot.TrimEnd("\") + "\"
if (-not $EvidenceDir.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "EvidenceDir must stay under $acceptanceRoot"
}
New-Item -ItemType Directory -Path $EvidenceDir -Force | Out-Null

$startedAt = Get-Date
$results = [System.Collections.Generic.List[object]]::new()

function Invoke-AcceptanceStep {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$DisplayCommand,
    [Parameter(Mandatory = $true)][scriptblock]$Command
  )

  $slug = ($Name.ToLowerInvariant() -replace "[^a-z0-9]+", "-").Trim("-")
  $logPath = Join-Path $EvidenceDir "$slug.log"
  $stepStart = Get-Date
  $status = "PASS"
  $exitCode = 0
  "`$ $DisplayCommand" | Set-Content -LiteralPath $logPath -Encoding utf8

  try {
    & $Command *>&1 | Tee-Object -FilePath $logPath -Append
    if ($LASTEXITCODE -ne 0) {
      $exitCode = $LASTEXITCODE
      throw "$Name failed with exit code $exitCode"
    }
  } catch {
    $status = "FAIL"
    if ($exitCode -eq 0) { $exitCode = 1 }
    ($_ | Out-String) | Add-Content -LiteralPath $logPath -Encoding utf8
  }

  $elapsedMs = [int]((Get-Date) - $stepStart).TotalMilliseconds
  $results.Add([pscustomobject]@{
    name       = $Name
    status     = $status
    exitCode   = $exitCode
    elapsedMs  = $elapsedMs
    command    = $DisplayCommand
    log        = [System.IO.Path]::GetRelativePath($repoRoot, $logPath)
  })

  if ($status -eq "FAIL") {
    throw "$Name failed; see $logPath"
  }
}

$failure = $null
try {
  if ($Lane -contains "Baseline") {
    Invoke-AcceptanceStep -Name "Full local release gate" -DisplayCommand "pwsh scripts/local-ci.ps1 -AgentAutomation -PerfGate" -Command {
      pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\local-ci.ps1 -AgentAutomation -PerfGate
    }
  }

  if ($Lane -contains "RealData") {
    Invoke-AcceptanceStep -Name "Real discovery adapters" -DisplayCommand "cargo test -p hangar-discovery -- --ignored --nocapture --test-threads=1" -Command {
      cargo test -p hangar-discovery -- --ignored --nocapture --test-threads=1
    }
    Invoke-AcceptanceStep -Name "Real session preview adapters" -DisplayCommand "cargo test -p hangar-api --no-default-features --features core tests::real_ -- --ignored --nocapture --test-threads=1 --skip tests::real_wsl_opt_out_does_not_start_a_stopped_distro" -Command {
      $previousActivePath = $env:CODEHANGAR_TEST_ACTIVE_PROJECT_PATH
      try {
        $env:CODEHANGAR_TEST_ACTIVE_PROJECT_PATH = $repoRoot
        cargo test -p hangar-api --no-default-features --features core tests::real_ -- --ignored --nocapture --test-threads=1 --skip tests::real_wsl_opt_out_does_not_start_a_stopped_distro
      } finally {
        $env:CODEHANGAR_TEST_ACTIVE_PROJECT_PATH = $previousActivePath
      }
    }
  }

  if ($Lane -contains "DataStress") {
    Invoke-AcceptanceStep -Name "Large adversarial inventory" -DisplayCommand "cargo test -p hangar-fs --test adversarial_inventory -- --ignored --nocapture --test-threads=1" -Command {
      cargo test -p hangar-fs --test adversarial_inventory -- --ignored --nocapture --test-threads=1
    }
    Invoke-AcceptanceStep -Name "Huge progressive session" -DisplayCommand "cargo test -p hangar-api --no-default-features --features core tests::huge_generated_session_progressively_loads_and_opens_fully -- --ignored --exact --nocapture --test-threads=1" -Command {
      cargo test -p hangar-api --no-default-features --features core tests::huge_generated_session_progressively_loads_and_opens_fully -- --ignored --exact --nocapture --test-threads=1
    }
    Invoke-AcceptanceStep -Name "Bounded transcript paging" -DisplayCommand "npm --workspace apps/desktop run test -- --run src/__tests__/session-transcript.test.ts" -Command {
      npm.cmd --workspace apps/desktop run test -- --run src/__tests__/session-transcript.test.ts
    }
    Invoke-AcceptanceStep -Name "Mutation adversarial battery" -DisplayCommand "cargo test -p hangar-mutation --all-features --test adversarial_battery -- --nocapture --test-threads=1" -Command {
      cargo test -p hangar-mutation --all-features --test adversarial_battery -- --nocapture --test-threads=1
    }
  }

  if ($Lane -contains "WslOff") {
    Invoke-AcceptanceStep -Name "WSL opt-out lifecycle" -DisplayCommand "cargo test -p hangar-api --no-default-features --features core tests::real_wsl_opt_out_does_not_start_a_stopped_distro -- --ignored --exact --nocapture --test-threads=1" -Command {
      cargo test -p hangar-api --no-default-features --features core tests::real_wsl_opt_out_does_not_start_a_stopped_distro -- --ignored --exact --nocapture --test-threads=1
    }
  }

  if ($Lane -contains "RuntimePerf") {
    Invoke-AcceptanceStep -Name "Build isolated runtime candidate" -DisplayCommand "npm --workspace apps/desktop run build; cargo build --release -p code-hangar-desktop --no-default-features --features core" -Command {
      npm.cmd --workspace apps/desktop run build
      if ($LASTEXITCODE -ne 0) { throw "Frontend release build failed with exit code $LASTEXITCODE" }
      cargo build --release -p code-hangar-desktop --no-default-features --features core
    }
    Invoke-AcceptanceStep -Name "Responsive start and process-tree memory" -DisplayCommand "pwsh scripts/measure-runtime.ps1 -EvidenceDir <evidence>" -Command {
      pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\measure-runtime.ps1 -EvidenceDir $EvidenceDir
    }
    Invoke-AcceptanceStep -Name "File-backed database compaction" -DisplayCommand "cargo test -p hangar-db tests::file_backed_compaction_reclaims_space_and_is_repeatable -- --ignored --exact --nocapture --test-threads=1" -Command {
      cargo test -p hangar-db tests::file_backed_compaction_reclaims_space_and_is_repeatable -- --ignored --exact --nocapture --test-threads=1
    }
  }

  if ($Lane -contains "McpHarness") {
    Invoke-AcceptanceStep -Name "Build connected-app sidecar" -DisplayCommand "cargo build --release -p code-hangar-mcp" -Command {
      cargo build --release -p code-hangar-mcp
    }
    Invoke-AcceptanceStep -Name "MCP sentinel lifecycle harness" -DisplayCommand "pwsh scripts/mcp-fixture-smoke.ps1 -EvidenceDir <evidence>" -Command {
      pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\mcp-fixture-smoke.ps1 -EvidenceDir $EvidenceDir
    }
  }

  if ($Lane -contains "Gate3") {
    Invoke-AcceptanceStep -Name "Gate 3 real-file journey" -DisplayCommand "cargo test -p hangar-api --no-default-features --features mutation gate3_final_remove_journey_on_real_files -- --ignored --nocapture --test-threads=1" -Command {
      cargo test -p hangar-api --no-default-features --features mutation gate3_final_remove_journey_on_real_files -- --ignored --nocapture --test-threads=1
    }
  }

  if ($Lane -contains "LocalProvider") {
    Invoke-AcceptanceStep -Name "Live local provider" -DisplayCommand "cargo test -p hangar-ai live_ollama_localhost_connects -- --ignored --nocapture --test-threads=1" -Command {
      cargo test -p hangar-ai live_ollama_localhost_connects -- --ignored --nocapture --test-threads=1
    }
  }
} catch {
  $failure = $_
} finally {
  $manifest = [pscustomobject]@{
    schemaVersion = 1
    startedAt     = $startedAt.ToString("o")
    completedAt   = (Get-Date).ToString("o")
    machine       = $env:COMPUTERNAME
    gitCommit     = (git rev-parse HEAD).Trim()
    gitBranch     = (git branch --show-current).Trim()
    lanes         = @($Lane)
    status        = if ($null -eq $failure) { "PASS" } else { "FAIL" }
    results       = @($results)
  }
  $manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $EvidenceDir "manifest.json") -Encoding utf8
}

if ($null -ne $failure) {
  throw $failure
}

Write-Host ""
Write-Host "Acceptance lanes passed. Evidence: $EvidenceDir" -ForegroundColor Green
