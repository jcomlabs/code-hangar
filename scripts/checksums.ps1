# Revalidate private release/acceptance/lifecycle proofs, copy the exact final
# installer bytes through write/delete-denying handles, and create the closed
# public release projection. This script never signs or uploads anything.
[CmdletBinding()]
param(
  [string]$ExpectedVersion,
  [string]$LifecycleEvidenceDir,
  [string]$AcceptanceEvidenceDir,
  [string]$ExpectedPrivateAcceptanceSha256,
  [string]$ReleaseArtifactProofDir,
  [string]$ExpectedReleaseArtifactProofSha256,
  [ValidateSet('Signed', 'Unsigned')][string]$SigningDecision,
  [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot 'packaging-common.ps1')

$script:ReleaseProofSchema = 'codehangar/release-artifact-proof/1'
$script:PrivateAcceptanceSchema = 'codehangar/acceptance-evidence-private/3'
$script:PublicAcceptanceSchema = 'codehangar/acceptance-evidence-public/1'

function Assert-CanonicalSha256 {
  param([object]$Value, [string]$Label)
  if ([string]$Value -cnotmatch '^[0-9a-f]{64}$') {
    throw "$Label must be exactly 64 lowercase hexadecimal characters."
  }
}

function Assert-ExactPropertyNames {
  param([object]$Object, [string[]]$Expected, [string]$Label)
  if ($null -eq $Object) { throw "$Label is missing." }
  $actual = @($Object.PSObject.Properties.Name | Sort-Object)
  $wanted = @($Expected | Sort-Object)
  if (($actual -join "`n") -cne ($wanted -join "`n")) {
    throw "$Label has unexpected or missing fields."
  }
}

function Assert-ExactStringSet {
  param([string[]]$Actual, [string[]]$Expected, [string]$Label)
  $actualSorted = @($Actual | Sort-Object -Unique)
  $expectedSorted = @($Expected | Sort-Object -Unique)
  if ($Actual.Count -ne $Expected.Count -or ($actualSorted -join "`n") -cne ($expectedSorted -join "`n")) {
    throw "$Label differs. Expected [$($expectedSorted -join ', ')], found [$($actualSorted -join ', ')]."
  }
}

function Assert-LocalNonReparsePath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequireExisting
  )
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) { throw "$Label must be fully qualified." }
  $full = [System.IO.Path]::GetFullPath($Path)
  $root = [System.IO.Path]::GetPathRoot($full)
  if ([string]::IsNullOrWhiteSpace($root) -or $root.StartsWith('\\')) { throw "$Label must stay on a local Windows volume." }
  $relative = $full.Substring($root.Length)
  if ($relative.Contains(':')) { throw "$Label must not address an alternate data stream." }
  $drive = [System.IO.DriveInfo]::new($root)
  if (-not $drive.IsReady -or $drive.DriveType -notin @(
      [System.IO.DriveType]::Fixed,
      [System.IO.DriveType]::Removable
    )) {
    throw "$Label must stay on a ready fixed or removable local volume."
  }
  $current = $root
  foreach ($segment in $relative.Split(
      [char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar),
      [System.StringSplitOptions]::RemoveEmptyEntries
    )) {
    $current = Join-Path $current $segment
    if (-not (Test-Path -LiteralPath $current)) {
      if ($RequireExisting) { throw "$Label path component does not exist: $current" }
      break
    }
    $item = Get-Item -LiteralPath $current -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "$Label path contains a reparse point: $current"
    }
  }
  return $full
}

function Resolve-ChildDirectory {
  param([string]$Path, [string]$AllowedRoot, [string]$Label)
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) { $Path = Join-Path $script:RepoRoot $Path }
  $full = Assert-LocalNonReparsePath -Path $Path -Label $Label -RequireExisting
  $prefix = $AllowedRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not (Test-Path -LiteralPath $full -PathType Container)) {
    throw "$Label must be an existing child under $AllowedRoot"
  }
  return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
}

