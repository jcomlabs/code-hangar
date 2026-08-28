[CmdletBinding()]
param()

# Static guard for the release orchestration itself. It performs no build,
# packaging, signing, config edit or network operation.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$releaseScripts = @(
  "scripts/local-ci.ps1",
  "scripts/package-local.ps1",
  "scripts/package-connector.ps1",
  "scripts/packaging-common.ps1",
  "scripts/acceptance-v013.ps1",
  "scripts/release-artifact-proof.ps1",
  "scripts/checksums.ps1",
  "scripts/mcp-claude-real.ps1"
)

foreach ($relative in $releaseScripts) {
  $path = Join-Path $repoRoot $relative
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Release pipeline script is missing: $relative"
  }
  $tokens = $null
  $errors = $null
  [void][System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
  if ($errors.Count -ne 0) {
    $details = @($errors | ForEach-Object { "$($_.Message) at $($_.Extent.StartLineNumber):$($_.Extent.StartColumnNumber)" }) -join "; "
    throw "PowerShell parser rejected ${relative}: $details"
  }
}

function Get-PackageWrapperInvocations {
  param([Parameter(Mandatory = $true)][string]$Path)
  return @(Get-Content -LiteralPath $Path | Where-Object {
    $_ -match '(?i)\bpwsh(?:\.exe)?\b.*\bscripts[/\\]package-(?:local|connector)\.ps1\b'
  })
}

$localCiPath = Join-Path $repoRoot "scripts/local-ci.ps1"
$ciInvocations = Get-PackageWrapperInvocations -Path $localCiPath
if ($ciInvocations.Count -ne 4) {
  throw "local-ci.ps1 must contain exactly four package-wrapper calls (two self-tests and two explicit preflights); found $($ciInvocations.Count)."
}
foreach ($line in $ciInvocations) {
  if ($line -notmatch '(?i)\s-(?:SelfTest|PreflightOnly|PrepareSigning|BundleSigned)(?:\s|$)') {
    throw "local-ci.ps1 contains a package-wrapper call without an explicit mode: $($line.Trim())"
  }
}
$ciPackagingCalls = @($ciInvocations | Where-Object { $_ -notmatch '(?i)\s-SelfTest(?:\s|$)' })
if ($ciPackagingCalls.Count -ne 2 -or
    @($ciPackagingCalls | Where-Object { $_ -notmatch '(?i)\s-PreflightOnly(?:\s|$)' }).Count -ne 0) {
  throw "Automatic local CI may perform only the two explicit package preflights; prepare/sign/bundle remain owner gates."
}

$readmeInvocations = Get-PackageWrapperInvocations -Path (Join-Path $repoRoot "README.md")
foreach ($line in $readmeInvocations) {
  if ($line -notmatch '(?i)\s-(?:SelfTest|PreflightOnly|PrepareSigning|BundleSigned)(?:\s|$)') {
    throw "README contains an impossible package-wrapper call without an explicit mode: $($line.Trim())"
  }
}

$acceptanceSpec = Get-Content -LiteralPath (Join-Path $repoRoot "docs/qa/v0.1.3-acceptance.md") -Raw
if ($acceptanceSpec -notmatch '(?m)^Tracked document status: \*\*STABLE SPECIFICATION' -or
    $acceptanceSpec -match '(?m)^Candidate (?:commit|artifacts):' -or
    $acceptanceSpec -match '(?m)^\|[^\r\n]+\|\s*(?:PENDING|PASS|HOLD|BLOCKED)\s*\|') {
  throw "Tracked v0.1.3 acceptance Markdown regressed into mutable candidate evidence."
}
$gateIds = @([regex]::Matches($acceptanceSpec, '(?m)^\|\s*`(?<id>[A-Z]+-[0-9]{2})`') | ForEach-Object { $_.Groups['id'].Value })
if ($gateIds.Count -ne 50 -or @($gateIds | Sort-Object -Unique).Count -ne 50) {
  throw "Tracked v0.1.3 acceptance specification must expose exactly 50 unique stable gate IDs."
}

$checksumText = Get-Content -LiteralPath (Join-Path $repoRoot "scripts/checksums.ps1") -Raw
foreach ($requiredMarker in @(
    "ExpectedPrivateAcceptanceSha256",
    "ExpectedReleaseArtifactProofSha256",
    "codehangar/acceptance-evidence-public/1",
    "codehangar/public-release-manifest/4",
    "ACCEPTANCE-EVIDENCE.json"
  )) {
  if ($checksumText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Checksum staging is not visibly bound to $requiredMarker."
  }
}

$mcpText = Get-Content -LiteralPath (Join-Path $repoRoot "scripts/mcp-claude-real.ps1") -Raw
foreach ($requiredMarker in @(
    "v0.1.3",
    "ServerPath",
    "ExpectedSha256",
    "SigningReceiptPath",
    "ExpectedSigningReceiptSha256",
    "ClaudeExecutablePath",
    "ExpectedClaudeExecutableSha256",
    "ClaudeConfigRoot",
    "OwnerAuthorized",
    "mcp-fixture-smoke.ps1",
    "--strict-mcp-config"
  )) {
  if ($mcpText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Claude live-client acceptance is missing required marker: $requiredMarker"
  }
}

