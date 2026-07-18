[CmdletBinding()]
param(
  [string]$EvidenceDir,
  [string]$BaselineLocalInstaller,
  [string]$CandidateLocalInstaller,
  [string]$CandidateConnectorInstaller,
  [string]$BaselineCatalogHelper,
  [string]$CandidateCatalogHelper,
  [switch]$ValidateOnly,
  [switch]$Resume,
  [switch]$SelfTest,
  [ValidateRange(60, 3600)][int]$TimeoutSeconds = 1200
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance\v0.1.1\sandbox-lifecycle"))

function Get-NonNullItems {
  param([object]$Value)
  return @($Value | Where-Object { $null -ne $_ })
}

function Get-IgnorableLifecycleFailureIds {
  param([string[]]$ResultIds)

  # Only the first repaired-Sandbox evidence set contains the superseded 07 retry.
  # A fresh run never emits 07, so its authoritative 06 registration failure must
  # remain fatal instead of inheriting this one-time historical exception.
  if ($ResultIds -contains "07-register-baseline-catalog") {
    return @("06-register-baseline-catalog", "07-register-baseline-catalog")
  }
  return @()
}

function Resolve-EvidenceDirectory {
  param([string]$RequestedPath, [bool]$ExistingOnly)

  if ([string]::IsNullOrWhiteSpace($RequestedPath)) {
    if ($ExistingOnly) {
      throw "EvidenceDir is required with -ValidateOnly or -Resume."
    }
    $RequestedPath = Join-Path $acceptanceRoot (Get-Date -Format "yyyyMMdd-HHmmss")
  }

  $resolved = [System.IO.Path]::GetFullPath($RequestedPath)
  $allowedPrefix = $acceptanceRoot.TrimEnd("\") + "\"
  if (-not $resolved.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "EvidenceDir must stay under $acceptanceRoot"
  }
  if ($ExistingOnly -and -not (Test-Path -LiteralPath $resolved -PathType Container)) {
    throw "Lifecycle evidence directory does not exist: $resolved"
  }
  return $resolved
}

function Test-SandboxSessionMissingBeyondGrace {
  param(
    [Parameter(Mandatory = $true)][datetime]$WaitStartedAt,
    [Parameter(Mandatory = $true)][datetime]$Now,
    [Parameter(Mandatory = $true)][bool]$SessionRunning,
    [ValidateRange(1, 60)][int]$StartupGraceSeconds = 15
  )

  if ($SessionRunning) { return $false }
  return ($Now - $WaitStartedAt).TotalSeconds -ge $StartupGraceSeconds
}

function Wait-ForPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][datetime]$Deadline,
    [Parameter(Mandatory = $true)][string]$Description
  )

  $waitStartedAt = Get-Date
  while (-not (Test-Path -LiteralPath $Path)) {
    $now = Get-Date
    if ($now -ge $Deadline) {
      throw "Timed out waiting for $Description at $Path"
    }
    $sessionRunning = $null -ne (Get-Process -Name WindowsSandboxRemoteSession -ErrorAction SilentlyContinue)
    if (Test-SandboxSessionMissingBeyondGrace -WaitStartedAt $waitStartedAt -Now $now -SessionRunning $sessionRunning) {
      throw "Windows Sandbox exited while waiting for $Description."
    }
    Start-Sleep -Milliseconds 500
  }
}

function Write-JsonFile {
  param([string]$Path, [object]$Value)
  $Value | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $Path -Encoding utf8
}

function Get-LifecycleResult {
  param([string]$ResultsDir, [string]$Id)
  $path = Join-Path $ResultsDir "$Id.json"
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Required lifecycle result is missing: $Id"
  }
  $result = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
  if ($result.id -ne $Id) {
    throw "Lifecycle result identity mismatch for $Id."
  }
  if ($result.status -ne "PASS") {
    throw "Lifecycle result did not pass: $Id ($($result.error))"
  }
  return $result
}

function Get-AppByName {
  param([object]$State, [string]$DisplayName)
  $matches = @(Get-NonNullItems $State.applications | Where-Object { $_.displayName -eq $DisplayName })
  if ($matches.Count -ne 1) {
    throw "Expected exactly one installed '$DisplayName' entry, found $($matches.Count)."
  }
  return $matches[0]
}