function Get-LockedStreamSha256 {
  param([System.IO.FileStream]$Stream)
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $Stream.Position = 0
    $digest = $sha.ComputeHash($Stream)
    $Stream.Position = 0
    return ([System.BitConverter]::ToString($digest)).Replace('-', '').ToLowerInvariant()
  } finally {
    $sha.Dispose()
  }
}

function Open-LockedFile {
  param([string]$Path, [string]$Label, [long]$MaximumBytes = 0)
  $full = Assert-LocalNonReparsePath -Path $Path -Label $Label -RequireExisting
  if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "$Label is not a regular file." }
  $stream = [System.IO.FileStream]::new($full, 'Open', 'Read', 'Read')
  if ($stream.Length -le 0 -or ($MaximumBytes -gt 0 -and $stream.Length -gt $MaximumBytes)) {
    $stream.Dispose()
    throw "$Label has invalid size."
  }
  return [pscustomobject]@{
    Path = $full
    Stream = $stream
    Bytes = [long]$stream.Length
    Sha256 = Get-LockedStreamSha256 -Stream $stream
  }
}

function Read-LockedJson {
  param([object]$Evidence, [string]$Label)
  $reader = [System.IO.StreamReader]::new(
    $Evidence.Stream,
    [System.Text.UTF8Encoding]::new($false, $true),
    $false,
    4096,
    $true
  )
  try {
    $Evidence.Stream.Position = 0
    $text = $reader.ReadToEnd()
    $Evidence.Stream.Position = 0
    return $text | ConvertFrom-Json -DateKind String
  } catch {
    throw "$Label is not strict UTF-8 JSON: $($_.Exception.Message)"
  } finally {
    $reader.Dispose()
  }
}