$releaseProofText = Get-Content -LiteralPath (Join-Path $repoRoot "scripts/release-artifact-proof.ps1") -Raw
foreach ($requiredMarker in @(
    "codehangar/release-artifact-proof/1",
    "ExpectedSignerThumbprint",
    "Get-Rfc3161Info",
    "messageImprint",
    "InstalledArtifacts",
    "codehangar/sandbox-lifecycle/3",
    "OfflineAuthenticode",
    "VerifyFile",
    "SigningDecision",
    "OwnerAcceptUnsignedOuter"
  )) {
  if ($releaseProofText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Private release artifact proof is missing required marker: $requiredMarker"
  }
}

$acceptanceText = Get-Content -LiteralPath (Join-Path $repoRoot "scripts/acceptance-v013.ps1") -Raw
foreach ($requiredMarker in @(
    "codehangar/gate-proof/2",
    "codehangar/acceptance-evidence-private/3",
    "codehangar/acceptance-evidence-public/1",
    "release-gate-contracts.json",
    "requiredDocumentTypes",
    "codehangar/publication-audit-evidence/1",
    "AUTO-06/candidate-publication-audit",
    "duplicateHashRejected",
    "gate-proofs",
    "privateEvidenceSha256"
  )) {
  if ($acceptanceText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Acceptance private/public evidence separation is missing required marker: $requiredMarker"
  }
}

$localCiText = Get-Content -LiteralPath $localCiPath -Raw
foreach ($requiredMarker in @(
    'CARGO_BUILD_JOBS = "2"',
    'RUST_TEST_THREADS = "2"',
    'compile-only Windows Local release (nonpublishable)',
    'cargo clippy Windows Connector desktop backend',
    'compile-only Windows Connector desktop release (nonpublishable)',
    'frontend Connector edition isolation',
    'release-artifact-proof.ps1 -SelfTest',
    'AUTO-06/secret-scan'
  )) {
  if ($localCiText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Local CI is missing a required low-parallel compile-only lane marker: $requiredMarker"
  }
}
if ($localCiText -cmatch [regex]::Escape('AUTO-06/candidate-publication-audit')) {
  throw 'Local CI must never issue the AUTO-06 publication-candidate claim.'
}

$publicationAuditText = Get-Content -LiteralPath (Join-Path $repoRoot 'scripts/publication-audit.mjs') -Raw
foreach ($requiredMarker in @(
    'codehangar/publication-audit-evidence/1',
    'AUTO-06/candidate-publication-audit',
    '--evidence-dir is accepted only with --candidate',
    'PUBLICATION-AUDIT.private.json',
    'sourceTreeDirty: false',
    'pathnamesInspected: true'
  )) {
  if ($publicationAuditText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Strict publication-candidate evidence is missing required marker: $requiredMarker"
  }
}

$workflowRoot = Join-Path $repoRoot '.github\workflows'
if ((Test-Path -LiteralPath $workflowRoot) -and
    @(Get-ChildItem -LiteralPath $workflowRoot -File -Recurse -Force).Count -ne 0) {
  throw 'GitHub Actions workflows must remain absent; release evidence is produced locally without consuming remote CI credits.'
}
if (Test-Path -LiteralPath (Join-Path $repoRoot '.github\dependabot.yml')) {
  throw 'Dependabot PR automation must remain absent so publication does not create bot PRs or routine notifications.'
}

$outboundGuardText = Get-Content -LiteralPath (Join-Path $repoRoot "scripts/check-no-forbidden-code.mjs") -Raw
foreach ($requiredMarker in @(
    '".ps1"', '".cmd"', '".bat"', '".yml"', '".yaml"', '".nsi"',
    "Invoke-WebRequest", "Invoke-RestMethod", "HttpClient", "WebClient",
    "Start-BitsTransfer", "bitsadmin", "certutil", "npm audit", "npm install",
    "cargo fetch", "node:http", "XMLHttpRequest", "sendBeacon", "TcpListener",
    "WinHttpOpen", "InternetOpenW", "WSAStartup", "DnsQuery_W", "negativeFixtures"
  )) {
  if ($outboundGuardText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Forbidden-code guard is missing language-aware coverage marker: $requiredMarker"
  }
}

$dependencyGuardText = Get-Content -LiteralPath (Join-Path $repoRoot "scripts/check-no-outbound-deps.mjs") -Raw
foreach ($requiredMarker in @(
    "x86_64-pc-windows-msvc",
    "checkAllTargetTauriMobileException",
    "Android/non-macOS-Apple",
    'bundle?.targets) !== JSON.stringify(["nsis"])'
  )) {
  if ($dependencyGuardText -cnotmatch [regex]::Escape($requiredMarker)) {
    throw "Outbound dependency guard is missing its Windows-only/all-target contract marker: $requiredMarker"
  }
}

Write-Host "Release pipeline parser and static contract self-test passed." -ForegroundColor Green