function Assert-AppSet {
  param(
    [object]$State,
    [bool]$ExpectLocal,
    [bool]$ExpectConnector,
    [string]$ExpectedVersion
  )

  $apps = @(Get-NonNullItems $State.applications)
  $expectedCount = [int]$ExpectLocal + [int]$ExpectConnector
  if ($apps.Count -ne $expectedCount) {
    throw "Expected $expectedCount installed app(s), found $($apps.Count)."
  }

  if ($ExpectLocal) {
    $local = Get-AppByName -State $State -DisplayName "Code Hangar"
    if ($local.displayVersion -ne $ExpectedVersion -or [bool]$local.sidecarExists) {
      throw "Local edition version/sidecar invariant failed."
    }
  }
  if ($ExpectConnector) {
    $connector = Get-AppByName -State $State -DisplayName "Code Hangar AI Connector"
    if ($connector.displayVersion -ne $ExpectedVersion -or -not [bool]$connector.sidecarExists) {
      throw "Connector edition version/sidecar invariant failed."
    }
  }
}

function Get-CatalogSignature {
  param([object]$CatalogResult)
  $catalog = $CatalogResult.detail.catalog
  return [pscustomobject]@{
    projectId = [int64]$catalog.project.id
    projectName = [string]$catalog.project.name
    projectPath = [string]$catalog.project.path
    scanState = [string]$catalog.project.scanState
    rootPath = [string]$catalog.root.path
    contextFiles = @($catalog.contextFiles | Sort-Object)
  }
}

function Assert-CatalogIdentity {
  param([object]$Expected, [object]$Actual, [string]$Label)
  $expectedJson = $Expected | ConvertTo-Json -Depth 5 -Compress
  $actualJson = $Actual | ConvertTo-Json -Depth 5 -Compress
  if ($expectedJson -cne $actualJson) {
    throw "Catalog identity changed at $Label. Expected $expectedJson, found $actualJson"
  }
}

