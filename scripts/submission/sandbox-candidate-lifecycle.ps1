[CmdletBinding()]
param(
  [string]$EvidenceDir,
  [string]$LocalInstaller,
  [string]$ConnectorInstaller,
  [ValidateSet("Both", "Local", "Connector")][string]$Edition = "Both",
  [ValidateRange(120, 1800)][int]$TimeoutSeconds = 900
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$acceptanceRoot = [System.IO.Path]::GetFullPath(
  (Join-Path $repoRoot ".local\acceptance\build-week-sandbox")
)
$releaseRoot = Join-Path $repoRoot "target\release\bundle\nsis\release-assets"

if ([string]::IsNullOrWhiteSpace($EvidenceDir)) {
  $EvidenceDir = Join-Path $acceptanceRoot (Get-Date -Format "yyyyMMdd-HHmmss")
}
$EvidenceDir = [System.IO.Path]::GetFullPath($EvidenceDir)
$allowedPrefix = $acceptanceRoot.TrimEnd("\") + "\"
if (-not $EvidenceDir.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "EvidenceDir must stay under $acceptanceRoot"
}
if (Test-Path -LiteralPath $EvidenceDir) {
  throw "Refusing to overwrite existing Sandbox evidence: $EvidenceDir"
}

if ([string]::IsNullOrWhiteSpace($LocalInstaller)) {
  $LocalInstaller = Join-Path $releaseRoot "Code-Hangar_0.1.2_x64-setup.exe"
}
if ([string]::IsNullOrWhiteSpace($ConnectorInstaller)) {
  $ConnectorInstaller = Join-Path $releaseRoot "Code-Hangar-AI-Connector_0.1.2_x64-setup.exe"
}
foreach ($input in @(
  @{ Label = "LocalInstaller"; Path = $LocalInstaller },
  @{ Label = "ConnectorInstaller"; Path = $ConnectorInstaller }
)) {
  if (-not (Test-Path -LiteralPath $input.Path -PathType Leaf)) {
    throw "$($input.Label) does not exist: $($input.Path)"
  }
}

if (Get-Process -Name WindowsSandboxRemoteSession -ErrorAction SilentlyContinue) {
  throw "Close the existing Windows Sandbox before starting a candidate lifecycle run."
}
$windowsSandbox = Join-Path $env:windir "System32\WindowsSandbox.exe"
if (-not (Test-Path -LiteralPath $windowsSandbox -PathType Leaf)) {
  throw "Windows Sandbox is not installed at $windowsSandbox"
}

function Write-JsonFile {
  param([string]$Path, [object]$Value)
  $json = $Value | ConvertTo-Json -Depth 12
  [System.IO.File]::WriteAllText($Path, $json, [System.Text.UTF8Encoding]::new($false))
}

function Wait-ForGuestPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][datetime]$Deadline,
    [Parameter(Mandatory = $true)][string]$Description
  )

  $startedAt = Get-Date
  while (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    $now = Get-Date
    if ($now -ge $Deadline) {
      throw "Timed out waiting for $Description at $Path"
    }
    $running = $null -ne (
      Get-Process -Name WindowsSandboxRemoteSession -ErrorAction SilentlyContinue
    )
    if (-not $running -and ($now - $startedAt).TotalSeconds -ge 20) {
      throw "Windows Sandbox exited while waiting for $Description."
    }
    Start-Sleep -Milliseconds 500
  }
}

function Get-Result {
  param([string]$ResultsDir, [string]$Id)
  $path = Join-Path $ResultsDir "$Id.json"
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Missing lifecycle result: $Id"
  }
  return Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
}

function Get-Application {
  param([object]$State, [string]$DisplayName)
  $matches = @($State.applications | Where-Object displayName -eq $DisplayName)
  if ($matches.Count -ne 1) {
    throw "Expected exactly one '$DisplayName' application, found $($matches.Count)."
  }
  return $matches[0]
}

$sharedDir = Join-Path $EvidenceDir "shared"
$commandsDir = Join-Path $sharedDir "commands"
$resultsDir = Join-Path $sharedDir "results"
New-Item -ItemType Directory -Path $commandsDir, $resultsDir -Force | Out-Null

$localName = "Code-Hangar_0.1.2_x64-setup.exe"
$connectorName = "Code-Hangar-AI-Connector_0.1.2_x64-setup.exe"
Copy-Item -LiteralPath ([System.IO.Path]::GetFullPath($LocalInstaller)) `
  -Destination (Join-Path $sharedDir $localName)
Copy-Item -LiteralPath ([System.IO.Path]::GetFullPath($ConnectorInstaller)) `
  -Destination (Join-Path $sharedDir $connectorName)