function Write-NewTextFile {
  param([string]$Path, [string]$Text, [System.Text.Encoding]$Encoding)
  $full = Assert-LocalNonReparsePath -Path $Path -Label 'Release staging output'
  $bytes = $Encoding.GetBytes($Text)
  $stream = [System.IO.FileStream]::new($full, 'CreateNew', 'Write', 'None')
  try {
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
}

function Copy-LockedInstallerToStage {
  param(
    [object]$Source,
    [string]$DestinationPath,
    [object]$ExpectedProof,
    [string]$Label
  )
  if ($Source.Sha256 -cne [string]$ExpectedProof.sha256 -or $Source.Bytes -ne [long]$ExpectedProof.bytes) {
    throw "$Label source does not match the release artifact proof."
  }
  $destination = Assert-LocalNonReparsePath -Path $DestinationPath -Label "$Label destination"
  if (Test-Path -LiteralPath $destination) { throw "$Label destination already exists." }
  $writer = [System.IO.FileStream]::new($destination, 'CreateNew', 'ReadWrite', 'None')
  try {
    $Source.Stream.Position = 0
    $Source.Stream.CopyTo($writer)
    $writer.Flush($true)
    $copiedHash = Get-LockedStreamSha256 -Stream $writer
    if ($writer.Length -ne $Source.Bytes -or $copiedHash -cne $Source.Sha256) {
      throw "$Label changed during locked snapshot staging."
    }
  } finally {
    $writer.Dispose()
  }
  $staged = Open-LockedFile -Path $destination -Label "$Label staged snapshot"
  try {
    $before = Get-LockedStreamSha256 -Stream $staged.Stream
    $signature = Get-AuthenticodeSignature -LiteralPath $staged.Path -ErrorAction Stop
    $expectedSignature = $ExpectedProof.authenticode
    if ([string]$signature.Status -cne [string]$expectedSignature.status) {
      throw "$Label staged Authenticode status does not match the private release proof."
    }
    if ([string]$expectedSignature.status -eq 'Valid') {
      if ($null -eq $signature.SignerCertificate -or $null -eq $signature.TimeStamperCertificate -or
          [string]$signature.SignerCertificate.Subject -cne [string]$expectedSignature.signer.subject -or
          [string]$signature.SignerCertificate.Thumbprint -cne [string]$expectedSignature.signer.thumbprint -or
          [string]$signature.TimeStamperCertificate.Thumbprint -cne [string]$expectedSignature.timestamp.signerThumbprint) {
        throw "$Label staged signer/timestamp identity does not match the private release proof."
      }
    } elseif ($null -ne $signature.SignerCertificate -or $null -ne $signature.TimeStamperCertificate) {
      throw "$Label is not honestly unsigned after staging."
    }
    if ((Get-LockedStreamSha256 -Stream $staged.Stream) -cne $before) {
      throw "$Label changed while Authenticode was checked on the staged bytes."
    }
    return $staged
  } catch {
    $staged.Stream.Dispose()
    throw
  }
}

function Get-ReleaseAssetNameMap {
  param([string]$Version)
  return [ordered]@{
    Connector = [ordered]@{
      source = "Code Hangar AI Connector_$($Version)_x64-setup.exe"
      staged = "Code-Hangar-AI-Connector_$($Version)_x64-setup.exe"
    }
    Local = [ordered]@{
      source = "Code Hangar_$($Version)_x64-setup.exe"
      staged = "Code-Hangar_$($Version)_x64-setup.exe"
    }
  }
}

function Get-ExpectedReleaseStagedNames {
  param([string]$Version)
  $map = Get-ReleaseAssetNameMap -Version $Version
  return @($map.Local.staged, $map.Connector.staged, 'ACCEPTANCE-EVIDENCE.json', 'SHA256SUMS', 'RELEASE-MANIFEST.json')
}

function Get-ConnectorEffectiveVersion {
  param([string]$BaseVersion, [object]$ConnectorConfig)
  if ($ConnectorConfig.PSObject.Properties.Name -contains 'version') { return [string]$ConnectorConfig.version }
  return $BaseVersion
}

function Get-ExpectedLifecycleResultIds {
  param([string]$BaselineVersion, [string]$CandidateVersion)
  $baselineTag = $BaselineVersion -replace '[^0-9A-Za-z]', ''
  $candidateTag = $CandidateVersion -replace '[^0-9A-Za-z]', ''
  return @(
    "01-clean-install-local-$candidateTag", '02-uninstall-provisioning-local',
    "03-install-baseline-local-$baselineTag", "04-launch-baseline-local-$baselineTag",
    "05-close-baseline-local-$baselineTag", '06-register-baseline-catalog',
    '08-check-baseline-catalog', "09-upgrade-local-$candidateTag",
    "10-check-upgraded-catalog-$candidateTag", "11-install-connector-$candidateTag",
    "12-launch-connector-$candidateTag", "13-close-connector-$candidateTag",
    '14-check-connector-catalog', "15-repair-connector-$candidateTag",
    '16-uninstall-local', '17-launch-connector-after-local-uninstall',
    '18-close-connector-after-local-uninstall', '19-check-connector-only-catalog',
    '20-uninstall-connector', "21-reinstall-local-$candidateTag",
    '22-launch-reinstalled-local', '23-close-reinstalled-local',
    '24-check-reinstalled-local-catalog', '25-final-uninstall-local', '26-final-inspect'
  )
}

if ($SelfTest) {
  $badHashRejected = $false
  try { Assert-CanonicalSha256 -Value ('A' * 64) -Label 'self-test' } catch { $badHashRejected = $true }
  if (-not $badHashRejected) { throw 'Checksum self-test accepted a non-canonical hash.' }
  $names = Get-ExpectedReleaseStagedNames -Version '9.8.7'
  Assert-ExactStringSet -Actual $names -Expected @(
    'Code-Hangar_9.8.7_x64-setup.exe', 'Code-Hangar-AI-Connector_9.8.7_x64-setup.exe',
    'ACCEPTANCE-EVIDENCE.json', 'SHA256SUMS', 'RELEASE-MANIFEST.json'
  ) -Label 'self-test release inventory'
  $tempParent = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $tempRoot = Join-Path $tempParent ('codehangar-checksums-selftest-' + [guid]::NewGuid().ToString('N'))
  [void][System.IO.Directory]::CreateDirectory($tempRoot)
  $source = $null
  $staged = $null
  try {
    $sourcePath = Join-Path $tempRoot 'source.exe'
    Write-PackagingSelfTestPe -Path $sourcePath
    $source = Open-LockedFile -Path $sourcePath -Label 'Checksum self-test source'
    $proof = [pscustomobject]@{
      bytes = $source.Bytes
      sha256 = $source.Sha256
      authenticode = [pscustomobject]@{ status = 'NotSigned'; signer = $null; timestamp = $null }
    }
    $staged = Copy-LockedInstallerToStage -Source $source -DestinationPath (Join-Path $tempRoot 'staged.exe') -ExpectedProof $proof -Label 'Checksum self-test installer'
    $writerRejected = $false
    try {
      $writer = [System.IO.FileStream]::new($staged.Path, 'Open', 'Write', 'Read')
      $writer.Dispose()
    } catch [System.IO.IOException] { $writerRejected = $true }
    if (-not $writerRejected) { throw 'Checksum self-test staged file allowed a tampering writer.' }
  } finally {
    if ($null -ne $staged) { $staged.Stream.Dispose() }
    if ($null -ne $source) { $source.Stream.Dispose() }
    $resolved = [System.IO.Path]::GetFullPath($tempRoot)
    if ([System.IO.Path]::GetDirectoryName($resolved).Equals($tempParent, [System.StringComparison]::OrdinalIgnoreCase) -and
        [System.IO.Path]::GetFileName($resolved).StartsWith('codehangar-checksums-selftest-', [System.StringComparison]::Ordinal)) {
      [System.IO.Directory]::Delete($resolved, $true)
    } else {
      throw "Refusing unsafe checksum self-test cleanup: $resolved"
    }
  }
  Write-Host 'Checksum staging locked-copy/AuthentiCode/tamper self-test passed.' -ForegroundColor Green
  exit 0
}

foreach ($required in ([ordered]@{
    ExpectedVersion = $ExpectedVersion
    LifecycleEvidenceDir = $LifecycleEvidenceDir
    AcceptanceEvidenceDir = $AcceptanceEvidenceDir
    ExpectedPrivateAcceptanceSha256 = $ExpectedPrivateAcceptanceSha256
    ReleaseArtifactProofDir = $ReleaseArtifactProofDir
    ExpectedReleaseArtifactProofSha256 = $ExpectedReleaseArtifactProofSha256
    SigningDecision = $SigningDecision
  }).GetEnumerator()) {
  if ([string]::IsNullOrWhiteSpace([string]$required.Value)) { throw "-$($required.Key) is required." }
}
if ($ExpectedVersion -notmatch '^\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$') { throw 'ExpectedVersion is malformed.' }
Assert-CanonicalSha256 -Value $ExpectedPrivateAcceptanceSha256 -Label 'Expected private acceptance SHA-256'
Assert-CanonicalSha256 -Value $ExpectedReleaseArtifactProofSha256 -Label 'Expected release artifact proof SHA-256'

$script:RepoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$pwshPath = Join-Path $PSHOME 'pwsh.exe'
$nsisDir = Assert-LocalNonReparsePath -Path (Join-Path $script:RepoRoot 'target\release\bundle\nsis') -Label 'NSIS bundle directory' -RequireExisting
if (-not (Test-Path -LiteralPath $nsisDir -PathType Container)) { throw 'NSIS bundle directory is missing.' }
$assetsDir = Join-Path $nsisDir 'release-assets'
if (Test-Path -LiteralPath $assetsDir) { throw 'Refusing to reuse release-assets; remove it deliberately before a fresh staging run.' }

$tauriConfig = Get-Content -LiteralPath (Join-Path $script:RepoRoot 'apps\desktop\src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$connectorConfig = Get-Content -LiteralPath (Join-Path $script:RepoRoot 'apps\desktop\src-tauri\tauri.connector.conf.json') -Raw | ConvertFrom-Json
$rootPackage = Get-Content -LiteralPath (Join-Path $script:RepoRoot 'package.json') -Raw | ConvertFrom-Json
$desktopPackage = Get-Content -LiteralPath (Join-Path $script:RepoRoot 'apps\desktop\package.json') -Raw | ConvertFrom-Json
$cargoText = Get-Content -LiteralPath (Join-Path $script:RepoRoot 'Cargo.toml') -Raw
$cargoSection = [regex]::Match($cargoText, '(?ms)^\[workspace\.package\][ \t]*\r?\n(?<body>.*?)(?=^\[|\z)')
$cargoVersion = if ($cargoSection.Success) { [regex]::Match($cargoSection.Groups['body'].Value, '(?m)^version[ \t]*=[ \t]*"([^"]+)"[ \t]*$').Groups[1].Value } else { '' }
$versions = [ordered]@{
  baseTauri = [string]$tauriConfig.version
  connectorTauriEffective = Get-ConnectorEffectiveVersion -BaseVersion ([string]$tauriConfig.version) -ConnectorConfig $connectorConfig
  rootNpm = [string]$rootPackage.version
  desktopNpm = [string]$desktopPackage.version
  cargo = [string]$cargoVersion
}
if ([string]$tauriConfig.productName -cne 'Code Hangar' -or [string]$connectorConfig.productName -cne 'Code Hangar AI Connector') {
  throw 'Tauri edition product names do not match the release contract.'
}
foreach ($entry in $versions.GetEnumerator()) {
  if ([string]$entry.Value -cne $ExpectedVersion) { throw "Version mismatch: $($entry.Key) is $($entry.Value), expected $ExpectedVersion." }
}

$identity = Get-CodeHangarCleanGitIdentity -RepoRoot $script:RepoRoot
$gitBranch = ([string](& git -C $script:RepoRoot branch --show-current)).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Unable to read release branch.' }
$gitCommit = $identity.Commit
$gitTree = $identity.Tree

$releaseProofDirectory = Resolve-ChildDirectory `
  -Path $ReleaseArtifactProofDir `
  -AllowedRoot ([System.IO.Path]::GetFullPath((Join-Path $script:RepoRoot '.local\acceptance\v0.1.3\release-proof'))) `
  -Label 'Release artifact proof directory'
& $pwshPath -NoProfile -File (Join-Path $PSScriptRoot 'release-artifact-proof.ps1') `
  -ValidateOnly -EvidenceDir $releaseProofDirectory -ExpectedReportSha256 $ExpectedReleaseArtifactProofSha256 | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Release artifact proof failed validation.' }
$releaseProofEvidence = Open-LockedFile -Path (Join-Path $releaseProofDirectory 'RELEASE-ARTIFACT-PROOF.private.json') -Label 'Release artifact proof report' -MaximumBytes 1048576
try {
  if ($releaseProofEvidence.Sha256 -cne $ExpectedReleaseArtifactProofSha256) { throw 'Release artifact proof changed after validation.' }
  $releaseProof = Read-LockedJson -Evidence $releaseProofEvidence -Label 'Release artifact proof report'
  if ([int]$releaseProof.schemaVersion -ne 1 -or [string]$releaseProof.documentType -cne $script:ReleaseProofSchema -or
      [string]$releaseProof.version -cne $ExpectedVersion -or [string]$releaseProof.status -cne 'PASS' -or
      [string]$releaseProof.source.gitCommit -cne $gitCommit -or [string]$releaseProof.source.gitTree -cne $gitTree -or
      [string]$releaseProof.bindings.signingDecision.value -cne $SigningDecision) {
    throw 'Release artifact proof source/version/signing decision mismatch.'
  }
} finally {
  $releaseProofEvidence.Stream.Dispose()
}

$acceptanceDirectory = Resolve-ChildDirectory `
  -Path $AcceptanceEvidenceDir `
  -AllowedRoot ([System.IO.Path]::GetFullPath((Join-Path $script:RepoRoot '.local\acceptance\v0.1.3\candidate'))) `
  -Label 'Private acceptance directory'
& $pwshPath -NoProfile -File (Join-Path $PSScriptRoot 'acceptance-v013.ps1') `
  -ValidateOnly `
  -EvidenceDir $acceptanceDirectory `
  -ReleaseArtifactProofDir $releaseProofDirectory `
  -ExpectedReleaseArtifactProofSha256 $ExpectedReleaseArtifactProofSha256 `
  -ExpectedPrivateReportSha256 $ExpectedPrivateAcceptanceSha256 | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Private acceptance evidence failed validation.' }
$privateAcceptanceEvidence = Open-LockedFile -Path (Join-Path $acceptanceDirectory 'ACCEPTANCE-EVIDENCE.private.json') -Label 'Private acceptance report' -MaximumBytes 4194304
try {
  if ($privateAcceptanceEvidence.Sha256 -cne $ExpectedPrivateAcceptanceSha256) { throw 'Private acceptance report changed after validation.' }
  $privateAcceptance = Read-LockedJson -Evidence $privateAcceptanceEvidence -Label 'Private acceptance report'
  if ([int]$privateAcceptance.schemaVersion -ne 3 -or [string]$privateAcceptance.documentType -cne $script:PrivateAcceptanceSchema -or
      [string]$privateAcceptance.status -cne 'PASS' -or @($privateAcceptance.gates).Count -ne 50 -or
      [string]$privateAcceptance.source.gitCommit -cne $gitCommit -or [string]$privateAcceptance.source.gitTree -cne $gitTree -or
      [string]$privateAcceptance.releaseBindings.artifactProof.sha256 -cne $ExpectedReleaseArtifactProofSha256) {
    throw 'Private acceptance report does not match the exact release proof/source.'
  }
} finally {
  $privateAcceptanceEvidence.Stream.Dispose()
}

$lifecycleDirectory = Resolve-ChildDirectory `
  -Path $LifecycleEvidenceDir `
  -AllowedRoot ([System.IO.Path]::GetFullPath((Join-Path $script:RepoRoot ".local\acceptance\v$ExpectedVersion\sandbox-lifecycle"))) `
  -Label 'Lifecycle evidence directory'
$lifecyclePath = Join-Path $lifecycleDirectory 'lifecycle-manifest.json'
$lifecycleEvidence = Open-LockedFile -Path $lifecyclePath -Label 'Lifecycle manifest' -MaximumBytes 1048576
try {
  if ($lifecycleEvidence.Sha256 -cne [string]$releaseProof.bindings.lifecycle.sha256) {
    throw 'Lifecycle manifest does not match the release artifact proof.'
  }
  $lifecycle = Read-LockedJson -Evidence $lifecycleEvidence -Label 'Lifecycle manifest'
  $baselineVersion = [string]$lifecycle.baselineVersion
  & $pwshPath -NoProfile -File (Join-Path $PSScriptRoot 'sandbox-lifecycle.ps1') `
    -ValidateOnly -EvidenceDir $lifecycleDirectory -BaselineVersion $baselineVersion -CandidateVersion $ExpectedVersion | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'Lifecycle evidence failed validation.' }
  if ([int]$lifecycle.schemaVersion -ne 3 -or [string]$lifecycle.documentType -cne 'codehangar/sandbox-lifecycle/3' -or
      [string]$lifecycle.status -cne 'PASS' -or
      [string]$lifecycle.candidateVersion -cne $ExpectedVersion -or [string]$lifecycle.gitCommit -cne $gitCommit -or
      [bool]$lifecycle.historicalFailuresAccepted) {
    throw 'Lifecycle evidence is not an authoritative passing run for this source/version.'
  }
  Assert-ExactStringSet `
    -Actual @($lifecycle.results | ForEach-Object { [string]$_.id }) `
    -Expected @(Get-ExpectedLifecycleResultIds -BaselineVersion $baselineVersion -CandidateVersion $ExpectedVersion) `
    -Label 'Lifecycle result inventory'
  Assert-ExactStringSet `
    -Actual @($releaseProof.bindings.lifecycle.resultIds | ForEach-Object { [string]$_ }) `
    -Expected @($lifecycle.results | ForEach-Object { [string]$_.id }) `
    -Label 'Release-proof lifecycle result binding'
  if (@($lifecycle.results | Where-Object { [string]$_.status -cne 'PASS' }).Count -ne 0) {
    throw 'Lifecycle evidence contains a non-passing result.'
  }
  $lifecycleHash = $lifecycleEvidence.Sha256
} finally {
  $lifecycleEvidence.Stream.Dispose()
}