function New-LifecycleManifest {
  param([string]$Root)

  $sharedDir = Join-Path $Root "shared"
  $resultsDir = Join-Path $sharedDir "results"
  $checks = [System.Collections.Generic.List[object]]::new()
  $resultSummaries = [System.Collections.Generic.List[object]]::new()
  $ignoredFailures = [System.Collections.Generic.List[object]]::new()
  $failure = $null

  try {
    $agentReady = Get-Content -LiteralPath (Join-Path $resultsDir "agent-ready.json") -Raw | ConvertFrom-Json
    $agentStopped = Get-Content -LiteralPath (Join-Path $resultsDir "agent-stopped.json") -Raw | ConvertFrom-Json
    if ($agentReady.status -ne "PASS" -or $agentStopped.status -ne "PASS") {
      throw "The guest lifecycle agent did not start and stop cleanly."
    }
    $checks.Add([pscustomobject]@{ name = "guest-agent-lifecycle"; status = "PASS" })

    $resultFiles = @(Get-ChildItem -LiteralPath $resultsDir -Filter "*.json" -File | Sort-Object Name)
    $resultIds = @($resultFiles | ForEach-Object { $_.BaseName })
    $allowedLegacyFailures = @(Get-IgnorableLifecycleFailureIds -ResultIds $resultIds)
    foreach ($file in $resultFiles) {
      $value = Get-Content -LiteralPath $file.FullName -Raw | ConvertFrom-Json
      if (-not ($value.PSObject.Properties.Name -contains "id")) { continue }
      $resultSummaries.Add([pscustomobject]@{
        id = [string]$value.id
        status = [string]$value.status
        startedAt = [string]$value.startedAt
        completedAt = [string]$value.completedAt
      })
      if ($value.status -eq "FAIL") {
        if ($allowedLegacyFailures -contains [string]$value.id) {
          $ignoredFailures.Add([pscustomobject]@{
            id = [string]$value.id
            reason = "Superseded harness attempt; the subsequent baseline catalog check is authoritative."
            error = [string]$value.error
          })
        } else {
          throw "Unexpected failed lifecycle result: $($value.id)"
        }
      }
    }

    $cleanInstall = Get-LifecycleResult $resultsDir "01-clean-install-local-011"
    $cleanBeforeApps = @(Get-NonNullItems $cleanInstall.detail.before.applications)
    if ($cleanBeforeApps.Count -ne 0 -or [bool]$cleanInstall.detail.before.catalog.keyExists) {
      throw "The clean-install proof did not start from an empty app/catalog-key state."
    }
    Assert-AppSet $cleanInstall.detail.after $true $false "0.1.1"
    $checks.Add([pscustomobject]@{ name = "clean-offline-local-install"; status = "PASS"; installerSha256 = [string]$cleanInstall.detail.installerSha256 })

    $provisioningUninstall = Get-LifecycleResult $resultsDir "02-uninstall-provisioning-local"
    Assert-AppSet $provisioningUninstall.detail.after $false $false "0.1.1"

    $baselineInstall = Get-LifecycleResult $resultsDir "03-install-baseline-local-010"
    Assert-AppSet $baselineInstall.detail.after $true $false "0.1.0"
    Get-LifecycleResult $resultsDir "04-launch-baseline-local-010" | Out-Null
    Get-LifecycleResult $resultsDir "05-close-baseline-local-010" | Out-Null

    $baselineCatalog = Get-LifecycleResult $resultsDir "08-check-baseline-catalog"
    $catalogKey = [string]$baselineCatalog.detail.state.catalog.keySha256
    if ([string]::IsNullOrWhiteSpace($catalogKey)) {
      throw "The baseline catalog has no DPAPI key hash."
    }
    $catalogSignature = Get-CatalogSignature $baselineCatalog
    if ($catalogSignature.projectName -ne "test-project" -or $catalogSignature.scanState -ne "scanned") {
      throw "The baseline project was not registered and scanned."
    }
    if (($catalogSignature.contextFiles -join "|") -ne "AGENTS.md|README.md") {
      throw "The baseline catalog does not contain the expected context files."
    }
    $checks.Add([pscustomobject]@{ name = "baseline-catalog"; status = "PASS"; keySha256 = $catalogKey; signature = $catalogSignature })

    $upgrade = Get-LifecycleResult $resultsDir "09-upgrade-local-011"
    Assert-AppSet $upgrade.detail.after $true $false "0.1.1"
    if ($upgrade.detail.before.catalog.keySha256 -ne $catalogKey -or $upgrade.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "The encrypted catalog key changed during upgrade."
    }
    $upgradedCatalog = Get-LifecycleResult $resultsDir "10-check-upgraded-catalog-011"
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $upgradedCatalog) "upgrade"
    if ($upgradedCatalog.detail.state.catalog.keySha256 -ne $catalogKey) {
      throw "The upgraded catalog key hash changed."
    }
    $checks.Add([pscustomobject]@{ name = "upgrade-0.1.0-to-0.1.1"; status = "PASS" })

    $connectorInstall = Get-LifecycleResult $resultsDir "11-install-connector-011"
    Assert-AppSet $connectorInstall.detail.after $true $true "0.1.1"
    if ($connectorInstall.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "The catalog key changed when Connector was installed."
    }
    Get-LifecycleResult $resultsDir "12-launch-connector-011" | Out-Null
    Get-LifecycleResult $resultsDir "13-close-connector-011" | Out-Null
    $connectorCatalog = Get-LifecycleResult $resultsDir "14-check-connector-catalog"
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $connectorCatalog) "Connector coexistence"

    $repair = Get-LifecycleResult $resultsDir "15-repair-connector-011"
    Assert-AppSet $repair.detail.before $true $true "0.1.1"
    Assert-AppSet $repair.detail.after $true $true "0.1.1"
    if ($repair.detail.before.catalog.keySha256 -ne $catalogKey -or $repair.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Connector repair changed the catalog key."
    }
    $checks.Add([pscustomobject]@{ name = "edition-coexistence-and-repair"; status = "PASS" })

    $removeLocal = Get-LifecycleResult $resultsDir "16-uninstall-local"
    Assert-AppSet $removeLocal.detail.after $false $true "0.1.1"
    if ($removeLocal.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Uninstalling Local changed the shared catalog key."
    }
    Get-LifecycleResult $resultsDir "17-launch-connector-after-local-uninstall" | Out-Null
    Get-LifecycleResult $resultsDir "18-close-connector-after-local-uninstall" | Out-Null
    $connectorOnlyCatalog = Get-LifecycleResult $resultsDir "19-check-connector-only-catalog"
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $connectorOnlyCatalog) "Connector-only state"

    $removeConnector = Get-LifecycleResult $resultsDir "20-uninstall-connector"
    Assert-AppSet $removeConnector.detail.after $false $false "0.1.1"
    if ($removeConnector.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Uninstalling Connector changed the shared catalog key."
    }

    $reinstallLocal = Get-LifecycleResult $resultsDir "21-reinstall-local-011"
    Assert-AppSet $reinstallLocal.detail.after $true $false "0.1.1"
    if ($reinstallLocal.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Reinstalling Local changed the shared catalog key."
    }
    Get-LifecycleResult $resultsDir "22-launch-reinstalled-local" | Out-Null
    Get-LifecycleResult $resultsDir "23-close-reinstalled-local" | Out-Null
    $reinstalledCatalog = Get-LifecycleResult $resultsDir "24-check-reinstalled-local-catalog"
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $reinstalledCatalog) "Local reinstall"

    $finalUninstall = Get-LifecycleResult $resultsDir "25-final-uninstall-local"
    Assert-AppSet $finalUninstall.detail.after $false $false "0.1.1"
    $finalInspect = Get-LifecycleResult $resultsDir "26-final-inspect"
    Assert-AppSet $finalInspect.detail.state $false $false "0.1.1"
    if ($finalInspect.detail.state.catalog.keySha256 -ne $catalogKey) {
      throw "The final empty-install state did not preserve the shared catalog key."
    }
    Assert-AppSet $agentStopped.state $false $false "0.1.1"
    if ($agentStopped.state.catalog.keySha256 -ne $catalogKey) {
      throw "The stopped guest agent observed a different final catalog key."
    }
    $checks.Add([pscustomobject]@{ name = "uninstall-switching-and-final-state"; status = "PASS" })
  } catch {
    $failure = $_
    $checks.Add([pscustomobject]@{ name = "validation"; status = "FAIL"; error = $_.Exception.Message })
  }

  $manifest = [pscustomobject]@{
    schemaVersion = 1
    generatedAt = (Get-Date).ToString("o")
    evidenceRoot = $Root
    machine = $env:COMPUTERNAME
    gitCommit = (git -C $repoRoot rev-parse HEAD).Trim()
    gitBranch = (git -C $repoRoot branch --show-current).Trim()
    status = if ($null -eq $failure) { "PASS" } else { "FAIL" }
    checks = @($checks)
    results = @($resultSummaries)
    ignoredHarnessAttempts = @($ignoredFailures)
  }
  Write-JsonFile -Path (Join-Path $Root "lifecycle-manifest.json") -Value $manifest
  if ($null -ne $failure) { throw $failure }
  return $manifest
}

