[CmdletBinding()]
param(
  [string]$EvidenceDir,
  [string]$BaselineLocalInstaller,
  [string]$CandidateLocalInstaller,
  [string]$CandidateConnectorInstaller,
  [string]$BaselineCatalogHelper,
  [string]$CandidateCatalogHelper,
  [ValidatePattern('^\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$')][string]$BaselineVersion = "0.1.1",
  [ValidatePattern('^\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$')][string]$CandidateVersion = "0.1.3",
  [switch]$ValidateOnly,
  [switch]$Resume,
  [switch]$SelfTest,
  [ValidateRange(60, 3600)][int]$TimeoutSeconds = 1200
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance\v$CandidateVersion\sandbox-lifecycle"))
$baselineTag = $BaselineVersion -replace '[^0-9A-Za-z]', ''
$candidateTag = $CandidateVersion -replace '[^0-9A-Za-z]', ''
$baselineLocalName = "Code Hangar_${BaselineVersion}_x64-setup.exe"
$candidateLocalName = "Code Hangar_${CandidateVersion}_x64-setup.exe"
$candidateConnectorName = "Code Hangar AI Connector_${CandidateVersion}_x64-setup.exe"
$baselineHelperName = "acceptance_catalog_${baselineTag}.exe"
$candidateHelperName = "acceptance_catalog_${candidateTag}.exe"

function Get-NonNullItems {
  param([object]$Value)
  return @($Value | Where-Object { $null -ne $_ })
}

function Get-Sha256 {
  param([Parameter(Mandatory = $true)][string]$Path)
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-Sha256Value {
  param([object]$Value, [string]$Label)
  if ([string]$Value -cnotmatch '^[0-9a-f]{64}$') {
    throw "$Label is not a lowercase SHA-256 value."
  }
}

function Get-CleanSourceState {
  $commit = (& git -C $repoRoot rev-parse HEAD)
  if ($LASTEXITCODE -ne 0) { throw "Unable to read the lifecycle source commit." }
  $tree = (& git -C $repoRoot rev-parse 'HEAD^{tree}')
  if ($LASTEXITCODE -ne 0) { throw "Unable to read the lifecycle source tree." }
  $branch = (& git -C $repoRoot branch --show-current)
  if ($LASTEXITCODE -ne 0) { throw "Unable to read the lifecycle source branch." }
  $status = @(& git -C $repoRoot status --porcelain --untracked-files=all)
  if ($LASTEXITCODE -ne 0) { throw "Unable to prove that the lifecycle source tree is clean." }
  if ($status.Count -ne 0) {
    throw "The official lifecycle run requires a clean source tree. Commit or remove local changes first."
  }
  $commit = ([string]$commit).Trim()
  $tree = ([string]$tree).Trim()
  if ($commit -notmatch '^[0-9a-fA-F]{40,64}$' -or $tree -notmatch '^[0-9a-fA-F]{40,64}$') {
    throw "The lifecycle source commit/tree identity is malformed."
  }
  return [pscustomobject]@{
    commit = $commit.ToLowerInvariant()
    tree = $tree.ToLowerInvariant()
    branch = ([string]$branch).Trim()
  }
}

function New-LifecycleCommands {
  return @(
    @{ id = "01-clean-install-local-$candidateTag"; action = "install"; installer = $candidateLocalName; displayName = "Code Hangar"; expectedVersion = $CandidateVersion; expectedSidecar = $false },
    @{ id = "02-uninstall-provisioning-local"; action = "uninstall"; displayName = "Code Hangar"; expectCatalogPreserved = $false },
    @{ id = "03-install-baseline-local-$baselineTag"; action = "install"; installer = $baselineLocalName; displayName = "Code Hangar"; expectedVersion = $BaselineVersion; expectedSidecar = $false },
    @{ id = "04-launch-baseline-local-$baselineTag"; action = "launch"; displayName = "Code Hangar"; expectedVersion = $BaselineVersion; expectedSidecar = $false },
    @{ id = "05-close-baseline-local-$baselineTag"; action = "close" },
    @{ id = "06-register-baseline-catalog"; action = "catalog"; mode = "register"; helper = $baselineHelperName; project = "test-project" },
    @{ id = "08-check-baseline-catalog"; action = "catalog"; mode = "check"; helper = $baselineHelperName; project = "test-project" },
    @{ id = "09-upgrade-local-$candidateTag"; action = "install"; installer = $candidateLocalName; displayName = "Code Hangar"; expectedVersion = $CandidateVersion; expectedSidecar = $false },
    @{ id = "10-check-upgraded-catalog-$candidateTag"; action = "catalog"; mode = "check"; helper = $candidateHelperName; project = "test-project" },
    @{ id = "11-install-connector-$candidateTag"; action = "install"; installer = $candidateConnectorName; displayName = "Code Hangar AI Connector"; expectedVersion = $CandidateVersion; expectedSidecar = $true },
    @{ id = "12-launch-connector-$candidateTag"; action = "launch"; displayName = "Code Hangar AI Connector"; expectedVersion = $CandidateVersion; expectedSidecar = $true },
    @{ id = "13-close-connector-$candidateTag"; action = "close" },
    @{ id = "14-check-connector-catalog"; action = "catalog"; mode = "check"; helper = $candidateHelperName; project = "test-project" },
    @{ id = "15-repair-connector-$candidateTag"; action = "install"; installer = $candidateConnectorName; displayName = "Code Hangar AI Connector"; expectedVersion = $CandidateVersion; expectedSidecar = $true },
    @{ id = "16-uninstall-local"; action = "uninstall"; displayName = "Code Hangar"; expectCatalogPreserved = $true },
    @{ id = "17-launch-connector-after-local-uninstall"; action = "launch"; displayName = "Code Hangar AI Connector"; expectedVersion = $CandidateVersion; expectedSidecar = $true },
    @{ id = "18-close-connector-after-local-uninstall"; action = "close" },
    @{ id = "19-check-connector-only-catalog"; action = "catalog"; mode = "check"; helper = $candidateHelperName; project = "test-project" },
    @{ id = "20-uninstall-connector"; action = "uninstall"; displayName = "Code Hangar AI Connector"; expectCatalogPreserved = $true },
    @{ id = "21-reinstall-local-$candidateTag"; action = "install"; installer = $candidateLocalName; displayName = "Code Hangar"; expectedVersion = $CandidateVersion; expectedSidecar = $false },
    @{ id = "22-launch-reinstalled-local"; action = "launch"; displayName = "Code Hangar"; expectedVersion = $CandidateVersion; expectedSidecar = $false },
    @{ id = "23-close-reinstalled-local"; action = "close" },
    @{ id = "24-check-reinstalled-local-catalog"; action = "catalog"; mode = "check"; helper = $candidateHelperName; project = "test-project" },
    @{ id = "25-final-uninstall-local"; action = "uninstall"; displayName = "Code Hangar"; expectCatalogPreserved = $true },
    @{ id = "26-final-inspect"; action = "inspect" }
  )
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
  if (-not $ExistingOnly -and (Test-Path -LiteralPath $resolved)) {
    throw "A new lifecycle run requires a new EvidenceDir; refusing to reuse stale evidence at $resolved"
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
  if ([string]$result.id -cne $Id) {
    throw "Lifecycle result identity mismatch for $Id."
  }
  if ([string]$result.status -cne "PASS") {
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
    if ([string]$local.displayVersion -cne $ExpectedVersion -or [bool]$local.sidecarExists) {
      throw "Local edition version/sidecar invariant failed."
    }
  }
  if ($ExpectConnector) {
    $connector = Get-AppByName -State $State -DisplayName "Code Hangar AI Connector"
    if ([string]$connector.displayVersion -cne $ExpectedVersion -or -not [bool]$connector.sidecarExists) {
      throw "Connector edition version/sidecar invariant failed."
    }
  }
}

function Assert-InstalledArtifactInventory {
  param(
    [object]$App,
    [ValidateSet('Local', 'Connector')][string]$Edition,
    [string]$SetupSha256,
    [string]$ObservationResultId
  )
  Assert-Sha256Value -Value $SetupSha256 -Label "$Edition setup SHA-256"
  if ([string]$App.edition -cne $Edition -or [string]$App.displayVersion -cne $CandidateVersion) {
    throw "$Edition installed-artifact observation has the wrong edition/version."
  }
  $installRoot = [System.IO.Path]::GetFullPath([string]$App.installLocation).TrimEnd('\')
  if (-not [System.IO.Path]::IsPathFullyQualified($installRoot)) { throw "$Edition install root is not fully qualified." }
  $artifacts = @(Get-NonNullItems $App.artifacts)
  $expectedRoles = if ($Edition -eq 'Connector') { @('helper', 'mcp', 'parent', 'uninstaller') } else { @('helper', 'parent', 'uninstaller') }
  Assert-ExactStringSet -Actual @($artifacts | ForEach-Object { [string]$_.role }) -Expected $expectedRoles -Label "$Edition installed artifact roles"
  $records = [System.Collections.Generic.List[object]]::new()
  foreach ($artifact in $artifacts) {
    $properties = @($artifact.PSObject.Properties.Name | Sort-Object)
    if (($properties -join "`n") -cne ((@('bytes', 'canonicalPath', 'relativePath', 'role', 'sha256') | Sort-Object) -join "`n")) {
      throw "$Edition installed artifact $($artifact.role) has unexpected or missing fields."
    }
    Assert-Sha256Value -Value $artifact.sha256 -Label "$Edition installed $($artifact.role) SHA-256"
    if ([long]$artifact.bytes -le 0 -or -not [System.IO.Path]::IsPathFullyQualified([string]$artifact.canonicalPath)) {
      throw "$Edition installed artifact $($artifact.role) has invalid bytes/path identity."
    }
    $canonical = [System.IO.Path]::GetFullPath([string]$artifact.canonicalPath)
    $prefix = $installRoot + '\'
    if (-not $canonical.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "$Edition installed artifact $($artifact.role) escapes its install root."
    }
    $expectedRelative = [System.IO.Path]::GetRelativePath($installRoot, $canonical).Replace('\', '/')
    if ([string]$artifact.relativePath -cne $expectedRelative -or $expectedRelative.StartsWith('../', [System.StringComparison]::Ordinal)) {
      throw "$Edition installed artifact $($artifact.role) has a mismatched relative/canonical path identity."
    }
    $records.Add([ordered]@{
        role = [string]$artifact.role
        relativePath = [string]$artifact.relativePath
        canonicalPath = $canonical
        bytes = [long]$artifact.bytes
        sha256 = [string]$artifact.sha256
      })
  }
  return [ordered]@{
    edition = $Edition
    setupSha256 = $SetupSha256
    observationResultId = $ObservationResultId
    installLocation = $installRoot
    artifacts = @($records | Sort-Object role)
  }
}

function Assert-InstalledArtifactSetEqual {
  param([object]$Expected, [object]$Actual, [string]$Label)
  $expectedProjection = @($Expected.artifacts | ForEach-Object {
      "$($_.role)`t$($_.relativePath)`t$($_.bytes)`t$($_.sha256)"
    } | Sort-Object)
  $actualProjection = @($Actual.artifacts | ForEach-Object {
      "$($_.role)`t$($_.relativePath)`t$($_.bytes)`t$($_.sha256)"
    } | Sort-Object)
  if (($expectedProjection -join "`n") -cne ($actualProjection -join "`n")) {
    throw "$Label installed artifact bytes/path roles differ."
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

function Assert-ExactStringSet {
  param([string[]]$Actual, [string[]]$Expected, [string]$Label)
  $actualSorted = @($Actual | Sort-Object -Unique)
  $expectedSorted = @($Expected | Sort-Object -Unique)
  if ($Actual.Count -ne $Expected.Count -or
      ($actualSorted -join "`n") -cne ($expectedSorted -join "`n")) {
    throw "$Label differs. Expected [$($expectedSorted -join ', ')], found [$($actualSorted -join ', ')]."
  }
}

function Get-ExpectedSharedInputNames {
  $commandNames = @(New-LifecycleCommands | ForEach-Object { "commands\$($_.id).json" })
  return @(
    $baselineLocalName,
    $candidateLocalName,
    $candidateConnectorName,
    $baselineHelperName,
    $candidateHelperName,
    "sandbox-lifecycle-agent.ps1",
    "VCRUNTIME140.dll",
    "test-project\README.md",
    "test-project\AGENTS.md",
    "test-project\src\main.rs"
  ) + $commandNames
}

function Read-And-ValidateSourceProvenance {
  param([Parameter(Mandatory = $true)][string]$Root)

  $sharedDir = Join-Path $Root "shared"
  # Keep the trust anchor outside the writable folder mapped into Windows
  # Sandbox. The guest may write results under shared, but it must never be able
  # to rewrite the host's expected hashes to make changed inputs look valid.
  $provenancePath = Join-Path $Root "source-provenance.json"
  if (-not (Test-Path -LiteralPath $provenancePath -PathType Leaf)) {
    throw "Lifecycle source provenance is missing."
  }
  $provenance = Get-Content -LiteralPath $provenancePath -Raw | ConvertFrom-Json
  if ([int]$provenance.schemaVersion -ne 2) {
    throw "Lifecycle source provenance schema is not the v0.1.3 fail-closed schema."
  }
  if ([bool]$provenance.sourceTreeDirty) {
    throw "Lifecycle source provenance reports a dirty tree."
  }
  if ([string]$provenance.baselineVersion -cne $BaselineVersion -or [string]$provenance.candidateVersion -cne $CandidateVersion) {
    throw "Lifecycle version parameters do not match the recorded source provenance."
  }
  if ([string]$provenance.gitCommit -cnotmatch '^[0-9a-f]{40,64}$' -or [string]$provenance.gitTree -cnotmatch '^[0-9a-f]{40,64}$') {
    throw "Lifecycle source provenance has a malformed Git commit/tree identity."
  }

  foreach ($property in @(
    "baselineLocalSha256",
    "candidateLocalSha256",
    "candidateConnectorSha256",
    "baselineCatalogHelperSha256",
    "candidateCatalogHelperSha256"
  )) {
    if (-not ($provenance.PSObject.Properties.Name -contains $property)) {
      throw "Lifecycle source provenance is missing $property."
    }
    Assert-Sha256Value -Value $provenance.$property -Label $property
  }

  $inputs = @(Get-NonNullItems $provenance.sharedInputs)
  $expectedNames = @(Get-ExpectedSharedInputNames)
  Assert-ExactStringSet -Actual @($inputs | ForEach-Object { [string]$_.path }) -Expected $expectedNames -Label "Lifecycle shared-input inventory"
  foreach ($input in $inputs) {
    $relative = [string]$input.path
    Assert-Sha256Value -Value $input.sha256 -Label "Shared input $relative"
    $fullPath = [System.IO.Path]::GetFullPath((Join-Path $sharedDir $relative))
    $sharedPrefix = $sharedDir.TrimEnd("\") + "\"
    if (-not $fullPath.StartsWith($sharedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Shared input path escapes lifecycle evidence: $relative"
    }
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
      throw "Lifecycle shared input is missing: $relative"
    }
    if ((Get-Sha256 -Path $fullPath) -cne [string]$input.sha256) {
      throw "Lifecycle shared input hash changed: $relative"
    }
  }

  $inputHashByPath = @{}
  foreach ($input in $inputs) { $inputHashByPath[[string]$input.path] = [string]$input.sha256 }
  $directBindings = @(
    [pscustomobject]@{ path = $baselineLocalName; sha256 = [string]$provenance.baselineLocalSha256 },
    [pscustomobject]@{ path = $candidateLocalName; sha256 = [string]$provenance.candidateLocalSha256 },
    [pscustomobject]@{ path = $candidateConnectorName; sha256 = [string]$provenance.candidateConnectorSha256 },
    [pscustomobject]@{ path = $baselineHelperName; sha256 = [string]$provenance.baselineCatalogHelperSha256 },
    [pscustomobject]@{ path = $candidateHelperName; sha256 = [string]$provenance.candidateCatalogHelperSha256 }
  )
  foreach ($binding in $directBindings) {
    if ([string]$inputHashByPath[$binding.path] -cne [string]$binding.sha256) {
      throw "Lifecycle direct hash binding disagrees with shared input $($binding.path)."
    }
  }

  $actualCommandNames = @(
    Get-ChildItem -LiteralPath (Join-Path $sharedDir "commands") -Filter "*.json" -File |
      ForEach-Object { $_.Name }
  )
  $expectedCommandNames = @(New-LifecycleCommands | ForEach-Object { "$($_.id).json" })
  Assert-ExactStringSet -Actual $actualCommandNames -Expected $expectedCommandNames -Label "Lifecycle command inventory"
  return $provenance
}

function New-LifecycleManifest {
  param([string]$Root, [switch]$DoNotWrite)

  $sharedDir = Join-Path $Root "shared"
  $resultsDir = Join-Path $sharedDir "results"
  $checks = [System.Collections.Generic.List[object]]::new()
  $resultSummaries = [System.Collections.Generic.List[object]]::new()
  $failure = $null
  $provenance = $null
  $installedArtifacts = $null

  try {
    $provenance = Read-And-ValidateSourceProvenance -Root $Root
    $checks.Add([pscustomobject]@{
      name = "source-provenance"
      status = "PASS"
      gitCommit = [string]$provenance.gitCommit
      candidateVersion = [string]$provenance.candidateVersion
    })

    $agentReady = Get-Content -LiteralPath (Join-Path $resultsDir "agent-ready.json") -Raw | ConvertFrom-Json
    $agentStopped = Get-Content -LiteralPath (Join-Path $resultsDir "agent-stopped.json") -Raw | ConvertFrom-Json
    if ([string]$agentReady.status -cne "PASS" -or [string]$agentStopped.status -cne "PASS") {
      throw "The guest lifecycle agent did not start and stop cleanly."
    }
    $checks.Add([pscustomobject]@{ name = "guest-agent-lifecycle"; status = "PASS" })

    $resultFiles = @(Get-ChildItem -LiteralPath $resultsDir -Filter "*.json" -File | Sort-Object Name)
    $observedResultIds = [System.Collections.Generic.List[string]]::new()
    foreach ($file in $resultFiles) {
      $value = Get-Content -LiteralPath $file.FullName -Raw | ConvertFrom-Json
      if (-not ($value.PSObject.Properties.Name -contains "id")) { continue }
      if ([string]$value.id -cne $file.BaseName) {
        throw "Lifecycle result filename/identity mismatch: $($file.Name) contains $($value.id)."
      }
      $observedResultIds.Add([string]$value.id)
      $resultSummaries.Add([pscustomobject]@{
        id = [string]$value.id
        status = [string]$value.status
        startedAt = [string]$value.startedAt
        completedAt = [string]$value.completedAt
      })
      if ([string]$value.status -cne "PASS") {
        throw "A non-passing lifecycle result is never publication evidence: $($value.id)"
      }
    }
    $expectedResultIds = @(New-LifecycleCommands | ForEach-Object { [string]$_.id })
    Assert-ExactStringSet -Actual @($observedResultIds) -Expected $expectedResultIds -Label "Lifecycle result inventory"

    $cleanInstall = Get-LifecycleResult $resultsDir "01-clean-install-local-$candidateTag"
    $cleanBeforeApps = @(Get-NonNullItems $cleanInstall.detail.before.applications)
    if ($cleanBeforeApps.Count -ne 0 -or [bool]$cleanInstall.detail.before.catalog.keyExists) {
      throw "The clean-install proof did not start from an empty app/catalog-key state."
    }
    Assert-AppSet $cleanInstall.detail.after $true $false $CandidateVersion
    if ([string]$cleanInstall.detail.installerSha256 -cne [string]$provenance.candidateLocalSha256) {
      throw "The clean-install Local artifact hash does not match source provenance."
    }
    $localInstalled = Assert-InstalledArtifactInventory `
      -App $cleanInstall.detail.selectedApp `
      -Edition Local `
      -SetupSha256 ([string]$provenance.candidateLocalSha256) `
      -ObservationResultId ([string]$cleanInstall.id)
    $checks.Add([pscustomobject]@{ name = "clean-offline-local-install"; status = "PASS"; installerSha256 = [string]$cleanInstall.detail.installerSha256 })

    $provisioningUninstall = Get-LifecycleResult $resultsDir "02-uninstall-provisioning-local"
    Assert-AppSet $provisioningUninstall.detail.after $false $false $CandidateVersion

    $baselineInstall = Get-LifecycleResult $resultsDir "03-install-baseline-local-$baselineTag"
    Assert-AppSet $baselineInstall.detail.after $true $false $BaselineVersion
    if ([string]$baselineInstall.detail.installerSha256 -cne [string]$provenance.baselineLocalSha256) {
      throw "The baseline Local artifact hash does not match source provenance."
    }
    Get-LifecycleResult $resultsDir "04-launch-baseline-local-$baselineTag" | Out-Null
    Get-LifecycleResult $resultsDir "05-close-baseline-local-$baselineTag" | Out-Null

    $baselineRegister = Get-LifecycleResult $resultsDir "06-register-baseline-catalog"
    if ([string]$baselineRegister.detail.helperSha256 -cne [string]$provenance.baselineCatalogHelperSha256) {
      throw "The baseline registration helper hash does not match source provenance."
    }
    $baselineCatalog = Get-LifecycleResult $resultsDir "08-check-baseline-catalog"
    if ([string]$baselineCatalog.detail.helperSha256 -cne [string]$provenance.baselineCatalogHelperSha256) {
      throw "The baseline catalog helper hash does not match source provenance."
    }
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

    $upgrade = Get-LifecycleResult $resultsDir "09-upgrade-local-$candidateTag"
    Assert-AppSet $upgrade.detail.after $true $false $CandidateVersion
    if ($upgrade.detail.before.catalog.keySha256 -ne $catalogKey -or $upgrade.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "The encrypted catalog key changed during upgrade."
    }
    $upgradedCatalog = Get-LifecycleResult $resultsDir "10-check-upgraded-catalog-$candidateTag"
    if ([string]$upgradedCatalog.detail.helperSha256 -cne [string]$provenance.candidateCatalogHelperSha256) {
      throw "The upgraded-catalog helper hash does not match source provenance."
    }
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $upgradedCatalog) "upgrade"
    if ($upgradedCatalog.detail.state.catalog.keySha256 -ne $catalogKey) {
      throw "The upgraded catalog key hash changed."
    }
    $checks.Add([pscustomobject]@{ name = "upgrade-$BaselineVersion-to-$CandidateVersion"; status = "PASS" })

    $connectorInstall = Get-LifecycleResult $resultsDir "11-install-connector-$candidateTag"
    Assert-AppSet $connectorInstall.detail.after $true $true $CandidateVersion
    if ([string]$connectorInstall.detail.installerSha256 -cne [string]$provenance.candidateConnectorSha256) {
      throw "The Connector artifact hash does not match source provenance."
    }
    $connectorInstalled = Assert-InstalledArtifactInventory `
      -App $connectorInstall.detail.selectedApp `
      -Edition Connector `
      -SetupSha256 ([string]$provenance.candidateConnectorSha256) `
      -ObservationResultId ([string]$connectorInstall.id)
    if ($connectorInstall.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "The catalog key changed when Connector was installed."
    }
    Get-LifecycleResult $resultsDir "12-launch-connector-$candidateTag" | Out-Null
    Get-LifecycleResult $resultsDir "13-close-connector-$candidateTag" | Out-Null
    $connectorCatalog = Get-LifecycleResult $resultsDir "14-check-connector-catalog"
    if ([string]$connectorCatalog.detail.helperSha256 -cne [string]$provenance.candidateCatalogHelperSha256) {
      throw "The Connector catalog helper hash does not match source provenance."
    }
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $connectorCatalog) "Connector coexistence"

    $repair = Get-LifecycleResult $resultsDir "15-repair-connector-$candidateTag"
    Assert-AppSet $repair.detail.before $true $true $CandidateVersion
    Assert-AppSet $repair.detail.after $true $true $CandidateVersion
    if ($repair.detail.before.catalog.keySha256 -ne $catalogKey -or $repair.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Connector repair changed the catalog key."
    }
    $connectorRepairInstalled = Assert-InstalledArtifactInventory `
      -App $repair.detail.selectedApp `
      -Edition Connector `
      -SetupSha256 ([string]$provenance.candidateConnectorSha256) `
      -ObservationResultId ([string]$repair.id)
    Assert-InstalledArtifactSetEqual -Expected $connectorInstalled -Actual $connectorRepairInstalled -Label 'Connector repair'
    $checks.Add([pscustomobject]@{ name = "edition-coexistence-and-repair"; status = "PASS" })

    $removeLocal = Get-LifecycleResult $resultsDir "16-uninstall-local"
    Assert-AppSet $removeLocal.detail.after $false $true $CandidateVersion
    if ($removeLocal.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Uninstalling Local changed the shared catalog key."
    }
    Get-LifecycleResult $resultsDir "17-launch-connector-after-local-uninstall" | Out-Null
    Get-LifecycleResult $resultsDir "18-close-connector-after-local-uninstall" | Out-Null
    $connectorOnlyCatalog = Get-LifecycleResult $resultsDir "19-check-connector-only-catalog"
    if ([string]$connectorOnlyCatalog.detail.helperSha256 -cne [string]$provenance.candidateCatalogHelperSha256) {
      throw "The Connector-only catalog helper hash does not match source provenance."
    }
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $connectorOnlyCatalog) "Connector-only state"

    $removeConnector = Get-LifecycleResult $resultsDir "20-uninstall-connector"
    Assert-AppSet $removeConnector.detail.after $false $false $CandidateVersion
    if ($removeConnector.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Uninstalling Connector changed the shared catalog key."
    }

    $reinstallLocal = Get-LifecycleResult $resultsDir "21-reinstall-local-$candidateTag"
    Assert-AppSet $reinstallLocal.detail.after $true $false $CandidateVersion
    if ($reinstallLocal.detail.after.catalog.keySha256 -ne $catalogKey) {
      throw "Reinstalling Local changed the shared catalog key."
    }
    $localReinstalled = Assert-InstalledArtifactInventory `
      -App $reinstallLocal.detail.selectedApp `
      -Edition Local `
      -SetupSha256 ([string]$provenance.candidateLocalSha256) `
      -ObservationResultId ([string]$reinstallLocal.id)
    Assert-InstalledArtifactSetEqual -Expected $localInstalled -Actual $localReinstalled -Label 'Local reinstall'
    Get-LifecycleResult $resultsDir "22-launch-reinstalled-local" | Out-Null
    Get-LifecycleResult $resultsDir "23-close-reinstalled-local" | Out-Null
    $reinstalledCatalog = Get-LifecycleResult $resultsDir "24-check-reinstalled-local-catalog"
    if ([string]$reinstalledCatalog.detail.helperSha256 -cne [string]$provenance.candidateCatalogHelperSha256) {
      throw "The reinstalled-Local catalog helper hash does not match source provenance."
    }
    Assert-CatalogIdentity $catalogSignature (Get-CatalogSignature $reinstalledCatalog) "Local reinstall"

    $finalUninstall = Get-LifecycleResult $resultsDir "25-final-uninstall-local"
    Assert-AppSet $finalUninstall.detail.after $false $false $CandidateVersion
    $finalInspect = Get-LifecycleResult $resultsDir "26-final-inspect"
    Assert-AppSet $finalInspect.detail.state $false $false $CandidateVersion
    if ($finalInspect.detail.state.catalog.keySha256 -ne $catalogKey) {
      throw "The final empty-install state did not preserve the shared catalog key."
    }
    Assert-AppSet $agentStopped.state $false $false $CandidateVersion
    if ($agentStopped.state.catalog.keySha256 -ne $catalogKey) {
      throw "The stopped guest agent observed a different final catalog key."
    }
    $checks.Add([pscustomobject]@{ name = "uninstall-switching-and-final-state"; status = "PASS" })
    $installedArtifacts = [ordered]@{ Local = $localInstalled; Connector = $connectorInstalled }
  } catch {
    $failure = $_
    $checks.Add([pscustomobject]@{ name = "validation"; status = "FAIL"; error = $_.Exception.Message })
  }

  $manifestCommit = if ($null -eq $provenance) { $null } else { [string]$provenance.gitCommit }
  $manifestBranch = if ($null -eq $provenance) { $null } else { [string]$provenance.gitBranch }
  $manifest = [pscustomobject]@{
    schemaVersion = 3
    documentType = 'codehangar/sandbox-lifecycle/3'
    generatedAt = (Get-Date).ToString("o")
    evidenceRoot = $Root
    machine = $env:COMPUTERNAME
    gitCommit = $manifestCommit
    gitBranch = $manifestBranch
    baselineVersion = $BaselineVersion
    candidateVersion = $CandidateVersion
    status = if ($null -eq $failure) { "PASS" } else { "FAIL" }
    checks = @($checks)
    results = @($resultSummaries)
    historicalFailuresAccepted = $false
    sourceProvenance = $provenance
    installedArtifacts = $installedArtifacts
  }
  if (-not $DoNotWrite) {
    Write-JsonFile -Path (Join-Path $Root "lifecycle-manifest.json") -Value $manifest
  }
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
  $commandIds = @(New-LifecycleCommands | ForEach-Object { [string]$_.id })
  if ($commandIds -contains "07-register-baseline-catalog" -or $commandIds.Count -ne 25) {
    throw "The authoritative lifecycle command inventory changed unexpectedly."
  }
  if ($baselineLocalName -ne "Code Hangar_${BaselineVersion}_x64-setup.exe" -or
      $candidateConnectorName -ne "Code Hangar AI Connector_${CandidateVersion}_x64-setup.exe") {
    throw "Lifecycle version interpolation changed unexpectedly."
  }
  $setMismatchRejected = $false
  try {
    Assert-ExactStringSet -Actual @("one") -Expected @("one", "two") -Label "self-test"
  } catch {
    $setMismatchRejected = $true
  }
  if (-not $setMismatchRejected) { throw "Exact inventory validation accepted a missing item." }
  $badHashRejected = $false
  try {
    Assert-Sha256Value -Value ("A" * 64) -Label "self-test"
  } catch {
    $badHashRejected = $true
  }
  if (-not $badHashRejected) { throw "SHA-256 validation accepted a non-canonical value." }

  $selfTestApp = [pscustomobject]@{
    edition = 'Connector'
    displayVersion = $CandidateVersion
    installLocation = 'C:\CodeHangarLifecycleSelfTest\Connector'
    artifacts = @(
      [pscustomobject]@{ role = 'parent'; relativePath = 'code-hangar-desktop.exe'; canonicalPath = 'C:\CodeHangarLifecycleSelfTest\Connector\code-hangar-desktop.exe'; bytes = 101; sha256 = ('11' * 32) },
      [pscustomobject]@{ role = 'helper'; relativePath = 'code-hangar-elevated.exe'; canonicalPath = 'C:\CodeHangarLifecycleSelfTest\Connector\code-hangar-elevated.exe'; bytes = 102; sha256 = ('22' * 32) },
      [pscustomobject]@{ role = 'mcp'; relativePath = 'code-hangar-mcp.exe'; canonicalPath = 'C:\CodeHangarLifecycleSelfTest\Connector\code-hangar-mcp.exe'; bytes = 103; sha256 = ('33' * 32) },
      [pscustomobject]@{ role = 'uninstaller'; relativePath = 'uninstall.exe'; canonicalPath = 'C:\CodeHangarLifecycleSelfTest\Connector\uninstall.exe'; bytes = 104; sha256 = ('44' * 32) }
    )
  }
  $validatedInstalled = Assert-InstalledArtifactInventory -App $selfTestApp -Edition Connector -SetupSha256 ('55' * 32) -ObservationResultId 'self-test-install'
  $substituted = $validatedInstalled | ConvertTo-Json -Depth 8 | ConvertFrom-Json
  $substituted.artifacts[2].sha256 = ('66' * 32)
  $substitutionRejected = $false
  try { Assert-InstalledArtifactSetEqual -Expected $validatedInstalled -Actual $substituted -Label 'Self-test substitution' } catch { $substitutionRejected = $true }
  if (-not $substitutionRejected) { throw 'Installed-artifact substitution self-test did not fail closed.' }
  $escapedApp = $selfTestApp | ConvertTo-Json -Depth 8 | ConvertFrom-Json
  $escapedApp.artifacts[0].canonicalPath = 'C:\foreign-build\code-hangar-desktop.exe'
  $pathSubstitutionRejected = $false
  try { [void](Assert-InstalledArtifactInventory -App $escapedApp -Edition Connector -SetupSha256 ('55' * 32) -ObservationResultId 'self-test-install') } catch { $pathSubstitutionRejected = $true }
  if (-not $pathSubstitutionRejected) { throw 'Installed-artifact path-substitution self-test did not fail closed.' }

  $selfTestRoot = [System.IO.Path]::GetFullPath((Join-Path $acceptanceRoot ("selftest-" + [guid]::NewGuid().ToString("N"))))
  try {
    $shared = Join-Path $selfTestRoot "shared"
    New-Item -ItemType Directory -Path (Join-Path $shared "commands"), (Join-Path $shared "test-project\src") -Force | Out-Null
    foreach ($command in @(New-LifecycleCommands)) {
      Write-JsonFile -Path (Join-Path $shared "commands\$($command.id).json") -Value $command
    }
    foreach ($relative in @(Get-ExpectedSharedInputNames | Where-Object { -not $_.StartsWith("commands\") })) {
      $path = Join-Path $shared $relative
      New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
      "self-test:$relative" | Set-Content -LiteralPath $path -Encoding utf8
    }
    $sharedInputs = @(Get-ExpectedSharedInputNames | ForEach-Object {
      [pscustomobject]@{ path = $_; sha256 = Get-Sha256 -Path (Join-Path $shared $_) }
    })
    $hashByPath = @{}
    foreach ($input in $sharedInputs) { $hashByPath[$input.path] = $input.sha256 }
    $provenance = [pscustomobject]@{
      schemaVersion = 2
      recordedAt = "2026-01-01T00:00:00Z"
      gitCommit = "a" * 40
      gitTree = "b" * 40
      gitBranch = "codex/self-test"
      sourceTreeDirty = $false
      baselineVersion = $BaselineVersion
      candidateVersion = $CandidateVersion
      baselineLocalSha256 = $hashByPath[$baselineLocalName]
      candidateLocalSha256 = $hashByPath[$candidateLocalName]
      candidateConnectorSha256 = $hashByPath[$candidateConnectorName]
      baselineCatalogHelperSha256 = $hashByPath[$baselineHelperName]
      candidateCatalogHelperSha256 = $hashByPath[$candidateHelperName]
      sharedInputs = $sharedInputs
    }
    Write-JsonFile -Path (Join-Path $selfTestRoot "source-provenance.json") -Value $provenance
    if (Test-Path -LiteralPath (Join-Path $shared "source-provenance.json")) {
      throw "The source-provenance trust anchor must not be exposed in the writable Sandbox share."
    }
    Read-And-ValidateSourceProvenance -Root $selfTestRoot | Out-Null

    Add-Content -LiteralPath (Join-Path $shared $candidateLocalName) -Value "tampered"
    $tamperRejected = $false
    try {
      Read-And-ValidateSourceProvenance -Root $selfTestRoot | Out-Null
    } catch {
      $tamperRejected = $true
    }
    if (-not $tamperRejected) { throw "Source provenance accepted a changed shared artifact." }
  } finally {
    $allowedSelfTestPrefix = $acceptanceRoot.TrimEnd("\") + "\selftest-"
    if (-not $selfTestRoot.StartsWith($allowedSelfTestPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Refusing to clean an unexpected self-test path: $selfTestRoot"
    }
    if (Test-Path -LiteralPath $selfTestRoot) {
      Remove-Item -LiteralPath $selfTestRoot -Recurse -Force
    }
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
  # Recompute every lifecycle invariant without rewriting the sealed manifest.
  # Callers may hold a write/delete-denying handle to that exact manifest while
  # this subprocess validates the underlying result set.
  $manifest = New-LifecycleManifest -Root $EvidenceDir -DoNotWrite
  Write-Host "Sandbox lifecycle evidence passed: $EvidenceDir" -ForegroundColor Green
  $manifest | ConvertTo-Json -Depth 4
  exit 0
}

if ($Resume) {
  # Authenticate the evidence/version/artifact set before waiting on it or writing
  # stop.flag. A stale or mismatched directory must remain completely untouched.
  Read-And-ValidateSourceProvenance -Root $EvidenceDir | Out-Null
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

$sourceStateBefore = Get-CleanSourceState

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

Copy-Artifact $BaselineLocalInstaller (Join-Path $sharedDir $baselineLocalName)
Copy-Artifact $CandidateLocalInstaller (Join-Path $sharedDir $candidateLocalName)
Copy-Artifact $CandidateConnectorInstaller (Join-Path $sharedDir $candidateConnectorName)
Copy-Artifact $BaselineCatalogHelper (Join-Path $sharedDir $baselineHelperName)
Copy-Artifact $CandidateCatalogHelper (Join-Path $sharedDir $candidateHelperName)
Copy-Artifact (Join-Path $PSScriptRoot "sandbox-lifecycle-agent.ps1") (Join-Path $sharedDir "sandbox-lifecycle-agent.ps1")
Copy-Artifact (Join-Path $env:windir "System32\VCRUNTIME140.dll") (Join-Path $sharedDir "VCRUNTIME140.dll")

"# Sandbox lifecycle fixture" | Set-Content -LiteralPath (Join-Path $sharedDir "test-project\README.md") -Encoding utf8
"# Acceptance agent context" | Set-Content -LiteralPath (Join-Path $sharedDir "test-project\AGENTS.md") -Encoding utf8
"fn main() {}" | Set-Content -LiteralPath (Join-Path $sharedDir "test-project\src\main.rs") -Encoding utf8

$commands = @(New-LifecycleCommands)

foreach ($command in $commands) {
  Write-JsonFile -Path (Join-Path $commandsDir "$($command.id).json") -Value $command
}

$baselineLocalInputHash = Get-Sha256 -Path $BaselineLocalInstaller
$candidateLocalInputHash = Get-Sha256 -Path $CandidateLocalInstaller
$candidateConnectorInputHash = Get-Sha256 -Path $CandidateConnectorInstaller
$baselineHelperInputHash = Get-Sha256 -Path $BaselineCatalogHelper
$candidateHelperInputHash = Get-Sha256 -Path $CandidateCatalogHelper
$sharedInputs = @(Get-ExpectedSharedInputNames | ForEach-Object {
  $inputPath = Join-Path $sharedDir $_
  if (-not (Test-Path -LiteralPath $inputPath -PathType Leaf)) {
    throw "Lifecycle shared input was not staged: $_"
  }
  [pscustomobject]@{ path = $_; sha256 = Get-Sha256 -Path $inputPath }
})
$stagedHashByPath = @{}
foreach ($input in $sharedInputs) { $stagedHashByPath[$input.path] = $input.sha256 }
foreach ($binding in @(
  [pscustomobject]@{ path = $baselineLocalName; expected = $baselineLocalInputHash },
  [pscustomobject]@{ path = $candidateLocalName; expected = $candidateLocalInputHash },
  [pscustomobject]@{ path = $candidateConnectorName; expected = $candidateConnectorInputHash },
  [pscustomobject]@{ path = $baselineHelperName; expected = $baselineHelperInputHash },
  [pscustomobject]@{ path = $candidateHelperName; expected = $candidateHelperInputHash }
)) {
  if ([string]$stagedHashByPath[$binding.path] -cne [string]$binding.expected) {
    throw "Lifecycle input changed while it was staged: $($binding.path)"
  }
}

$sourceStateAfter = Get-CleanSourceState
if ($sourceStateAfter.commit -cne $sourceStateBefore.commit -or $sourceStateAfter.tree -cne $sourceStateBefore.tree) {
  throw "The source commit/tree changed while lifecycle inputs were being staged."
}
$sourceProvenance = [pscustomobject]@{
  schemaVersion = 2
  recordedAt = (Get-Date).ToString("o")
  gitCommit = $sourceStateAfter.commit
  gitTree = $sourceStateAfter.tree
  gitBranch = $sourceStateAfter.branch
  sourceTreeDirty = $false
  baselineVersion = $BaselineVersion
  candidateVersion = $CandidateVersion
  baselineLocalSha256 = $baselineLocalInputHash
  candidateLocalSha256 = $candidateLocalInputHash
  candidateConnectorSha256 = $candidateConnectorInputHash
  baselineCatalogHelperSha256 = $baselineHelperInputHash
  candidateCatalogHelperSha256 = $candidateHelperInputHash
  sharedInputs = $sharedInputs
}
Write-JsonFile -Path (Join-Path $EvidenceDir "source-provenance.json") -Value $sourceProvenance
Read-And-ValidateSourceProvenance -Root $EvidenceDir | Out-Null

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