$assetMap = Get-ReleaseAssetNameMap -Version $ExpectedVersion
$sourceLocks = [ordered]@{}
$stagedLocks = [ordered]@{}
try {
  foreach ($edition in @('Connector', 'Local')) {
    $sourcePath = Join-Path $nsisDir $assetMap[$edition].source
    $sourceLocks[$edition] = Open-LockedFile -Path $sourcePath -Label "$edition final setup"
    $expectedProof = $releaseProof.bindings.editions.$edition.artifacts.setup
    $lifecycleHashProperty = if ($edition -eq 'Local') { 'candidateLocalSha256' } else { 'candidateConnectorSha256' }
    if ($sourceLocks[$edition].Sha256 -cne [string]$expectedProof.sha256 -or
        $sourceLocks[$edition].Bytes -ne [long]$expectedProof.bytes -or
        $sourceLocks[$edition].Sha256 -cne [string]$lifecycle.sourceProvenance.$lifecycleHashProperty) {
      throw "$edition setup does not match its release proof and lifecycle evidence."
    }
  }

  # Re-read the source identity and every locked input before creating the one
  # canonical staging directory. Any drift leaves no publishable projection.
  $identityAfter = Get-CodeHangarCleanGitIdentity -RepoRoot $script:RepoRoot
  if ($identityAfter.Commit -cne $gitCommit -or $identityAfter.Tree -cne $gitTree) { throw 'Release source changed during validation.' }
  foreach ($edition in @('Connector', 'Local')) {
    if ((Get-LockedStreamSha256 -Stream $sourceLocks[$edition].Stream) -cne $sourceLocks[$edition].Sha256) {
      throw "$edition setup changed during validation."
    }
  }
  if (Test-Path -LiteralPath $assetsDir) { throw 'release-assets appeared during validation; refusing to merge or overwrite it.' }
  [void][System.IO.Directory]::CreateDirectory($assetsDir)
  Assert-LocalNonReparsePath -Path $assetsDir -Label 'Release staging directory' -RequireExisting | Out-Null

  foreach ($edition in @('Connector', 'Local')) {
    $stagedLocks[$edition] = Copy-LockedInstallerToStage `
      -Source $sourceLocks[$edition] `
      -DestinationPath (Join-Path $assetsDir $assetMap[$edition].staged) `
      -ExpectedProof $releaseProof.bindings.editions.$edition.artifacts.setup `
      -Label "$edition final setup"
  }

  $checksumLines = @(@('Connector', 'Local') | ForEach-Object {
      "$($stagedLocks[$_].Sha256)  $($assetMap[$_].staged)"
    })
  $checksumsPath = Join-Path $assetsDir 'SHA256SUMS'
  Write-NewTextFile -Path $checksumsPath -Text (($checksumLines -join "`n") + "`n") -Encoding ([System.Text.Encoding]::ASCII)

  $publicAcceptancePath = Join-Path $assetsDir 'ACCEPTANCE-EVIDENCE.json'
  & $pwshPath -NoProfile -File (Join-Path $PSScriptRoot 'acceptance-v013.ps1') `
    -ExportPublicProjection `
    -EvidenceDir $acceptanceDirectory `
    -ReleaseArtifactProofDir $releaseProofDirectory `
    -ExpectedReleaseArtifactProofSha256 $ExpectedReleaseArtifactProofSha256 `
    -ExpectedPrivateReportSha256 $ExpectedPrivateAcceptanceSha256 `
    -OutputPath $publicAcceptancePath | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'Public acceptance projection export failed.' }
  $publicAcceptanceEvidence = Open-LockedFile -Path $publicAcceptancePath -Label 'Public acceptance projection' -MaximumBytes 1048576
  try {
    $publicAcceptance = Read-LockedJson -Evidence $publicAcceptanceEvidence -Label 'Public acceptance projection'
    if ([int]$publicAcceptance.schemaVersion -ne 1 -or [string]$publicAcceptance.documentType -cne $script:PublicAcceptanceSchema -or
        [string]$publicAcceptance.status -cne 'PASS' -or [int]$publicAcceptance.gateCount -ne 50 -or
        [string]$publicAcceptance.privateEvidenceSha256 -cne $ExpectedPrivateAcceptanceSha256) {
      throw 'Public acceptance projection is invalid.'
    }
    $publicAcceptanceHash = $publicAcceptanceEvidence.Sha256
    $publicAcceptanceBytes = $publicAcceptanceEvidence.Bytes
  } finally {
    $publicAcceptanceEvidence.Stream.Dispose()
  }

  $outerAuthenticodeStatus = if ([string]$publicAcceptance.releaseBindings.signingDecision.value -ceq 'Signed') {
    'Valid'
  } else {
    'NotSigned'
  }
  $manifest = [ordered]@{
    schemaVersion = 4
    documentType = 'codehangar/public-release-manifest/4'
    generatedAt = (Get-Date).ToString('o')
    version = $ExpectedVersion
    source = [ordered]@{ gitCommit = $gitCommit; gitTree = $gitTree; sourceTreeDirty = $false }
    versions = $versions
    signingDecision = $publicAcceptance.releaseBindings.signingDecision
    privateBindings = [ordered]@{
      releaseArtifactProofSha256 = $ExpectedReleaseArtifactProofSha256
      privateAcceptanceSha256 = $ExpectedPrivateAcceptanceSha256
      lifecycleManifestSha256 = $lifecycleHash
    }
    releaseBindings = $publicAcceptance.releaseBindings
    acceptance = [ordered]@{
      stagedName = 'ACCEPTANCE-EVIDENCE.json'
      schema = $script:PublicAcceptanceSchema
      bytes = $publicAcceptanceBytes
      sha256 = $publicAcceptanceHash
      gateCount = 50
    }
    assets = @(@('Connector', 'Local') | ForEach-Object {
        [ordered]@{
          edition = $_
          stagedName = $assetMap[$_].staged
          bytes = $stagedLocks[$_].Bytes
          sha256 = $stagedLocks[$_].Sha256
          authenticodeStatus = $outerAuthenticodeStatus
        }
      })
  }
  $manifestPath = Join-Path $assetsDir 'RELEASE-MANIFEST.json'
  Write-NewTextFile `
    -Path $manifestPath `
    -Text (($manifest | ConvertTo-Json -Depth 16) + "`n") `
    -Encoding ([System.Text.UTF8Encoding]::new($false))

  $actualEntries = @(Get-ChildItem -LiteralPath $assetsDir -Force)
  if (@($actualEntries | Where-Object { $_.PSIsContainer -or ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 }).Count -ne 0) {
    throw 'Release staging contains a directory or reparse point.'
  }
  Assert-ExactStringSet `
    -Actual @($actualEntries.Name) `
    -Expected @(Get-ExpectedReleaseStagedNames -Version $ExpectedVersion) `
    -Label 'Final release staging inventory'
  foreach ($edition in @('Connector', 'Local')) {
    if ((Get-LockedStreamSha256 -Stream $stagedLocks[$edition].Stream) -cne $stagedLocks[$edition].Sha256) {
      throw "$edition staged installer changed before the manifest was sealed."
    }
  }

  Write-Host "Wrote $checksumsPath" -ForegroundColor Green
  $checksumLines | ForEach-Object { Write-Host $_ }
  Write-Host 'Release remains HOLD until the explicit upload/download verification and owner announcement gate.' -ForegroundColor Yellow
} finally {
  foreach ($item in $stagedLocks.Values) { $item.Stream.Dispose() }
  foreach ($item in $sourceLocks.Values) { $item.Stream.Dispose() }
}