function Assert-InputFile {
  param([string]$Path, [string]$Label)
  if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "$Label is required and must be an existing file."
  }
}

function Copy-Artifact {
  param([string]$Source, [string]$Destination)
  Copy-Item -LiteralPath ([System.IO.Path]::GetFullPath($Source)) -Destination $Destination -Force
}

$exclusiveModes = @($ValidateOnly, $Resume, $SelfTest) | Where-Object { [bool]$_ }
if (@($exclusiveModes).Count -gt 1) {
  throw "Use only one of -ValidateOnly, -Resume or -SelfTest."
}

if ($SelfTest) {
  $fresh = @(Get-IgnorableLifecycleFailureIds -ResultIds @(
    "06-register-baseline-catalog",
    "08-check-baseline-catalog"
  ))
  if ($fresh.Count -ne 0) {
    throw "Fresh lifecycle runs must not ignore a failed catalog registration."
  }

  $legacy = @(Get-IgnorableLifecycleFailureIds -ResultIds @(
    "06-register-baseline-catalog",
    "07-register-baseline-catalog",
    "08-check-baseline-catalog"
  ))
  if (($legacy -join "|") -ne "06-register-baseline-catalog|07-register-baseline-catalog") {
    throw "Historical lifecycle compatibility classification changed unexpectedly."
  }

  $waitStartedAt = [datetime]"2026-01-01T00:00:00Z"
  if (Test-SandboxSessionMissingBeyondGrace -WaitStartedAt $waitStartedAt -Now $waitStartedAt.AddSeconds(5) -SessionRunning $false) {
    throw "The Sandbox launcher handoff grace must tolerate a delayed RemoteSession process."
  }
  if (-not (Test-SandboxSessionMissingBeyondGrace -WaitStartedAt $waitStartedAt -Now $waitStartedAt.AddSeconds(15) -SessionRunning $false)) {
    throw "A missing Sandbox session must become fatal after the launcher handoff grace."
  }
  if (Test-SandboxSessionMissingBeyondGrace -WaitStartedAt $waitStartedAt -Now $waitStartedAt.AddSeconds(60) -SessionRunning $true) {
    throw "A running Sandbox session must remain valid after the launcher process exits."
  }

  Write-Host "Sandbox lifecycle validator self-test passed." -ForegroundColor Green
  exit 0
}