Copy-Item -LiteralPath (Join-Path $repoRoot "scripts\sandbox-lifecycle-agent.ps1") `
  -Destination (Join-Path $sharedDir "sandbox-lifecycle-agent.ps1")

$artifactIdentity = [ordered]@{
  local = [ordered]@{
    file = $localName
    bytes = (Get-Item -LiteralPath $LocalInstaller).Length
    sha256 = (Get-FileHash -LiteralPath $LocalInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
  }
  connector = [ordered]@{
    file = $connectorName
    bytes = (Get-Item -LiteralPath $ConnectorInstaller).Length
    sha256 = (Get-FileHash -LiteralPath $ConnectorInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
  }
}

$allCommands = @(
  @{ id = "00-initial-inspect"; action = "inspect" },
  @{ id = "01-install-local-012"; action = "install"; installer = $localName; displayName = "Code Hangar"; expectedVersion = "0.1.2"; expectedSidecar = $false },
  @{ id = "02-launch-local-012"; action = "launch"; displayName = "Code Hangar"; expectedVersion = "0.1.2"; expectedSidecar = $false },
  @{ id = "03-close-local-012"; action = "close" },
  @{ id = "04-uninstall-local-012"; action = "uninstall"; displayName = "Code Hangar"; expectCatalogPreserved = $false },
  @{ id = "05-after-local-inspect"; action = "inspect" },
  @{ id = "06-install-connector-012"; action = "install"; installer = $connectorName; displayName = "Code Hangar AI Connector"; expectedVersion = "0.1.2"; expectedSidecar = $true },
  @{ id = "07-launch-connector-012"; action = "launch"; displayName = "Code Hangar AI Connector"; expectedVersion = "0.1.2"; expectedSidecar = $true },
  @{ id = "08-close-connector-012"; action = "close" },
  @{ id = "09-uninstall-connector-012"; action = "uninstall"; displayName = "Code Hangar AI Connector"; expectCatalogPreserved = $false },
  @{ id = "10-final-inspect"; action = "inspect" }
)
$commands = switch ($Edition) {
  "Local" {
    @($allCommands | Where-Object { $_.id -match "^(00|0[1-5]|10)-" })
  }
  "Connector" {
    @($allCommands | Where-Object { $_.id -match "^(00|0[6-9]|10)-" })
  }
  default { @($allCommands) }
}

$hostFolder = [System.Security.SecurityElement]::Escape($sharedDir)
$sandboxConfig = "<Configuration><VGpu>Disable</VGpu><Networking>Disable</Networking><MappedFolders><MappedFolder><HostFolder>$hostFolder</HostFolder><SandboxFolder>C:\CodeHangarAcceptance</SandboxFolder><ReadOnly>false</ReadOnly></MappedFolder></MappedFolders><LogonCommand><Command>powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -Command `"`$env:CODEHANGAR_SANDBOX_AGENT='1'; C:\CodeHangarAcceptance\sandbox-lifecycle-agent.ps1 -SharedRoot C:\CodeHangarAcceptance`"</Command></LogonCommand></Configuration>"
$sandboxPath = Join-Path $EvidenceDir "candidate-lifecycle.wsb"
[System.IO.File]::WriteAllText(
  $sandboxPath,
  $sandboxConfig,
  [System.Text.UTF8Encoding]::new($false)
)

Start-Process -FilePath $windowsSandbox -ArgumentList "`"$sandboxPath`"" | Out-Null
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$stopWritten = $false
$commandFailure = $null
$finalInspectId = "10-final-inspect"
try {
  Wait-ForGuestPath -Path (Join-Path $resultsDir "agent-ready.json") `
    -Deadline $deadline -Description "guest agent readiness"
  foreach ($command in $commands) {
    Write-JsonFile -Path (Join-Path $commandsDir "$($command.id).json") -Value $command
    Wait-ForGuestPath -Path (Join-Path $resultsDir "$($command.id).json") `
      -Deadline $deadline -Description "lifecycle result $($command.id)"
    $commandResult = Get-Result -ResultsDir $resultsDir -Id $command.id
    if ($commandResult.status -ne "PASS") {
      $commandFailure = $commandResult
      break
    }
  }
  if ($null -ne $commandFailure) {
    $finalInspectId = "99-failure-final-inspect"
    $failureInspect = @{ id = $finalInspectId; action = "inspect" }
    Write-JsonFile -Path (Join-Path $commandsDir "$finalInspectId.json") `
      -Value $failureInspect
    Wait-ForGuestPath -Path (Join-Path $resultsDir "$finalInspectId.json") `
      -Deadline $deadline -Description "failure final-state inspection"
  }
  [System.IO.File]::WriteAllText(
    (Join-Path $sharedDir "stop.flag"),
    "stop",
    [System.Text.Encoding]::ASCII
  )
  $stopWritten = $true
  Wait-ForGuestPath -Path (Join-Path $resultsDir "agent-stopped.json") `
    -Deadline $deadline -Description "guest agent shutdown"
} finally {
  if (-not $stopWritten -and (Test-Path -LiteralPath $resultsDir -PathType Container)) {
    [System.IO.File]::WriteAllText(
      (Join-Path $sharedDir "stop.flag"),
      "stop",
      [System.Text.Encoding]::ASCII
    )
  }
}