$EvidenceDir = Resolve-EvidenceDirectory -RequestedPath $EvidenceDir -ExistingOnly ([bool]($ValidateOnly -or $Resume))

if ($ValidateOnly) {
  $manifest = New-LifecycleManifest -Root $EvidenceDir
  Write-Host "Sandbox lifecycle evidence passed: $EvidenceDir" -ForegroundColor Green
  $manifest | ConvertTo-Json -Depth 4
  exit 0
}

if ($Resume) {
  $sharedDir = Join-Path $EvidenceDir "shared"
  $resultsDir = Join-Path $sharedDir "results"
  if (-not (Test-Path -LiteralPath $resultsDir -PathType Container)) {
    throw "Lifecycle results directory does not exist: $resultsDir"
  }
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  Wait-ForPath -Path (Join-Path $resultsDir "agent-ready.json") -Deadline $deadline -Description "guest agent readiness"
  Wait-ForPath -Path (Join-Path $resultsDir "26-final-inspect.json") -Deadline $deadline -Description "final lifecycle result"
  "stop" | Set-Content -LiteralPath (Join-Path $sharedDir "stop.flag") -Encoding ascii
  Wait-ForPath -Path (Join-Path $resultsDir "agent-stopped.json") -Deadline $deadline -Description "guest agent shutdown"

  $manifest = New-LifecycleManifest -Root $EvidenceDir
  Write-Host "Sandbox lifecycle resumed and passed: $EvidenceDir" -ForegroundColor Green
  Write-Host "The stopped Sandbox window remains open for inspection; closing it discards guest state."
  $manifest | ConvertTo-Json -Depth 4
  exit 0
}

Assert-InputFile $BaselineLocalInstaller "BaselineLocalInstaller"
Assert-InputFile $CandidateLocalInstaller "CandidateLocalInstaller"
Assert-InputFile $CandidateConnectorInstaller "CandidateConnectorInstaller"
Assert-InputFile $BaselineCatalogHelper "BaselineCatalogHelper"
Assert-InputFile $CandidateCatalogHelper "CandidateCatalogHelper"

if (Get-Process -Name WindowsSandboxRemoteSession -ErrorAction SilentlyContinue) {
  throw "Close the existing Windows Sandbox before starting a new lifecycle run."
}

$windowsSandbox = Join-Path $env:windir "System32\WindowsSandbox.exe"
if (-not (Test-Path -LiteralPath $windowsSandbox -PathType Leaf)) {
  throw "Windows Sandbox is not installed at $windowsSandbox"
}

$sharedDir = Join-Path $EvidenceDir "shared"
$commandsDir = Join-Path $sharedDir "commands"
$resultsDir = Join-Path $sharedDir "results"
New-Item -ItemType Directory -Path $commandsDir, $resultsDir, (Join-Path $sharedDir "test-project\src") -Force | Out-Null

Copy-Artifact $BaselineLocalInstaller (Join-Path $sharedDir "Code Hangar_0.1.0_x64-setup.exe")
Copy-Artifact $CandidateLocalInstaller (Join-Path $sharedDir "Code Hangar_0.1.1_x64-setup.exe")
Copy-Artifact $CandidateConnectorInstaller (Join-Path $sharedDir "Code Hangar AI Connector_0.1.1_x64-setup.exe")
Copy-Artifact $BaselineCatalogHelper (Join-Path $sharedDir "acceptance_catalog_010.exe")
Copy-Artifact $CandidateCatalogHelper (Join-Path $sharedDir "acceptance_catalog_011.exe")
Copy-Artifact (Join-Path $PSScriptRoot "sandbox-lifecycle-agent.ps1") (Join-Path $sharedDir "sandbox-lifecycle-agent.ps1")
Copy-Artifact (Join-Path $env:windir "System32\VCRUNTIME140.dll") (Join-Path $sharedDir "VCRUNTIME140.dll")

"# Sandbox lifecycle fixture" | Set-Content -LiteralPath (Join-Path $sharedDir "test-project\README.md") -Encoding utf8
"# Acceptance agent context" | Set-Content -LiteralPath (Join-Path $sharedDir "test-project\AGENTS.md") -Encoding utf8
"fn main() {}" | Set-Content -LiteralPath (Join-Path $sharedDir "test-project\src\main.rs") -Encoding utf8

$commands = @(
  @{ id = "01-clean-install-local-011"; action = "install"; installer = "Code Hangar_0.1.1_x64-setup.exe"; displayName = "Code Hangar"; expectedVersion = "0.1.1"; expectedSidecar = $false },
  @{ id = "02-uninstall-provisioning-local"; action = "uninstall"; displayName = "Code Hangar"; expectCatalogPreserved = $false },
  @{ id = "03-install-baseline-local-010"; action = "install"; installer = "Code Hangar_0.1.0_x64-setup.exe"; displayName = "Code Hangar"; expectedVersion = "0.1.0"; expectedSidecar = $false },
  @{ id = "04-launch-baseline-local-010"; action = "launch"; displayName = "Code Hangar"; expectedVersion = "0.1.0"; expectedSidecar = $false },
  @{ id = "05-close-baseline-local-010"; action = "close" },
  @{ id = "06-register-baseline-catalog"; action = "catalog"; mode = "register"; helper = "acceptance_catalog_010.exe"; project = "test-project" },
  @{ id = "08-check-baseline-catalog"; action = "catalog"; mode = "check"; helper = "acceptance_catalog_010.exe"; project = "test-project" },
  @{ id = "09-upgrade-local-011"; action = "install"; installer = "Code Hangar_0.1.1_x64-setup.exe"; displayName = "Code Hangar"; expectedVersion = "0.1.1"; expectedSidecar = $false },
  @{ id = "10-check-upgraded-catalog-011"; action = "catalog"; mode = "check"; helper = "acceptance_catalog_011.exe"; project = "test-project" },
  @{ id = "11-install-connector-011"; action = "install"; installer = "Code Hangar AI Connector_0.1.1_x64-setup.exe"; displayName = "Code Hangar AI Connector"; expectedVersion = "0.1.1"; expectedSidecar = $true },
  @{ id = "12-launch-connector-011"; action = "launch"; displayName = "Code Hangar AI Connector"; expectedVersion = "0.1.1"; expectedSidecar = $true },
  @{ id = "13-close-connector-011"; action = "close" },
  @{ id = "14-check-connector-catalog"; action = "catalog"; mode = "check"; helper = "acceptance_catalog_011.exe"; project = "test-project" },
  @{ id = "15-repair-connector-011"; action = "install"; installer = "Code Hangar AI Connector_0.1.1_x64-setup.exe"; displayName = "Code Hangar AI Connector"; expectedVersion = "0.1.1"; expectedSidecar = $true },
  @{ id = "16-uninstall-local"; action = "uninstall"; displayName = "Code Hangar"; expectCatalogPreserved = $true },
  @{ id = "17-launch-connector-after-local-uninstall"; action = "launch"; displayName = "Code Hangar AI Connector"; expectedVersion = "0.1.1"; expectedSidecar = $true },
  @{ id = "18-close-connector-after-local-uninstall"; action = "close" },
  @{ id = "19-check-connector-only-catalog"; action = "catalog"; mode = "check"; helper = "acceptance_catalog_011.exe"; project = "test-project" },
  @{ id = "20-uninstall-connector"; action = "uninstall"; displayName = "Code Hangar AI Connector"; expectCatalogPreserved = $true },
  @{ id = "21-reinstall-local-011"; action = "install"; installer = "Code Hangar_0.1.1_x64-setup.exe"; displayName = "Code Hangar"; expectedVersion = "0.1.1"; expectedSidecar = $false },
  @{ id = "22-launch-reinstalled-local"; action = "launch"; displayName = "Code Hangar"; expectedVersion = "0.1.1"; expectedSidecar = $false },
  @{ id = "23-close-reinstalled-local"; action = "close" },
  @{ id = "24-check-reinstalled-local-catalog"; action = "catalog"; mode = "check"; helper = "acceptance_catalog_011.exe"; project = "test-project" },
  @{ id = "25-final-uninstall-local"; action = "uninstall"; displayName = "Code Hangar"; expectCatalogPreserved = $true },
  @{ id = "26-final-inspect"; action = "inspect" }
)