$checks = [System.Collections.Generic.List[object]]::new()
$resultSummary = [System.Collections.Generic.List[object]]::new()
$failure = $null
try {
  $ready = Get-Content -Raw -LiteralPath (Join-Path $resultsDir "agent-ready.json") |
    ConvertFrom-Json
  if ($ready.status -ne "PASS") { throw "Guest agent did not become ready." }
  if (@($ready.state.applications).Count -ne 0 -or [bool]$ready.state.catalog.keyExists) {
    throw "Sandbox did not begin with a clean Code Hangar app/catalog state."
  }
  $checks.Add([pscustomobject]@{ name = "clean-sandbox-start"; status = "PASS" })

  $failedResults = [System.Collections.Generic.List[object]]::new()
  foreach ($resultFile in @(
    Get-ChildItem -LiteralPath $resultsDir -Filter "*.json" -File | Sort-Object Name
  )) {
    $result = Get-Content -Raw -LiteralPath $resultFile.FullName | ConvertFrom-Json
    if (-not ($result.PSObject.Properties.Name -contains "id")) { continue }
    $resultSummary.Add([pscustomobject]@{
      id = [string]$result.id
      status = [string]$result.status
      error = [string]$result.error
    })
    if ($result.status -ne "PASS") {
      $failedResults.Add($result)
    }
  }

  $final = Get-Result $resultsDir $finalInspectId
  $stopped = Get-Content -Raw -LiteralPath (Join-Path $resultsDir "agent-stopped.json") |
    ConvertFrom-Json
  if (@($final.detail.state.applications).Count -ne 0 -or
      @($final.detail.state.runningPids).Count -ne 0 -or
      @($stopped.state.applications).Count -ne 0 -or
      @($stopped.state.runningPids).Count -ne 0) {
    throw "Sandbox did not finish with zero installed Code Hangar apps/processes."
  }
  $checks.Add([pscustomobject]@{ name = "clean-final-state"; status = "PASS" })

  if ($failedResults.Count -gt 0) {
    $firstFailure = $failedResults[0]
    throw "Lifecycle command failed: $($firstFailure.id): $($firstFailure.error)"
  }

  if ($Edition -ne "Connector") {
    $localInstall = Get-Result $resultsDir "01-install-local-012"
    $localApp = Get-Application $localInstall.detail.after "Code Hangar"
    if ($localApp.displayVersion -ne "0.1.2" -or [bool]$localApp.sidecarExists) {
      throw "Installed Local edition version/sidecar invariant failed."
    }
    if ($localInstall.detail.installerSha256 -ne $artifactIdentity.local.sha256) {
      throw "Sandbox Local installer hash does not match the candidate."
    }
    $checks.Add([pscustomobject]@{ name = "local-install-launch-uninstall"; status = "PASS" })

    $afterLocal = Get-Result $resultsDir "05-after-local-inspect"
    if (@($afterLocal.detail.state.applications).Count -ne 0) {
      throw "Local edition remains installed after uninstall."
    }
  }

  if ($Edition -ne "Local") {
    $connectorInstall = Get-Result $resultsDir "06-install-connector-012"
    $connectorApp = Get-Application $connectorInstall.detail.after "Code Hangar AI Connector"
    if ($connectorApp.displayVersion -ne "0.1.2" -or -not [bool]$connectorApp.sidecarExists) {
      throw "Installed Connector edition version/sidecar invariant failed."
    }
    if ($connectorInstall.detail.installerSha256 -ne $artifactIdentity.connector.sha256) {
      throw "Sandbox Connector installer hash does not match the candidate."
    }
    $checks.Add([pscustomobject]@{ name = "connector-install-launch-uninstall"; status = "PASS" })
  }

} catch {
  $failure = $_
  $checks.Add([pscustomobject]@{
    name = "validation"
    status = "FAIL"
    error = $_.Exception.Message
  })
}

$manifest = [ordered]@{
  schemaVersion = 1
  status = if ($null -eq $failure) { "PASS" } else { "FAIL" }
  executedAt = (Get-Date).ToString("o")
  sandbox = [ordered]@{
    networking = "Disable"
    vGpu = "Disable"
    cleanStart = $true
    requestedEdition = $Edition
  }
  candidate = $artifactIdentity
  gitCommit = (git -C $repoRoot rev-parse HEAD).Trim()
  checks = @($checks)
  results = @($resultSummary)
}
Write-JsonFile -Path (Join-Path $EvidenceDir "sandbox-candidate-manifest.json") -Value $manifest

if ($null -ne $failure) { throw $failure }
Write-Host "Candidate Sandbox lifecycle passed: $EvidenceDir" -ForegroundColor Green
$manifest | ConvertTo-Json -Depth 8