foreach ($command in $commands) {
  Write-JsonFile -Path (Join-Path $commandsDir "$($command.id).json") -Value $command
}

$hostFolder = [System.Security.SecurityElement]::Escape($sharedDir)
# The LogonCommand sets CODEHANGAR_SANDBOX_AGENT=1 before invoking the agent — a sandbox-only
# sentinel (this command runs only inside the guest) that the agent's fail-closed guard checks
# alongside the WDAGUtilityAccount auto-logon. The env assignment and quotes are backtick-escaped
# so the host does NOT expand them while building this string; the agent path is rooted, so it is
# executed directly without a call operator. Keep it in sync with the guard in
# sandbox-lifecycle-agent.ps1.
$sandboxConfig = "<Configuration><VGpu>Disable</VGpu><Networking>Disable</Networking><MappedFolders><MappedFolder><HostFolder>$hostFolder</HostFolder><SandboxFolder>C:\CodeHangarAcceptance</SandboxFolder><ReadOnly>false</ReadOnly></MappedFolder></MappedFolders><LogonCommand><Command>powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -Command `"`$env:CODEHANGAR_SANDBOX_AGENT='1'; C:\CodeHangarAcceptance\sandbox-lifecycle-agent.ps1 -SharedRoot C:\CodeHangarAcceptance`"</Command></LogonCommand></Configuration>"
$sandboxPath = Join-Path $EvidenceDir "lifecycle.wsb"
$sandboxConfig | Set-Content -LiteralPath $sandboxPath -Encoding utf8
$EvidenceDir | Set-Content -LiteralPath (Join-Path $acceptanceRoot "..\sandbox-current.txt") -Encoding utf8

Start-Process -FilePath $windowsSandbox -ArgumentList "`"$sandboxPath`"" | Out-Null
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
Wait-ForPath -Path (Join-Path $resultsDir "agent-ready.json") -Deadline $deadline -Description "guest agent readiness"
Wait-ForPath -Path (Join-Path $resultsDir "26-final-inspect.json") -Deadline $deadline -Description "final lifecycle result"
"stop" | Set-Content -LiteralPath (Join-Path $sharedDir "stop.flag") -Encoding ascii
Wait-ForPath -Path (Join-Path $resultsDir "agent-stopped.json") -Deadline $deadline -Description "guest agent shutdown"

$manifest = New-LifecycleManifest -Root $EvidenceDir
Write-Host "Sandbox lifecycle passed: $EvidenceDir" -ForegroundColor Green
Write-Host "The stopped Sandbox window remains open for inspection; closing it discards guest state."
$manifest | ConvertTo-Json -Depth 4
