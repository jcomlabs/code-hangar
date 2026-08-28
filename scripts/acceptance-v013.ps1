[CmdletBinding()]
param(
  [switch]$Initialize,
  [switch]$Finalize,
  [switch]$ValidateOnly,
  [switch]$ExportPublicProjection,
  [string]$EvidenceDir,
  [string]$ReleaseArtifactProofDir,
  [string]$ExpectedReleaseArtifactProofSha256,
  [string]$ExpectedPrivateReportSha256,
  [string]$OutputPath,
  [switch]$SelfTest
)

# The tracked Markdown is a stable gate specification. Candidate evidence stays
# private and local. Each of the 50 gates requires its own structured gate-proof
# envelope and a unique gate-scoped evidence subtree. The public projection is a
# closed schema containing only source/hash/status bindings: no local path,
# private note, claim text, command output or secret-bearing payload is copied.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:ExpectedVersion = '0.1.3'
$script:DraftName = 'acceptance-results.private.json'
$script:PrivateReportName = 'ACCEPTANCE-EVIDENCE.private.json'
$script:DraftSchema = 'codehangar/acceptance-results-private/3'
$script:GateProofSchema = 'codehangar/gate-proof/2'
$script:PrivateReportSchema = 'codehangar/acceptance-evidence-private/3'
$script:PublicReportSchema = 'codehangar/acceptance-evidence-public/1'
$script:ReleaseProofSchema = 'codehangar/release-artifact-proof/1'
$script:ContractSchema = 'codehangar/release-gate-contracts/1'
$script:LocalCiSchema = 'codehangar/local-ci-evidence/1'
$script:PublicationAuditSchema = 'codehangar/publication-audit-evidence/1'
$script:OwnerAttestationSchema = 'codehangar/owner-gate-attestation/1'
$script:SupervisedAttestationSchema = 'codehangar/supervised-gate-attestation/1'
$script:PublicationRepository = 'https://github.com/jcomlabs/code-hangar'
$script:PublicationIdentityName = 'JC-OM'
$script:PublicationIdentityEmail = '268269267+JigSawPT@users.noreply.github.com'

$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$specPath = Join-Path $repoRoot 'docs\qa\v0.1.3-acceptance.md'
$contractPath = Join-Path $repoRoot 'scripts\release-gate-contracts.json'
$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local\acceptance\v0.1.3\candidate'))
$releaseProofRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local\acceptance\v0.1.3\release-proof'))

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

function Assert-LocalNonReparsePath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequireExisting
  )
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) { throw "$Label must be fully qualified." }
  $full = [System.IO.Path]::GetFullPath($Path)
  $root = [System.IO.Path]::GetPathRoot($full)
  if ([string]::IsNullOrWhiteSpace($root) -or $root.StartsWith('\\')) {
    throw "$Label must stay on a local Windows volume."
  }
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

function Get-LockedStreamSha256 {
  param([Parameter(Mandatory = $true)][System.IO.FileStream]$Stream)
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

function Get-TextSha256 {
  param([Parameter(Mandatory = $true)][string]$Text)
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    return ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
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

function Write-NewUtf8File {
  param([string]$Path, [string]$Text)
  $full = Assert-LocalNonReparsePath -Path $Path -Label 'Acceptance output'
  $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($Text + "`n")
  $stream = [System.IO.FileStream]::new($full, 'CreateNew', 'Write', 'None')
  try {
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
}

function Write-NewJson {
  param([string]$Path, [object]$Value, [int]$Depth = 15)
  Write-NewUtf8File -Path $Path -Text ($Value | ConvertTo-Json -Depth $Depth)
}

function Resolve-ScopedDirectory {
  param([string]$Path, [string]$AllowedRoot, [string]$Label, [switch]$RequireExisting)
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) { $Path = Join-Path $repoRoot $Path }
  $full = Assert-LocalNonReparsePath -Path $Path -Label $Label -RequireExisting:$RequireExisting
  $prefix = $AllowedRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "$Label must be a child directory under $AllowedRoot"
  }
  if ($RequireExisting -and -not (Test-Path -LiteralPath $full -PathType Container)) {
    throw "$Label does not exist."
  }
  return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
}

function Resolve-CandidateDirectory {
  param([string]$Path, [switch]$RequireExisting)
  return Resolve-ScopedDirectory -Path $Path -AllowedRoot $acceptanceRoot -Label 'Acceptance evidence directory' -RequireExisting:$RequireExisting
}

function Resolve-ReleaseProofDirectory {
  param([string]$Path)
  return Resolve-ScopedDirectory -Path $Path -AllowedRoot $releaseProofRoot -Label 'Release artifact proof directory' -RequireExisting
}

function Get-CleanGitIdentity {
  $commit = ([string](& git -C $repoRoot rev-parse HEAD)).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to read the acceptance source commit.' }
  $tree = ([string](& git -C $repoRoot rev-parse 'HEAD^{tree}')).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to read the acceptance source tree.' }
  $branch = ([string](& git -C $repoRoot branch --show-current)).Trim()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to read the acceptance source branch.' }
  $status = @(& git -C $repoRoot status --porcelain --untracked-files=all)
  if ($LASTEXITCODE -ne 0 -or $status.Count -ne 0) {
    throw 'Acceptance evidence requires the exact clean release source tree.'
  }
  if ($commit -cnotmatch '^[0-9a-f]{40,64}$' -or $tree -cnotmatch '^[0-9a-f]{40,64}$') {
    throw 'Acceptance source commit/tree identity is malformed.'
  }
  return [pscustomobject]@{ commit = $commit; tree = $tree; branch = $branch }
}

function Read-AcceptanceSpecification {
  if (-not (Test-Path -LiteralPath $specPath -PathType Leaf)) { throw "Tracked acceptance specification is missing: $specPath" }
  $gates = [System.Collections.Generic.List[object]]::new()
  foreach ($line in Get-Content -LiteralPath $specPath) {
    if ($line -match '^\|\s*`(?<id>[A-Z]+-[0-9]{2})`\s+(?<name>[^|]+?)\s*\|\s*(?<rule>[^|]+?)\s*\|\s*(?<requirement>[^|]+?)\s*\|\s*$') {
      $gates.Add([pscustomobject]@{
        id = $Matches.id
        name = $Matches.name.Trim()
        rule = $Matches.rule.Trim()
        requirement = $Matches.requirement.Trim()
      })
    }
  }
  if ($gates.Count -ne 50) { throw "Acceptance specification must contain exactly 50 stable gate IDs; found $($gates.Count)." }
  $ids = @($gates | ForEach-Object { [string]$_.id })
  if (@($ids | Sort-Object -Unique).Count -ne 50) { throw 'Acceptance specification contains duplicate gate IDs.' }
  $prefixCounts = [ordered]@{ SRC = 6; AUTO = 8; SAFE = 13; UX = 11; LIFE = 7; OWNER = 5 }
  foreach ($entry in $prefixCounts.GetEnumerator()) {
    if (@($ids | Where-Object { $_ -match "^$($entry.Key)-" }).Count -ne $entry.Value) {
      throw "Acceptance specification has the wrong number of $($entry.Key) gates."
    }
  }
  return @($gates)
}

function Assert-ExactStringSet {
  param([object[]]$Actual, [object[]]$Expected, [string]$Label)
  $actualValues = @($Actual | ForEach-Object { [string]$_ } | Sort-Object)
  $expectedValues = @($Expected | ForEach-Object { [string]$_ } | Sort-Object)
  if (($actualValues -join "`n") -cne ($expectedValues -join "`n") -or
      @($actualValues | Sort-Object -Unique).Count -ne $actualValues.Count) {
    throw "$Label does not match its exact unique contract."
  }
}

function Assert-Safe06Contract {
  param([object]$Contract)
  if ([string]$Contract.gateId -cne 'SAFE-06' -or
      [string]$Contract.mode -cne 'supervised-manual' -or
      [string]$Contract.producer.id -cne 'supervised-safety' -or
      [string]$Contract.producer.schema -cne $script:SupervisedAttestationSchema) {
    throw 'SAFE-06 must remain a supervised-safety gate with a typed supervised attestation.'
  }
  Assert-ExactStringSet `
    -Actual @($Contract.sharedEvidenceIds) `
    -Expected @('release-proof') `
    -Label 'SAFE-06 shared evidence'
  Assert-ExactStringSet `
    -Actual @($Contract.requiredTestIds) `
    -Expected @(
      'SAFE-06/extended-disposition-ntfs',
      'SAFE-06/legacy-disposition-ntfs',
      'SAFE-06/cancel-delete-pending-abrupt-termination'
    ) `
    -Label 'SAFE-06 supervised NTFS results'
  Assert-ExactStringSet `
    -Actual @($Contract.requiredLifecycleResultIds) `
    -Expected @() `
    -Label 'SAFE-06 lifecycle results'
  Assert-ExactStringSet `
    -Actual @($Contract.requiredDocumentTypes) `
    -Expected @($script:SupervisedAttestationSchema) `
    -Label 'SAFE-06 typed attestation'
}

function Assert-Auto06Contract {
  param([object]$Contract)
  if ([string]$Contract.gateId -cne 'AUTO-06' -or
      [string]$Contract.mode -cne 'automated' -or
      [string]$Contract.producer.id -cne 'publication-audit' -or
      [string]$Contract.producer.schema -cne $script:PublicationAuditSchema) {
    throw 'AUTO-06 must be produced by the strict publication-candidate auditor, never by worktree local-CI.'
  }
  Assert-ExactStringSet `
    -Actual @($Contract.sharedEvidenceIds) `
    -Expected @('local-ci', 'publication-audit') `
    -Label 'AUTO-06 shared evidence'
  Assert-ExactStringSet `
    -Actual @($Contract.requiredTestIds) `
    -Expected @('AUTO-06/secret-scan', 'AUTO-06/candidate-publication-audit') `
    -Label 'AUTO-06 split test claims'
  Assert-ExactStringSet -Actual @($Contract.requiredLifecycleResultIds) -Expected @() -Label 'AUTO-06 lifecycle results'
  Assert-ExactStringSet -Actual @($Contract.requiredDocumentTypes) -Expected @() -Label 'AUTO-06 gate payload types'
}

function Read-GateContracts {
  param([object[]]$Specification)
  $evidence = Open-LockedFile -Path $contractPath -Label 'Release gate contracts' -MaximumBytes 524288
  try {
    $value = Read-LockedJson -Evidence $evidence -Label 'Release gate contracts'
    Assert-ExactPropertyNames -Object $value -Expected @('schemaVersion', 'documentType', 'version', 'sharedEvidence', 'gates') -Label 'Release gate contracts'
    if ([int]$value.schemaVersion -ne 1 -or [string]$value.documentType -cne $script:ContractSchema -or
        [string]$value.version -cne $script:ExpectedVersion) {
      throw 'Release gate contract schema/version mismatch.'
    }
    Assert-ExactPropertyNames -Object $value.sharedEvidence -Expected @('local-ci', 'publication-audit', 'release-proof', 'lifecycle') -Label 'Shared evidence contract'
    Assert-ExactPropertyNames -Object $value.sharedEvidence.'local-ci' -Expected @('documentType', 'path') -Label 'Local-CI shared evidence contract'
    Assert-ExactPropertyNames -Object $value.sharedEvidence.'publication-audit' -Expected @('documentType', 'path') -Label 'Publication-audit shared evidence contract'
    foreach ($externalId in @('release-proof', 'lifecycle')) {
      Assert-ExactPropertyNames -Object $value.sharedEvidence.$externalId -Expected @('documentType', 'external') -Label "$externalId shared evidence contract"
      if (-not [bool]$value.sharedEvidence.$externalId.external) { throw "$externalId must remain an external independently validated proof." }
    }
    if ([string]$value.sharedEvidence.'local-ci'.documentType -cne $script:LocalCiSchema -or
        [string]$value.sharedEvidence.'local-ci'.path -cne 'shared-evidence/LOCAL-CI-EVIDENCE.private.json') {
      throw 'Local-CI shared evidence path/schema changed.'
    }
    if ([string]$value.sharedEvidence.'publication-audit'.documentType -cne $script:PublicationAuditSchema -or
        [string]$value.sharedEvidence.'publication-audit'.path -cne 'shared-evidence/PUBLICATION-AUDIT.private.json') {
      throw 'Publication-audit shared evidence path/schema changed.'
    }
    $gates = @($value.gates)
    if ($gates.Count -ne 50) { throw 'Release gate contracts must contain exactly 50 gates.' }
    Assert-ExactStringSet -Actual @($gates | ForEach-Object gateId) -Expected @($Specification | ForEach-Object id) -Label 'Release gate contract IDs'
    foreach ($gate in $gates) {
      Assert-ExactPropertyNames -Object $gate -Expected @(
        'gateId', 'mode', 'producer', 'sharedEvidenceIds', 'requiredTestIds',
        'requiredLifecycleResultIds', 'requiredDocumentTypes'
      ) -Label "Gate contract $($gate.gateId)"
      Assert-ExactPropertyNames -Object $gate.producer -Expected @('id', 'schema') -Label "Gate contract $($gate.gateId) producer"
      if ([string]$gate.mode -notin @('automated', 'supervised-manual', 'owner') -or
          [string]::IsNullOrWhiteSpace([string]$gate.producer.id) -or
          [string]::IsNullOrWhiteSpace([string]$gate.producer.schema)) {
        throw "Gate contract $($gate.gateId) has an invalid mode/producer."
      }
      foreach ($field in @('sharedEvidenceIds', 'requiredTestIds', 'requiredLifecycleResultIds', 'requiredDocumentTypes')) {
        $values = @($gate.$field)
        if (@($values | ForEach-Object { [string]$_ } | Sort-Object -Unique).Count -ne $values.Count) {
          throw "Gate contract $($gate.gateId) repeats $field."
        }
      }
      foreach ($sharedId in @($gate.sharedEvidenceIds)) {
        if ([string]$sharedId -notin @('local-ci', 'publication-audit', 'release-proof', 'lifecycle')) {
          throw "Gate contract $($gate.gateId) cites unknown shared evidence $sharedId."
        }
      }
      if ([string]$gate.mode -eq 'automated' -and @($gate.requiredDocumentTypes).Count -ne 0) {
        throw "Automated gate $($gate.gateId) may not be satisfied by a free-form gate payload."
      }
      if ([string]$gate.mode -ne 'automated' -and @($gate.requiredDocumentTypes).Count -ne 1) {
        throw "Manual/owner gate $($gate.gateId) requires exactly one typed attestation/report."
      }
      if ([string]$gate.gateId -ceq 'SAFE-06') {
        Assert-Safe06Contract -Contract $gate
      }
      if ([string]$gate.gateId -ceq 'AUTO-06') {
        Assert-Auto06Contract -Contract $gate
      }
    }
    return [pscustomobject]@{ value = $value; sha256 = $evidence.Sha256; gates = $gates }
  } finally { $evidence.Stream.Dispose() }
}

function Get-GateContract {
  param([object]$Contracts, [string]$GateId)
  $matches = @($Contracts.gates | Where-Object gateId -eq $GateId)
  if ($matches.Count -ne 1) { throw "Missing or ambiguous canonical gate contract: $GateId" }
  return $matches[0]
}

function Assert-ProducerContract {
  param([object]$Producer, [object]$Contract, [string]$GateId)
  Assert-ExactPropertyNames -Object $Producer -Expected @('id', 'schema') -Label "Gate $GateId producer"
  if ([string]$Producer.id -cne [string]$Contract.producer.id -or
      [string]$Producer.schema -cne [string]$Contract.producer.schema) {
    throw "Gate $GateId producer id/schema does not match its canonical contract."
  }
}

function Assert-ExactGateIndex {
  param([object[]]$Actual, [object[]]$Specification)
  $actualIds = @($Actual | ForEach-Object { [string]$_.id })
  $expectedIds = @($Specification | ForEach-Object { [string]$_.id })
  if ($actualIds.Count -ne $expectedIds.Count -or ($actualIds -join "`n") -cne ($expectedIds -join "`n")) {
    throw 'Acceptance index does not contain the exact ordered 50-gate specification.'
  }
  foreach ($gate in $Actual) {
    $expectedProof = "gate-proofs/$($gate.id).json"
    if ([string]$gate.proof -cne $expectedProof) { throw "Gate $($gate.id) must use its exact dedicated proof envelope." }
  }
}

function Get-NonReparseFileInventory {
  param([string]$Root)
  $files = [System.Collections.Generic.List[object]]::new()
  $pending = [System.Collections.Generic.Stack[string]]::new()
  $pending.Push($Root)
  while ($pending.Count -gt 0) {
    $directory = $pending.Pop()
    foreach ($item in Get-ChildItem -LiteralPath $directory -Force) {
      if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Acceptance evidence contains a reparse point: $($item.FullName)"
      }
      if ($item.PSIsContainer) {
        $pending.Push($item.FullName)
      } else {
        $files.Add([pscustomobject]@{
          fullPath = $item.FullName
          relative = [System.IO.Path]::GetRelativePath($Root, $item.FullName).Replace('\', '/')
        })
      }
    }
  }
  return @($files | Sort-Object relative)
}

function Open-GateEvidenceFile {
  param([string]$Root, [string]$GateId, [object]$Record)
  Assert-ExactPropertyNames -Object $Record -Expected @('role', 'documentType', 'path', 'bytes', 'sha256') -Label "Gate $GateId evidence record"
  if ([string]$Record.role -cne 'gate-result' -or [string]::IsNullOrWhiteSpace([string]$Record.documentType)) {
    throw "Gate $GateId evidence must be a typed gate-result document."
  }
  if ([string]::IsNullOrWhiteSpace([string]$Record.path) -or [System.IO.Path]::IsPathFullyQualified([string]$Record.path)) {
    throw "Gate $GateId evidence path must be relative."
  }
  $canonical = ([string]$Record.path).Replace('\', '/')
  $requiredPrefix = "evidence/$GateId/"
  if (-not $canonical.StartsWith($requiredPrefix, [System.StringComparison]::Ordinal)) {
    throw "Gate $GateId may cite only its unique $requiredPrefix subtree."
  }
  $full = Assert-LocalNonReparsePath -Path (Join-Path $Root $canonical) -Label "Gate $GateId evidence" -RequireExisting
  $rootPrefix = $Root.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Gate $GateId evidence escapes the acceptance directory."
  }
  $evidence = Open-LockedFile -Path $full -Label "Gate $GateId evidence"
  if ([long]$Record.bytes -ne $evidence.Bytes -or [string]$Record.sha256 -cne $evidence.Sha256) {
    $evidence.Stream.Dispose()
    throw "Gate $GateId evidence bytes/hash do not match its proof envelope."
  }
  return $evidence
}

function Assert-Timestamp {
  param([object]$Value, [string]$Label)
  $parsed = [datetime]::MinValue
  if (-not [datetime]::TryParseExact(
      [string]$Value,
      'yyyy-MM-ddTHH:mm:ss.fffffffZ',
      [System.Globalization.CultureInfo]::InvariantCulture,
      [System.Globalization.DateTimeStyles]::AssumeUniversal,
      [ref]$parsed
    )) { throw "$Label is not a canonical UTC timestamp." }
}

function Assert-ManualAttestation {
  param(
    [object]$Document,
    [object]$Contract,
    [object]$Identity,
    [string]$SpecificationSha256,
    [string]$ReleaseProofSha256,
    [string]$GateId
  )
  $expectedSchema = [string]$Contract.requiredDocumentTypes[0]
  Assert-ExactPropertyNames -Object $Document -Expected @(
    'schemaVersion', 'documentType', 'version', 'gateId', 'status', 'attestedAtUtc',
    'source', 'specificationSha256', 'releaseArtifactProofSha256', 'attestor',
    'decision', 'testIds'
  ) -Label "Gate $GateId attestation"
  Assert-ExactPropertyNames -Object $Document.source -Expected @('gitCommit', 'gitTree', 'sourceTreeDirty') -Label "Gate $GateId attestation source"
  Assert-ExactPropertyNames -Object $Document.attestor -Expected @('kind', 'name') -Label "Gate $GateId attestor"
  if ([int]$Document.schemaVersion -ne 1 -or [string]$Document.documentType -cne $expectedSchema -or
      [string]$Document.version -cne $script:ExpectedVersion -or [string]$Document.gateId -cne $GateId -or
      [string]$Document.status -cne 'PASS' -or [string]$Document.decision -cne 'APPROVE' -or
      [bool]$Document.source.sourceTreeDirty -or [string]$Document.source.gitCommit -cne $Identity.commit -or
      [string]$Document.source.gitTree -cne $Identity.tree -or
      [string]$Document.specificationSha256 -cne $SpecificationSha256 -or
      [string]$Document.releaseArtifactProofSha256 -cne $ReleaseProofSha256) {
    throw "Gate $GateId attestation is not bound to the exact gate/source/specification/release proof."
  }
  $expectedKind = if ([string]$Contract.mode -eq 'owner') { 'owner' } else { 'supervisor' }
  if ([string]$Document.attestor.kind -cne $expectedKind -or
      [string]::IsNullOrWhiteSpace([string]$Document.attestor.name) -or
      ([string]$Document.attestor.name).Length -gt 160) {
    throw "Gate $GateId attestor identity is invalid."
  }
  Assert-Timestamp -Value $Document.attestedAtUtc -Label "Gate $GateId attestation timestamp"
  Assert-ExactStringSet -Actual @($Document.testIds) -Expected @($Contract.requiredTestIds) -Label "Gate $GateId attested test IDs"
}

function Assert-ClaudeLiveMcpReport {
  param(
    [object]$Document,
    [object]$Identity,
    [object]$ReleaseBindings
  )
  Assert-ExactPropertyNames -Object $Document -Expected @(
    'schemaVersion', 'documentType', 'version', 'status', 'completedAt', 'source',
    'flow', 'receipt', 'server', 'client', 'syntheticFixture', 'liveConfig',
    'failure', 'disconnectFailure'
  ) -Label 'OWNER-02 MCP live-client report'
  Assert-ExactPropertyNames -Object $Document.source -Expected @('gitCommit', 'gitTree', 'sourceTreeDirty') -Label 'OWNER-02 MCP source'
  Assert-ExactPropertyNames -Object $Document.flow -Expected @('client', 'data', 'config', 'ownerAuthorized') -Label 'OWNER-02 MCP flow'
  Assert-ExactPropertyNames -Object $Document.receipt -Expected @('path', 'schema', 'edition', 'version', 'sha256', 'unchanged') -Label 'OWNER-02 MCP receipt'
  Assert-ExactPropertyNames -Object $Document.server -Expected @('path', 'bytes', 'sha256', 'receiptSha256', 'unchanged') -Label 'OWNER-02 MCP server'
  Assert-ExactPropertyNames -Object $Document.client -Expected @('path', 'version', 'bytes', 'sha256', 'unchanged', 'exitCode') -Label 'OWNER-02 MCP client'
  Assert-ExactPropertyNames -Object $Document.syntheticFixture -Expected @('projectObserved', 'requiredMethods', 'audit', 'disconnect') -Label 'OWNER-02 synthetic fixture'
  Assert-ExactPropertyNames -Object $Document.liveConfig -Expected @('root', 'leaf', 'before', 'after', 'unchanged') -Label 'OWNER-02 live config'

  if ([int]$Document.schemaVersion -ne 3 -or
      [string]$Document.documentType -cne 'codehangar/claude-live-mcp-acceptance/3' -or
      [string]$Document.version -cne $script:ExpectedVersion -or
      [string]$Document.status -cne 'PASS' -or
      [bool]$Document.source.sourceTreeDirty -or
      [string]$Document.source.gitCommit -cne $Identity.commit -or
      [string]$Document.source.gitTree -cne $Identity.tree -or
      [string]$Document.flow.client -cne 'Claude Code authenticated live client' -or
      [string]$Document.flow.data -cne 'isolated synthetic fixture' -or
      [string]$Document.flow.config -cne 'temporary command-line --strict-mcp-config; no registration or unregistration' -or
      -not [bool]$Document.flow.ownerAuthorized -or
      $null -ne $Document.failure -or $null -ne $Document.disconnectFailure) {
    throw 'OWNER-02 MCP report is not the exact passing owner-supervised flow.'
  }
  Assert-Timestamp -Value $Document.completedAt -Label 'OWNER-02 MCP completion timestamp'

  foreach ($entry in @(
      @{ Value = $Document.receipt.sha256; Label = 'OWNER-02 receipt SHA-256' },
      @{ Value = $Document.server.sha256; Label = 'OWNER-02 server SHA-256' },
      @{ Value = $Document.server.receiptSha256; Label = 'OWNER-02 server receipt SHA-256' },
      @{ Value = $Document.client.sha256; Label = 'OWNER-02 client SHA-256' }
    )) { Assert-CanonicalSha256 -Value $entry.Value -Label $entry.Label }
  if ([string]$Document.receipt.schema -cne 'codehangar/signing-preparation/3' -or
      [string]$Document.receipt.edition -cne 'Connector' -or
      [string]$Document.receipt.version -cne $script:ExpectedVersion -or
      -not [bool]$Document.receipt.unchanged -or
      -not [bool]$Document.server.unchanged -or [long]$Document.server.bytes -le 0 -or
      -not [bool]$Document.client.unchanged -or [long]$Document.client.bytes -le 0 -or
      [int]$Document.client.exitCode -ne 0 -or
      [string]::IsNullOrWhiteSpace([string]$Document.client.version) -or
      ([string]$Document.client.version).Length -gt 512 -or
      [string]$Document.client.version -match '[\r\n]' -or
      [string]$Document.receipt.sha256 -cne [string]$ReleaseBindings.editions.Connector.signingReceipt.sha256 -or
      [string]$Document.server.sha256 -cne [string]$ReleaseBindings.editions.Connector.artifacts.mcp.sha256 -or
      [string]$Document.server.receiptSha256 -cne [string]$ReleaseBindings.editions.Connector.artifacts.mcp.receiptSha256) {
    throw 'OWNER-02 MCP report does not bind the exact Connector receipt, sidecar and successful client.'
  }

  foreach ($pathContract in @(
      @{ Value = [string]$Document.receipt.path; FileName = 'code-hangar-signing-receipt.json'; Label = 'receipt' },
      @{ Value = [string]$Document.server.path; FileName = 'code-hangar-mcp.exe'; Label = 'server' },
      @{ Value = [string]$Document.client.path; FileName = $null; Label = 'client' },
      @{ Value = [string]$Document.liveConfig.root; FileName = $null; Label = 'config root' }
    )) {
    if ([string]::IsNullOrWhiteSpace($pathContract.Value) -or
        -not [System.IO.Path]::IsPathFullyQualified($pathContract.Value) -or
        $pathContract.Value.StartsWith('\\') -or $pathContract.Value.IndexOf(':', 3) -ge 0 -or
        $pathContract.Value -match '[\r\n]') {
      throw "OWNER-02 MCP $($pathContract.Label) path identity is not a fixed local path."
    }
    if ($null -ne $pathContract.FileName -and
        [System.IO.Path]::GetFileName($pathContract.Value) -cne $pathContract.FileName) {
      throw "OWNER-02 MCP $($pathContract.Label) path has the wrong canonical leaf."
    }
  }
  if ([System.IO.Path]::GetExtension([string]$Document.client.path) -ine '.exe' -or
      [string]$Document.liveConfig.leaf -cne '.claude.json') {
    throw 'OWNER-02 client executable or Claude config leaf identity is invalid.'
  }

  Assert-ExactStringSet -Actual @($Document.syntheticFixture.requiredMethods) -Expected @('list_catalog', 'project_context') -Label 'OWNER-02 required MCP methods'
  if (-not [bool]$Document.syntheticFixture.projectObserved -or
      [string]$Document.syntheticFixture.audit -cne 'fixture/mcp-audit-claude.json' -or
      [string]$Document.syntheticFixture.disconnect -cne 'fixture/mcp-disconnect.json') {
    throw 'OWNER-02 synthetic fixture/audit/disconnect proof is incomplete.'
  }

  $configFields = @('exists', 'bytes', 'sha256', 'attributes', 'creationTimeUtc', 'lastWriteTimeUtc')
  foreach ($stateName in @('before', 'after')) {
    $state = $Document.liveConfig.$stateName
    Assert-ExactPropertyNames -Object $state -Expected $configFields -Label "OWNER-02 live config $stateName state"
    if ([bool]$state.exists) {
      Assert-CanonicalSha256 -Value $state.sha256 -Label "OWNER-02 live config $stateName SHA-256"
      if ([long]$state.bytes -le 0 -or [string]::IsNullOrWhiteSpace([string]$state.attributes)) {
        throw "OWNER-02 live config $stateName state has invalid file identity."
      }
      Assert-Timestamp -Value $state.creationTimeUtc -Label "OWNER-02 live config $stateName creation time"
      Assert-Timestamp -Value $state.lastWriteTimeUtc -Label "OWNER-02 live config $stateName write time"
    } elseif ($null -ne $state.bytes -or $null -ne $state.sha256 -or $null -ne $state.attributes -or
        $null -ne $state.creationTimeUtc -or $null -ne $state.lastWriteTimeUtc) {
      throw "OWNER-02 absent live config $stateName state contains invented file metadata."
    }
  }
  $beforeValues = @($configFields | ForEach-Object { $Document.liveConfig.before.$_ }) | ConvertTo-Json -Compress
  $afterValues = @($configFields | ForEach-Object { $Document.liveConfig.after.$_ }) | ConvertTo-Json -Compress
  if (-not [bool]$Document.liveConfig.unchanged -or $beforeValues -cne $afterValues) {
    throw 'OWNER-02 live Claude config was not bit/metadata-identical before and after the test.'
  }
}

function Assert-LocalCiDoesNotClaimPublicationCandidate {
  param([string[]]$TestIds)
  if ($TestIds -ccontains 'AUTO-06/candidate-publication-audit' -or
      @($TestIds | Where-Object { $_ -clike 'AUTO-06/*' -and $_ -cne 'AUTO-06/secret-scan' }).Count -ne 0) {
    throw 'Shared local-CI evidence attempted to issue a publication-candidate claim.'
  }
}

function Read-SharedLocalCiEvidence {
  param([string]$Root, [object]$Identity, [object]$Contracts)
  $relative = [string]$Contracts.value.sharedEvidence.'local-ci'.path
  $evidence = Open-LockedFile -Path (Join-Path $Root $relative) -Label 'Shared local-CI evidence' -MaximumBytes 1048576
  try {
    $report = Read-LockedJson -Evidence $evidence -Label 'Shared local-CI evidence'
    Assert-ExactPropertyNames -Object $report -Expected @(
      'schemaVersion', 'documentType', 'version', 'status', 'completedAtUtc', 'source',
      'invocation', 'isolation', 'completedStepIds', 'testIds'
    ) -Label 'Shared local-CI evidence'
    Assert-ExactPropertyNames -Object $report.source -Expected @('gitCommit', 'gitTree', 'sourceTreeDirty') -Label 'Shared local-CI source'
    Assert-ExactPropertyNames -Object $report.invocation -Expected @('agentAutomation', 'skipTauriBuild', 'coreOnly') -Label 'Shared local-CI invocation'
    Assert-ExactPropertyNames -Object $report.isolation -Expected @('targetTriple', 'cargoOffline', 'npmOffline', 'cargoBuildJobs', 'rustTestThreads') -Label 'Shared local-CI isolation'
    if ([int]$report.schemaVersion -ne 1 -or [string]$report.documentType -cne $script:LocalCiSchema -or
        [string]$report.version -cne $script:ExpectedVersion -or [string]$report.status -cne 'PASS' -or
        [bool]$report.source.sourceTreeDirty -or [string]$report.source.gitCommit -cne $Identity.commit -or
        [string]$report.source.gitTree -cne $Identity.tree -or
        -not [bool]$report.invocation.agentAutomation -or -not [bool]$report.invocation.skipTauriBuild -or
        [bool]$report.invocation.coreOnly -or [string]$report.isolation.targetTriple -cne 'x86_64-pc-windows-msvc' -or
        -not [bool]$report.isolation.cargoOffline -or -not [bool]$report.isolation.npmOffline -or
        [int]$report.isolation.cargoBuildJobs -ne 2 -or [int]$report.isolation.rustTestThreads -ne 2) {
      throw 'Shared local-CI evidence is not the canonical offline AgentAutomation release lane.'
    }
    Assert-Timestamp -Value $report.completedAtUtc -Label 'Shared local-CI completion timestamp'
    $mandatorySteps = @(
      'local-packaging-deterministic-self-tests', 'connector-packaging-deterministic-self-tests',
      'release-pipeline-parser-and-contract-self-test', 'v0-1-3-acceptance-evidence-self-test',
      'v0-1-3-claude-live-client-mcp-self-test', 'release-artifact-proof-self-test',
      'checksum-staging-self-test', 'worktree-javascript-toolchain-preflight', 'npm-run-check',
      'secret-scan-and-worktree-publication-audit', 'frontend-local-edition-isolation',
      'cargo-fmt', 'sandbox-lifecycle-validator-self-test', 'v0-1-3-release-stress-evidence-self-test',
      'v0-1-3-sequential-release-stress-lane', 'cargo-test-core', 'cargo-clippy-core',
      'cargo-test-mutation', 'cargo-clippy-mutation', 'cargo-test-agent-automation',
      'cargo-clippy-agent-automation', 'cargo-clippy-windows-connector-desktop-backend',
      'cargo-test-connected-app-surface', 'cargo-clippy-connected-app-surface',
      'frontend-connector-edition-isolation', 'compile-only-windows-local-release-nonpublishable',
      'compile-only-windows-connector-desktop-release-nonpublishable',
      'compile-only-windows-connected-app-server-release-nonpublishable'
    )
    $stepIds = @($report.completedStepIds | ForEach-Object { [string]$_ })
    if (@($stepIds | Sort-Object -Unique).Count -ne $stepIds.Count) { throw 'Shared local-CI evidence repeats a completed step ID.' }
    Assert-ExactStringSet -Actual $stepIds -Expected $mandatorySteps -Label 'Shared local-CI canonical completed step IDs'
    $testIds = @($report.testIds | ForEach-Object { [string]$_ })
    if (@($testIds | Sort-Object -Unique).Count -ne $testIds.Count) { throw 'Shared local-CI evidence repeats a test ID.' }
    Assert-LocalCiDoesNotClaimPublicationCandidate -TestIds $testIds
    foreach ($contract in @($Contracts.gates | Where-Object {
          [string]$_.mode -ne 'automated' -and @($_.sharedEvidenceIds) -ccontains 'local-ci'
        })) {
      if (@($contract.requiredTestIds | Where-Object { $testIds -ccontains [string]$_ }).Count -lt 1) {
        throw "Shared local-CI evidence supplies none of the required automated test IDs for $($contract.gateId)."
      }
    }
    return [pscustomobject]@{
      relative = $relative
      bytes = $evidence.Bytes
      sha256 = $evidence.Sha256
      report = $report
    }
  } finally { $evidence.Stream.Dispose() }
}

function Assert-PublicationAuditReport {
  param([object]$Report, [object]$Identity)
  Assert-ExactPropertyNames -Object $Report -Expected @(
    'schemaVersion', 'documentType', 'version', 'status', 'completedAtUtc', 'source',
    'invocation', 'topology', 'coverage', 'testIds'
  ) -Label 'Shared publication-audit evidence'
  Assert-ExactPropertyNames -Object $Report.source -Expected @('gitCommit', 'gitTree', 'sourceTreeDirty') -Label 'Publication-audit source'
  Assert-ExactPropertyNames -Object $Report.invocation -Expected @('candidate', 'publicHistory', 'sourceTree') -Label 'Publication-audit invocation'
  Assert-ExactPropertyNames -Object $Report.topology -Expected @(
    'shallow', 'headBranch', 'commitCount', 'rootCount', 'localHeadCount', 'tagCount',
    'remoteCount', 'remoteName', 'fetchUrl', 'pushUrl', 'author', 'committer'
  ) -Label 'Publication-audit topology'
  Assert-ExactPropertyNames -Object $Report.topology.author -Expected @('name', 'email') -Label 'Publication-audit author'
  Assert-ExactPropertyNames -Object $Report.topology.committer -Expected @('name', 'email') -Label 'Publication-audit committer'
  Assert-ExactPropertyNames -Object $Report.coverage -Expected @(
    'trackedFileCount', 'textFileCount', 'pathnamesInspected', 'worktreeContentInspected',
    'historyInspected', 'historyMessagesInspected', 'refsInspected'
  ) -Label 'Publication-audit coverage'

  if ([int]$Report.schemaVersion -ne 1 -or [string]$Report.documentType -cne $script:PublicationAuditSchema -or
      [string]$Report.version -cne $script:ExpectedVersion -or [string]$Report.status -cne 'PASS' -or
      $Report.source.sourceTreeDirty -isnot [bool] -or [bool]$Report.source.sourceTreeDirty -or
      [string]$Report.source.gitCommit -cne $Identity.commit -or [string]$Report.source.gitTree -cne $Identity.tree -or
      $Report.invocation.candidate -isnot [bool] -or -not [bool]$Report.invocation.candidate -or
      $Report.invocation.publicHistory -isnot [bool] -or -not [bool]$Report.invocation.publicHistory -or
      [string]$Report.invocation.sourceTree -cne $Identity.tree) {
    throw 'Shared publication-audit evidence is not a source-bound strict candidate PASS.'
  }
  Assert-Timestamp -Value $Report.completedAtUtc -Label 'Shared publication-audit completion timestamp'

  if ($Report.topology.shallow -isnot [bool] -or [bool]$Report.topology.shallow -or
      [string]$Report.topology.headBranch -cne 'main' -or
      [int]$Report.topology.commitCount -ne 1 -or [int]$Report.topology.rootCount -ne 1 -or
      [int]$Report.topology.localHeadCount -ne 1 -or [int]$Report.topology.tagCount -ne 0 -or
      [int]$Report.topology.remoteCount -ne 1 -or [string]$Report.topology.remoteName -cne 'origin' -or
      [string]$Report.topology.fetchUrl -cne $script:PublicationRepository -or
      [string]$Report.topology.pushUrl -cne $script:PublicationRepository -or
      [string]$Report.topology.author.name -cne $script:PublicationIdentityName -or
      [string]$Report.topology.author.email -cne $script:PublicationIdentityEmail -or
      [string]$Report.topology.committer.name -cne $script:PublicationIdentityName -or
      [string]$Report.topology.committer.email -cne $script:PublicationIdentityEmail) {
    throw 'Shared publication-audit evidence does not prove the exact one-root public topology and identity.'
  }

  if ([int]$Report.coverage.trackedFileCount -le 0 -or [int]$Report.coverage.textFileCount -le 0 -or
      [int]$Report.coverage.textFileCount -gt [int]$Report.coverage.trackedFileCount) {
    throw 'Shared publication-audit evidence has invalid scan coverage counts.'
  }
  foreach ($field in @('pathnamesInspected', 'worktreeContentInspected', 'historyInspected', 'historyMessagesInspected', 'refsInspected')) {
    if ($Report.coverage.$field -isnot [bool] -or -not [bool]$Report.coverage.$field) {
      throw "Shared publication-audit evidence did not prove coverage field $field."
    }
  }
  Assert-ExactStringSet -Actual @($Report.testIds) -Expected @('AUTO-06/candidate-publication-audit') -Label 'Publication-audit test IDs'
}

function Read-SharedPublicationAuditEvidence {
  param([string]$Root, [object]$Identity, [object]$Contracts)
  $relative = [string]$Contracts.value.sharedEvidence.'publication-audit'.path
  $evidence = Open-LockedFile -Path (Join-Path $Root $relative) -Label 'Shared publication-audit evidence' -MaximumBytes 1048576
  try {
    $report = Read-LockedJson -Evidence $evidence -Label 'Shared publication-audit evidence'
    Assert-PublicationAuditReport -Report $report -Identity $Identity
    return [pscustomobject]@{
      relative = $relative
      bytes = $evidence.Bytes
      sha256 = $evidence.Sha256
      report = $report
    }
  } finally { $evidence.Stream.Dispose() }
}

function Assert-AutomatedSharedTestCoverage {
  param([object]$Contracts, [object]$LocalCi, [object]$PublicationAudit)
  $claimsByEvidence = @{
    'local-ci' = @($LocalCi.report.testIds | ForEach-Object { [string]$_ })
    'publication-audit' = @($PublicationAudit.report.testIds | ForEach-Object { [string]$_ })
  }
  foreach ($contract in @($Contracts.gates | Where-Object { [string]$_.mode -eq 'automated' })) {
    $available = [System.Collections.Generic.List[string]]::new()
    foreach ($sharedId in @($contract.sharedEvidenceIds)) {
      if ($claimsByEvidence.ContainsKey([string]$sharedId)) {
        foreach ($claim in @($claimsByEvidence[[string]$sharedId])) { $available.Add([string]$claim) }
      }
    }
    foreach ($required in @($contract.requiredTestIds)) {
      if (@($available) -cnotcontains [string]$required) {
        throw "Automated gate $($contract.gateId) is missing shared-evidence test claim $required."
      }
    }
  }
}

function Assert-NoDuplicateGatePayloadHashes {
  param([object[]]$PrivateGates)
  $seen = @{}
  foreach ($gate in $PrivateGates) {
    foreach ($file in @($gate.evidenceFiles)) {
      $hash = [string]$file.sha256
      if ($seen.ContainsKey($hash)) {
        throw "Gate payload SHA-256 is reused by $($seen[$hash]) and $($gate.id). Reusable evidence belongs in the explicit shared-evidence registry."
      }
      $seen[$hash] = [string]$gate.id
    }
  }
}

function Read-AndValidateGateProof {
  param(
    [string]$Root,
    [object]$Gate,
    [object]$Contract,
    [object]$Identity,
    [string]$SpecificationSha256,
    [string]$ReleaseProofSha256,
    [object]$ReleaseProof
  )
  $proofRelative = "gate-proofs/$($Gate.id).json"
  $proofEvidence = Open-LockedFile -Path (Join-Path $Root $proofRelative) -Label "Gate $($Gate.id) proof" -MaximumBytes 131072
  $payloadLocks = [System.Collections.Generic.List[object]]::new()
  try {
    $proof = Read-LockedJson -Evidence $proofEvidence -Label "Gate $($Gate.id) proof"
    Assert-ExactPropertyNames -Object $proof -Expected @(
      'schemaVersion', 'documentType', 'version', 'gateId', 'status', 'source',
      'specificationSha256', 'producer', 'testIds', 'sharedEvidenceIds', 'evidenceFiles'
    ) -Label "Gate $($Gate.id) proof"
    Assert-ExactPropertyNames -Object $proof.source -Expected @('gitCommit', 'gitTree') -Label "Gate $($Gate.id) proof source"
    if ([int]$proof.schemaVersion -ne 2 -or [string]$proof.documentType -cne $script:GateProofSchema -or
        [string]$proof.version -cne $script:ExpectedVersion -or [string]$proof.gateId -cne [string]$Gate.id -or
        [string]$proof.status -cne 'PASS' -or
        [string]$proof.source.gitCommit -cne $Identity.commit -or [string]$proof.source.gitTree -cne $Identity.tree -or
        [string]$proof.specificationSha256 -cne $SpecificationSha256) {
      throw "Gate $($Gate.id) proof is not a source-bound PASS from its required producer."
    }
    Assert-ProducerContract -Producer $proof.producer -Contract $Contract -GateId $Gate.id
    Assert-ExactStringSet -Actual @($proof.testIds) -Expected @($Contract.requiredTestIds) -Label "Gate $($Gate.id) test IDs"
    Assert-ExactStringSet -Actual @($proof.sharedEvidenceIds) -Expected @($Contract.sharedEvidenceIds) -Label "Gate $($Gate.id) shared evidence IDs"
    $records = @($proof.evidenceFiles)
    $requiredDocumentTypes = @($Contract.requiredDocumentTypes)
    if ($records.Count -ne $requiredDocumentTypes.Count) {
      throw "Gate $($Gate.id) must bind exactly $($requiredDocumentTypes.Count) canonical gate-result document(s)."
    }
    $paths = @($records | ForEach-Object { [string]$_.path })
    if (@($paths | Sort-Object -Unique).Count -ne $paths.Count) { throw "Gate $($Gate.id) repeats an evidence path." }
    Assert-ExactStringSet -Actual @($records | ForEach-Object { [string]$_.documentType }) -Expected $requiredDocumentTypes -Label "Gate $($Gate.id) result document types"
    foreach ($record in $records) {
      $lock = Open-GateEvidenceFile -Root $Root -GateId $Gate.id -Record $record
      $payloadLocks.Add($lock)
      $document = Read-LockedJson -Evidence $lock -Label "Gate $($Gate.id) $($record.documentType) result"
      if ([string]$record.documentType -in @($script:OwnerAttestationSchema, $script:SupervisedAttestationSchema)) {
        Assert-ManualAttestation `
          -Document $document `
          -Contract $Contract `
          -Identity $Identity `
          -SpecificationSha256 $SpecificationSha256 `
          -ReleaseProofSha256 $ReleaseProofSha256 `
          -GateId $Gate.id
      } elseif ([string]$record.documentType -eq 'codehangar/claude-live-mcp-acceptance/3') {
        Assert-ClaudeLiveMcpReport -Document $document -Identity $Identity -ReleaseBindings $ReleaseProof.report.bindings
      } else {
        throw "Gate $($Gate.id) has no canonical validator for document type $($record.documentType)."
      }
    }

    foreach ($resultId in @($Contract.requiredLifecycleResultIds)) {
      if (@($ReleaseProof.report.bindings.lifecycle.resultIds) -cnotcontains [string]$resultId) {
        throw "Gate $($Gate.id) lifecycle proof is missing required result ID $resultId."
      }
    }

    # OWNER-02 is not satisfied by prose: it must bind the exact private
    # schema-3 real-client report produced by mcp-claude-real.ps1.
    if ($Gate.id -eq 'OWNER-02') {
      $mcpRecords = @($records | Where-Object { [System.IO.Path]::GetFileName([string]$_.path) -ceq 'claude-live-mcp.private.json' })
      if ($mcpRecords.Count -ne 1) { throw 'OWNER-02 requires exactly one claude-live-mcp.private.json evidence file.' }
      $mcpLock = @($payloadLocks | Where-Object { $_.Path.EndsWith('claude-live-mcp.private.json', [System.StringComparison]::Ordinal) })
      if ($mcpLock.Count -ne 1) { throw 'OWNER-02 MCP evidence lock is ambiguous.' }
    }

    $inventory = @($records | ForEach-Object {
      [ordered]@{
        role = [string]$_.role
        documentType = [string]$_.documentType
        path = ([string]$_.path).Replace('\', '/')
        bytes = [long]$_.bytes
        sha256 = [string]$_.sha256
      }
    })
    $digestLines = @($inventory | Sort-Object path | ForEach-Object { "$($_.path)`t$($_.bytes)`t$($_.sha256)" })
    return [pscustomobject]@{
      private = [ordered]@{
        id = [string]$Gate.id
        name = [string]$Gate.name
        rule = [string]$Gate.rule
        requirement = [string]$Gate.requirement
        status = 'PASS'
        mode = [string]$Contract.mode
        producer = [ordered]@{ id = [string]$proof.producer.id; schema = [string]$proof.producer.schema }
        testIds = @($proof.testIds)
        sharedEvidenceIds = @($proof.sharedEvidenceIds)
        proof = [ordered]@{ path = $proofRelative; bytes = $proofEvidence.Bytes; sha256 = $proofEvidence.Sha256 }
        evidenceFiles = $inventory
      }
      public = [ordered]@{
        id = [string]$Gate.id
        status = 'PASS'
        mode = [string]$Contract.mode
        producerId = [string]$proof.producer.id
        producerSchema = [string]$proof.producer.schema
        proofSha256 = $proofEvidence.Sha256
        evidenceSetSha256 = Get-TextSha256 -Text (($digestLines -join "`n") + "`n")
      }
    }
  } finally {
    foreach ($lock in $payloadLocks) { $lock.Stream.Dispose() }
    $proofEvidence.Stream.Dispose()
  }
}

function Read-ValidatedReleaseProof {
  param([string]$Directory, [string]$ExpectedHash, [object]$Identity)
  Assert-CanonicalSha256 -Value $ExpectedHash -Label 'Expected release artifact proof hash'
  $pwshPath = Join-Path $PSHOME 'pwsh.exe'
  & $pwshPath -NoProfile -File (Join-Path $PSScriptRoot 'release-artifact-proof.ps1') `
    -ValidateOnly -EvidenceDir $Directory -ExpectedReportSha256 $ExpectedHash | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'Release artifact proof failed immediate validation.' }
  $reportPath = Join-Path $Directory 'RELEASE-ARTIFACT-PROOF.private.json'
  $evidence = Open-LockedFile -Path $reportPath -Label 'Release artifact proof report' -MaximumBytes 1048576
  try {
    if ($evidence.Sha256 -cne $ExpectedHash) { throw 'Release artifact proof changed after validation.' }
    $report = Read-LockedJson -Evidence $evidence -Label 'Release artifact proof report'
    if ([int]$report.schemaVersion -ne 1 -or [string]$report.documentType -cne $script:ReleaseProofSchema -or
        [string]$report.version -cne $script:ExpectedVersion -or [string]$report.status -cne 'PASS' -or
        [string]$report.source.gitCommit -cne $Identity.commit -or [string]$report.source.gitTree -cne $Identity.tree) {
      throw 'Release artifact proof does not match this acceptance source/version.'
    }
    return [pscustomobject]@{ sha256 = $evidence.Sha256; report = $report }
  } finally {
    $evidence.Stream.Dispose()
  }
}

function Get-ReleaseBindingsProjection {
  param([object]$ReleaseProof)
  $bindings = $ReleaseProof.report.bindings
  return [ordered]@{
    artifactProof = [ordered]@{ schema = [string]$ReleaseProof.report.documentType; sha256 = [string]$ReleaseProof.sha256 }
    signingDecision = [ordered]@{
      value = [string]$bindings.signingDecision.value
      ownerAuthorized = [bool]$bindings.signingDecision.ownerAuthorized
      unsignedOuterAccepted = [bool]$bindings.signingDecision.unsignedOuterAccepted
      smartScreenDisclosureRequired = [bool]$bindings.signingDecision.smartScreenDisclosureRequired
      signerSubject = [string]$bindings.signingDecision.signerSubject
      signerThumbprint = [string]$bindings.signingDecision.signerThumbprint
    }
    lifecycle = [ordered]@{
      schemaVersion = [int]$bindings.lifecycle.schemaVersion
      sha256 = [string]$bindings.lifecycle.sha256
      localSetupSha256 = [string]$bindings.lifecycle.localSetupSha256
      connectorSetupSha256 = [string]$bindings.lifecycle.connectorSetupSha256
    }
    editions = [ordered]@{
      Local = [ordered]@{
        signingReceiptSha256 = [string]$bindings.editions.Local.signingReceipt.sha256
        releaseIdentitySha256 = [string]$bindings.editions.Local.releaseIdentity.sha256
        releaseId = [string]$bindings.editions.Local.releaseIdentity.releaseId
        setupSha256 = [string]$bindings.editions.Local.artifacts.setup.sha256
        parentSha256 = [string]$bindings.editions.Local.artifacts.parent.sha256
        helperSha256 = [string]$bindings.editions.Local.artifacts.helper.sha256
        uninstallerSha256 = [string]$bindings.editions.Local.artifacts.uninstaller.sha256
      }
      Connector = [ordered]@{
        signingReceiptSha256 = [string]$bindings.editions.Connector.signingReceipt.sha256
        releaseIdentitySha256 = [string]$bindings.editions.Connector.releaseIdentity.sha256
        releaseId = [string]$bindings.editions.Connector.releaseIdentity.releaseId
        setupSha256 = [string]$bindings.editions.Connector.artifacts.setup.sha256
        parentSha256 = [string]$bindings.editions.Connector.artifacts.parent.sha256
        helperSha256 = [string]$bindings.editions.Connector.artifacts.helper.sha256
        uninstallerSha256 = [string]$bindings.editions.Connector.artifacts.uninstaller.sha256
        mcpSha256 = [string]$bindings.editions.Connector.artifacts.mcp.sha256
        mcpReceiptSha256 = [string]$bindings.editions.Connector.artifacts.mcp.receiptSha256
      }
    }
  }
}

function Initialize-Acceptance {
  param([string]$Root)
  if (Test-Path -LiteralPath $Root) { throw 'Initialization requires a new EvidenceDir.' }
  $identity = Get-CleanGitIdentity
  $specification = Read-AcceptanceSpecification
  $contracts = Read-GateContracts -Specification $specification
  $specHash = (Get-FileHash -LiteralPath $specPath -Algorithm SHA256).Hash.ToLowerInvariant()
  [void][System.IO.Directory]::CreateDirectory($Root)
  [void][System.IO.Directory]::CreateDirectory((Join-Path $Root 'gate-proofs'))
  [void][System.IO.Directory]::CreateDirectory((Join-Path $Root 'evidence'))
  [void][System.IO.Directory]::CreateDirectory((Join-Path $Root 'shared-evidence'))
  $index = [ordered]@{
    schemaVersion = 3
    documentType = $script:DraftSchema
    version = $script:ExpectedVersion
    status = 'DRAFT'
    createdAt = (Get-Date).ToString('o')
    source = [ordered]@{ gitCommit = $identity.commit; gitTree = $identity.tree; gitBranch = $identity.branch; sourceTreeDirty = $false }
    specification = [ordered]@{ path = 'docs/qa/v0.1.3-acceptance.md'; sha256 = $specHash }
    contracts = [ordered]@{ path = 'scripts/release-gate-contracts.json'; sha256 = $contracts.sha256 }
    sharedEvidence = @(
      [ordered]@{
        id = 'local-ci'
        path = [string]$contracts.value.sharedEvidence.'local-ci'.path
        documentType = [string]$contracts.value.sharedEvidence.'local-ci'.documentType
      }
      [ordered]@{
        id = 'publication-audit'
        path = [string]$contracts.value.sharedEvidence.'publication-audit'.path
        documentType = [string]$contracts.value.sharedEvidence.'publication-audit'.documentType
      }
    )
    gates = @($specification | ForEach-Object { [ordered]@{ id = $_.id; proof = "gate-proofs/$($_.id).json" } })
  }
  Write-NewJson -Path (Join-Path $Root $script:DraftName) -Value $index
  foreach ($gate in $specification) {
    $contract = Get-GateContract -Contracts $contracts -GateId $gate.id
    [void][System.IO.Directory]::CreateDirectory((Join-Path $Root "evidence\$($gate.id)"))
    $proof = [ordered]@{
      schemaVersion = 2
      documentType = $script:GateProofSchema
      version = $script:ExpectedVersion
      gateId = $gate.id
      status = 'PENDING'
      source = [ordered]@{ gitCommit = $identity.commit; gitTree = $identity.tree }
      specificationSha256 = $specHash
      producer = [ordered]@{ id = [string]$contract.producer.id; schema = [string]$contract.producer.schema }
      testIds = @($contract.requiredTestIds)
      sharedEvidenceIds = @($contract.sharedEvidenceIds)
      evidenceFiles = @()
    }
    Write-NewJson -Path (Join-Path $Root "gate-proofs\$($gate.id).json") -Value $proof
  }
  Write-Host "Private acceptance workspace initialized: $Root" -ForegroundColor Green
  Write-Host 'Automated gates consume canonical shared local-CI/publication/release/lifecycle outputs; manual gates require exact typed attestations. Generic per-gate files are rejected.' -ForegroundColor Yellow
}

function Get-ValidatedAcceptanceState {
  param([string]$Root, [string]$ProofDirectory, [string]$ProofHash)
  $identity = Get-CleanGitIdentity
  $specification = Read-AcceptanceSpecification
  $contracts = Read-GateContracts -Specification $specification
  $specHash = (Get-FileHash -LiteralPath $specPath -Algorithm SHA256).Hash.ToLowerInvariant()
  $draftEvidence = Open-LockedFile -Path (Join-Path $Root $script:DraftName) -Label 'Private acceptance index' -MaximumBytes 262144
  try {
    $draft = Read-LockedJson -Evidence $draftEvidence -Label 'Private acceptance index'
    Assert-ExactPropertyNames -Object $draft -Expected @(
      'schemaVersion', 'documentType', 'version', 'status', 'createdAt', 'source',
      'specification', 'contracts', 'sharedEvidence', 'gates'
    ) -Label 'Private acceptance index'
    if ([int]$draft.schemaVersion -ne 3 -or [string]$draft.documentType -cne $script:DraftSchema -or
        [string]$draft.version -cne $script:ExpectedVersion -or [string]$draft.status -cne 'DRAFT' -or
        [string]$draft.source.gitCommit -cne $identity.commit -or [string]$draft.source.gitTree -cne $identity.tree -or
        [string]$draft.specification.path -cne 'docs/qa/v0.1.3-acceptance.md' -or
        [string]$draft.specification.sha256 -cne $specHash -or
        [string]$draft.contracts.path -cne 'scripts/release-gate-contracts.json' -or
        [string]$draft.contracts.sha256 -cne $contracts.sha256) {
      throw 'Private acceptance index is not bound to this exact source/specification.'
    }
    if (@($draft.sharedEvidence).Count -ne 2 -or [string]$draft.sharedEvidence[0].id -cne 'local-ci' -or
        [string]$draft.sharedEvidence[0].path -cne [string]$contracts.value.sharedEvidence.'local-ci'.path -or
        [string]$draft.sharedEvidence[0].documentType -cne $script:LocalCiSchema -or
        [string]$draft.sharedEvidence[1].id -cne 'publication-audit' -or
        [string]$draft.sharedEvidence[1].path -cne [string]$contracts.value.sharedEvidence.'publication-audit'.path -or
        [string]$draft.sharedEvidence[1].documentType -cne $script:PublicationAuditSchema) {
      throw 'Private acceptance index shared-evidence registry changed.'
    }
    Assert-ExactGateIndex -Actual @($draft.gates) -Specification $specification
    $releaseProof = Read-ValidatedReleaseProof -Directory $ProofDirectory -ExpectedHash $ProofHash -Identity $identity
    $releaseBindings = Get-ReleaseBindingsProjection -ReleaseProof $releaseProof
    $localCi = Read-SharedLocalCiEvidence -Root $Root -Identity $identity -Contracts $contracts
    $publicationAudit = Read-SharedPublicationAuditEvidence -Root $Root -Identity $identity -Contracts $contracts
    Assert-AutomatedSharedTestCoverage -Contracts $contracts -LocalCi $localCi -PublicationAudit $publicationAudit
    $privateGates = [System.Collections.Generic.List[object]]::new()
    $publicGates = [System.Collections.Generic.List[object]]::new()
    foreach ($gate in $specification) {
      $contract = Get-GateContract -Contracts $contracts -GateId $gate.id
      $validated = Read-AndValidateGateProof `
        -Root $Root `
        -Gate $gate `
        -Contract $contract `
        -Identity $identity `
        -SpecificationSha256 $specHash `
        -ReleaseProofSha256 $releaseProof.sha256 `
        -ReleaseProof $releaseProof
      $privateGates.Add($validated.private)
      $publicGates.Add($validated.public)
    }

    # OWNER-02 must additionally bind the exact MCP receipt and sidecar already
    # accepted by the release artifact proof, not merely any passing MCP report.
    $owner02 = $privateGates | Where-Object id -eq 'OWNER-02'
    $mcpEvidenceRecord = @($owner02.evidenceFiles | Where-Object { [System.IO.Path]::GetFileName([string]$_.path) -ceq 'claude-live-mcp.private.json' })
    $mcpEvidence = Open-LockedFile -Path (Join-Path $Root $mcpEvidenceRecord[0].path) -Label 'OWNER-02 MCP binding' -MaximumBytes 1048576
    try {
      $mcp = Read-LockedJson -Evidence $mcpEvidence -Label 'OWNER-02 MCP binding'
      if ([string]$mcp.receipt.sha256 -cne [string]$releaseBindings.editions.Connector.signingReceiptSha256 -or
          [string]$mcp.server.sha256 -cne [string]$releaseBindings.editions.Connector.mcpSha256 -or
          [string]$mcp.server.receiptSha256 -cne [string]$releaseBindings.editions.Connector.mcpReceiptSha256) {
        throw 'OWNER-02 MCP report does not bind the exact released Connector receipt/sidecar.'
      }
    } finally {
      $mcpEvidence.Stream.Dispose()
    }
    Assert-NoDuplicateGatePayloadHashes -PrivateGates @($privateGates)

    return [pscustomobject]@{
      identity = $identity
      specificationSha256 = $specHash
      contractsSha256 = $contracts.sha256
      draft = $draft
      draftSha256 = $draftEvidence.Sha256
      privateGates = @($privateGates)
      publicGates = @($publicGates)
      sharedEvidence = @(
        [ordered]@{
          id = 'local-ci'
          documentType = $script:LocalCiSchema
          path = $localCi.relative
          bytes = $localCi.bytes
          sha256 = $localCi.sha256
        }
        [ordered]@{
          id = 'publication-audit'
          documentType = $script:PublicationAuditSchema
          path = $publicationAudit.relative
          bytes = $publicationAudit.bytes
          sha256 = $publicationAudit.sha256
        }
      )
      releaseBindings = $releaseBindings
    }
  } finally {
    $draftEvidence.Stream.Dispose()
  }
}

function Get-ExpectedPrivateInventory {
  param([object]$State)
  $expected = [System.Collections.Generic.List[string]]::new()
  $expected.Add($script:DraftName)
  $expected.Add($script:PrivateReportName)
  foreach ($shared in @($State.sharedEvidence)) { $expected.Add([string]$shared.path) }
  foreach ($gate in $State.privateGates) {
    $expected.Add([string]$gate.proof.path)
    foreach ($file in $gate.evidenceFiles) { $expected.Add([string]$file.path) }
  }
  return @($expected | Sort-Object)
}

function Get-EvidenceInventory {
  param([object]$State)
  $records = [System.Collections.Generic.List[object]]::new()
  foreach ($shared in @($State.sharedEvidence)) {
    $records.Add([ordered]@{
        role = "shared-evidence:$($shared.id)"
        documentType = [string]$shared.documentType
        path = [string]$shared.path
        bytes = [long]$shared.bytes
        sha256 = [string]$shared.sha256
      })
  }
  foreach ($gate in $State.privateGates) {
    $records.Add([ordered]@{ role = "gate-proof:$($gate.id)"; path = $gate.proof.path; bytes = $gate.proof.bytes; sha256 = $gate.proof.sha256 })
    foreach ($file in $gate.evidenceFiles) {
      $records.Add([ordered]@{ role = "gate-evidence:$($gate.id)"; documentType = $file.documentType; path = $file.path; bytes = $file.bytes; sha256 = $file.sha256 })
    }
  }
  return @($records | Sort-Object path)
}

function Finalize-Acceptance {
  param([string]$Root, [string]$ProofDirectory, [string]$ProofHash)
  $reportPath = Join-Path $Root $script:PrivateReportName
  if (Test-Path -LiteralPath $reportPath) { throw 'Private acceptance report already exists; choose a new run instead of overwriting.' }
  $state = Get-ValidatedAcceptanceState -Root $Root -ProofDirectory $ProofDirectory -ProofHash $ProofHash
  $inventory = Get-EvidenceInventory -State $state
  $report = [ordered]@{
    schemaVersion = 3
    documentType = $script:PrivateReportSchema
    version = $script:ExpectedVersion
    status = 'PASS'
    sealedAt = (Get-Date).ToString('o')
    source = [ordered]@{ gitCommit = $state.identity.commit; gitTree = $state.identity.tree; gitBranch = $state.identity.branch; sourceTreeDirty = $false }
    specification = [ordered]@{ path = 'docs/qa/v0.1.3-acceptance.md'; sha256 = $state.specificationSha256 }
    contracts = [ordered]@{ path = 'scripts/release-gate-contracts.json'; sha256 = $state.contractsSha256 }
    draftSha256 = $state.draftSha256
    sharedEvidence = $state.sharedEvidence
    releaseBindings = $state.releaseBindings
    gates = $state.privateGates
    evidenceInventory = $inventory
  }
  Write-NewJson -Path $reportPath -Value $report -Depth 18
  $reportEvidence = Open-LockedFile -Path $reportPath -Label 'Private acceptance report' -MaximumBytes 4194304
  try { $reportHash = $reportEvidence.Sha256 } finally { $reportEvidence.Stream.Dispose() }
  # Validate against the independently supplied release-proof hash before
  # returning the private report hash to be recorded out-of-band.
  [void](Validate-Acceptance -Root $Root -ProofDirectory $ProofDirectory -ProofHash $ProofHash -ExpectedReportHash $reportHash)
  Write-Host "Sealed private acceptance report: $reportPath" -ForegroundColor Green
  Write-Host "PRIVATE ACCEPTANCE SHA-256: $reportHash" -ForegroundColor Green
}

function Validate-Acceptance {
  param([string]$Root, [string]$ProofDirectory, [string]$ProofHash, [string]$ExpectedReportHash)
  Assert-CanonicalSha256 -Value $ExpectedReportHash -Label 'Expected private acceptance report hash'
  $state = Get-ValidatedAcceptanceState -Root $Root -ProofDirectory $ProofDirectory -ProofHash $ProofHash
  $reportEvidence = Open-LockedFile -Path (Join-Path $Root $script:PrivateReportName) -Label 'Private acceptance report' -MaximumBytes 4194304
  try {
    if ($reportEvidence.Sha256 -cne $ExpectedReportHash) { throw 'Private acceptance report hash mismatch.' }
    $report = Read-LockedJson -Evidence $reportEvidence -Label 'Private acceptance report'
    Assert-ExactPropertyNames -Object $report -Expected @(
      'schemaVersion', 'documentType', 'version', 'status', 'sealedAt', 'source',
      'specification', 'contracts', 'draftSha256', 'sharedEvidence', 'releaseBindings',
      'gates', 'evidenceInventory'
    ) -Label 'Private acceptance report'
    if ([int]$report.schemaVersion -ne 3 -or [string]$report.documentType -cne $script:PrivateReportSchema -or
        [string]$report.version -cne $script:ExpectedVersion -or [string]$report.status -cne 'PASS' -or
        [string]$report.source.gitCommit -cne $state.identity.commit -or [string]$report.source.gitTree -cne $state.identity.tree -or
        [string]$report.specification.sha256 -cne $state.specificationSha256 -or
        [string]$report.contracts.sha256 -cne $state.contractsSha256 -or
        [string]$report.draftSha256 -cne $state.draftSha256) {
      throw 'Private acceptance report source/specification binding is invalid.'
    }
    if ((@($report.sharedEvidence) | ConvertTo-Json -Depth 8 -Compress) -cne (@($state.sharedEvidence) | ConvertTo-Json -Depth 8 -Compress) -or
        ($report.releaseBindings | ConvertTo-Json -Depth 12 -Compress) -cne ($state.releaseBindings | ConvertTo-Json -Depth 12 -Compress) -or
        (@($report.gates) | ConvertTo-Json -Depth 15 -Compress) -cne ($state.privateGates | ConvertTo-Json -Depth 15 -Compress) -or
        (@($report.evidenceInventory) | ConvertTo-Json -Depth 8 -Compress) -cne ((Get-EvidenceInventory -State $state) | ConvertTo-Json -Depth 8 -Compress)) {
      throw 'Private acceptance report no longer matches its release bindings or gate evidence.'
    }
    $actualInventory = @(Get-NonReparseFileInventory -Root $Root | ForEach-Object { $_.relative } | Sort-Object)
    $expectedInventory = @(Get-ExpectedPrivateInventory -State $state)
    if (($actualInventory -join "`n") -cne ($expectedInventory -join "`n")) {
      throw 'Acceptance directory contains a missing or unbound file.'
    }
    return [pscustomobject]@{ report = $report; reportSha256 = $reportEvidence.Sha256; state = $state }
  } finally {
    $reportEvidence.Stream.Dispose()
  }
}

function Get-PublicProjection {
  param([object]$Validated)
  $inventoryLines = @($Validated.report.evidenceInventory | Sort-Object role, path | ForEach-Object {
      "$($_.role)`t$($_.path)`t$($_.bytes)`t$($_.sha256)"
    })
  return [ordered]@{
    schemaVersion = 1
    documentType = $script:PublicReportSchema
    version = $script:ExpectedVersion
    status = 'PASS'
    source = [ordered]@{
      gitCommit = [string]$Validated.report.source.gitCommit
      gitTree = [string]$Validated.report.source.gitTree
      sourceTreeDirty = $false
    }
    specification = [ordered]@{
      id = 'v0.1.3-50-gate-specification'
      sha256 = [string]$Validated.report.specification.sha256
    }
    contracts = [ordered]@{
      schema = $script:ContractSchema
      sha256 = [string]$Validated.report.contracts.sha256
    }
    privateEvidenceSha256 = [string]$Validated.reportSha256
    sharedEvidence = @($Validated.report.sharedEvidence | ForEach-Object {
        [ordered]@{ id = [string]$_.id; documentType = [string]$_.documentType; bytes = [long]$_.bytes; sha256 = [string]$_.sha256 }
      })
    releaseBindings = $Validated.report.releaseBindings
    gateCount = 50
    gates = $Validated.state.publicGates
    evidenceInventorySha256 = Get-TextSha256 -Text (($inventoryLines -join "`n") + "`n")
  }
}

if ($SelfTest) {
  if ($Initialize -or $Finalize -or $ValidateOnly -or $ExportPublicProjection) {
    throw '-SelfTest accepts no acceptance mode.'
  }
  $specification = Read-AcceptanceSpecification
  $contracts = Read-GateContracts -Specification $specification
  $index = @($specification | ForEach-Object { [pscustomobject]@{ id = $_.id; proof = "gate-proofs/$($_.id).json" } })
  Assert-ExactGateIndex -Actual $index -Specification $specification
  $owner02Contract = Get-GateContract -Contracts $contracts -GateId 'OWNER-02'
  $wrongProducerSchemaRejected = $false
  try {
    Assert-ProducerContract `
      -Producer ([pscustomobject]@{ id = 'owner-live-client'; schema = 'codehangar/arbitrary/1' }) `
      -Contract $owner02Contract `
      -GateId 'OWNER-02'
  } catch { $wrongProducerSchemaRejected = $true }
  if (-not $wrongProducerSchemaRejected) { throw 'Acceptance self-test accepted a correct producer string with the wrong producer schema.' }

  $missingTestIdRejected = $false
  try {
    $safe05Contract = Get-GateContract -Contracts $contracts -GateId 'SAFE-05'
    Assert-ExactStringSet -Actual @() -Expected @($safe05Contract.requiredTestIds) -Label 'self-test required test IDs'
  } catch { $missingTestIdRejected = $true }
  if (-not $missingTestIdRejected) { throw 'Acceptance self-test accepted a missing required test ID.' }

  $auto06Contract = Get-GateContract -Contracts $contracts -GateId 'AUTO-06'
  Assert-Auto06Contract -Contract $auto06Contract
  $worktreeAuto06 = ($auto06Contract | ConvertTo-Json -Depth 8) | ConvertFrom-Json -DateKind String
  $worktreeAuto06.producer.id = 'local-ci'
  $worktreeAuto06.producer.schema = $script:LocalCiSchema
  $worktreeAuto06Rejected = $false
  try { Assert-Auto06Contract -Contract $worktreeAuto06 } catch { $worktreeAuto06Rejected = $true }
  if (-not $worktreeAuto06Rejected) { throw 'Acceptance self-test allowed local-CI to become the AUTO-06 candidate producer.' }

  $candidateClaimInLocalCiRejected = $false
  try {
    Assert-LocalCiDoesNotClaimPublicationCandidate -TestIds @(
      'AUTO-06/secret-scan',
      'AUTO-06/candidate-publication-audit'
    )
  } catch { $candidateClaimInLocalCiRejected = $true }
  if (-not $candidateClaimInLocalCiRejected) { throw 'Acceptance self-test allowed worktree local-CI to issue the candidate publication claim.' }
  Assert-LocalCiDoesNotClaimPublicationCandidate -TestIds @('AUTO-06/secret-scan')

  $publicationIdentity = [pscustomobject]@{ commit = ('a' * 40); tree = ('b' * 40) }
  $publicationReport = [pscustomobject][ordered]@{
    schemaVersion = 1
    documentType = $script:PublicationAuditSchema
    version = $script:ExpectedVersion
    status = 'PASS'
    completedAtUtc = '2026-08-28T00:00:00.0000000Z'
    source = [ordered]@{ gitCommit = $publicationIdentity.commit; gitTree = $publicationIdentity.tree; sourceTreeDirty = $false }
    invocation = [ordered]@{ candidate = $true; publicHistory = $true; sourceTree = $publicationIdentity.tree }
    topology = [ordered]@{
      shallow = $false; headBranch = 'main'; commitCount = 1; rootCount = 1; localHeadCount = 1; tagCount = 0
      remoteCount = 1; remoteName = 'origin'; fetchUrl = $script:PublicationRepository; pushUrl = $script:PublicationRepository
      author = [ordered]@{ name = $script:PublicationIdentityName; email = $script:PublicationIdentityEmail }
      committer = [ordered]@{ name = $script:PublicationIdentityName; email = $script:PublicationIdentityEmail }
    }
    coverage = [ordered]@{
      trackedFileCount = 100; textFileCount = 80; pathnamesInspected = $true; worktreeContentInspected = $true
      historyInspected = $true; historyMessagesInspected = $true; refsInspected = $true
    }
    testIds = @('AUTO-06/candidate-publication-audit')
  }
  $publicationReport = ($publicationReport | ConvertTo-Json -Depth 10) | ConvertFrom-Json -DateKind String
  Assert-PublicationAuditReport -Report $publicationReport -Identity $publicationIdentity

  foreach ($mutation in @(
      @{ Name = 'dirty source'; Apply = { param($value) $value.source.sourceTreeDirty = $true } },
      @{ Name = 'non-candidate invocation'; Apply = { param($value) $value.invocation.candidate = $false } },
      @{ Name = 'tagged topology'; Apply = { param($value) $value.topology.tagCount = 1 } },
      @{ Name = 'missing pathname coverage'; Apply = { param($value) $value.coverage.pathnamesInspected = $false } },
      @{ Name = 'worktree-issued claim'; Apply = { param($value) $value.testIds = @('AUTO-06/secret-scan') } }
    )) {
    $invalidPublicationReport = ($publicationReport | ConvertTo-Json -Depth 10) | ConvertFrom-Json -DateKind String
    & $mutation.Apply $invalidPublicationReport
    $invalidPublicationRejected = $false
    try { Assert-PublicationAuditReport -Report $invalidPublicationReport -Identity $publicationIdentity } catch { $invalidPublicationRejected = $true }
    if (-not $invalidPublicationRejected) { throw "Acceptance self-test accepted publication evidence with $($mutation.Name)." }
  }

  $auto06OnlyContracts = [pscustomobject]@{ gates = @($auto06Contract) }
  $fakeLocalCi = [pscustomobject]@{ report = [pscustomobject]@{ testIds = @('AUTO-06/secret-scan') } }
  $fakePublicationAudit = [pscustomobject]@{ report = $publicationReport }
  Assert-AutomatedSharedTestCoverage -Contracts $auto06OnlyContracts -LocalCi $fakeLocalCi -PublicationAudit $fakePublicationAudit
  $missingCandidateAudit = [pscustomobject]@{ report = [pscustomobject]@{ testIds = @() } }
  $missingCandidateAuditRejected = $false
  try {
    Assert-AutomatedSharedTestCoverage -Contracts $auto06OnlyContracts -LocalCi $fakeLocalCi -PublicationAudit $missingCandidateAudit
  } catch { $missingCandidateAuditRejected = $true }
  if (-not $missingCandidateAuditRejected) { throw 'Acceptance self-test sealed AUTO-06 without candidate publication evidence.' }

  $safe06Contract = Get-GateContract -Contracts $contracts -GateId 'SAFE-06'
  Assert-Safe06Contract -Contract $safe06Contract
  $automatedSafe06 = ($safe06Contract | ConvertTo-Json -Depth 8) | ConvertFrom-Json -DateKind String
  $automatedSafe06.mode = 'automated'
  $automatedSafe06Rejected = $false
  try { Assert-Safe06Contract -Contract $automatedSafe06 } catch { $automatedSafe06Rejected = $true }
  if (-not $automatedSafe06Rejected) { throw 'Acceptance self-test allowed SAFE-06 to regress to an automated gate.' }

  $safe06Identity = [pscustomobject]@{ commit = ('6' * 40); tree = ('7' * 40) }
  $safe06SpecificationSha256 = ('86' * 32)
  $safe06ReleaseProofSha256 = ('97' * 32)
  $safe06Attestation = [pscustomobject][ordered]@{
    schemaVersion = 1
    documentType = $script:SupervisedAttestationSchema
    version = $script:ExpectedVersion
    gateId = 'SAFE-06'
    status = 'PASS'
    attestedAtUtc = '2026-08-28T00:00:00.0000000Z'
    source = [ordered]@{
      gitCommit = $safe06Identity.commit
      gitTree = $safe06Identity.tree
      sourceTreeDirty = $false
    }
    specificationSha256 = $safe06SpecificationSha256
    releaseArtifactProofSha256 = $safe06ReleaseProofSha256
    attestor = [ordered]@{ kind = 'supervisor'; name = 'SAFE-06 self-test supervisor' }
    decision = 'APPROVE'
    testIds = @($safe06Contract.requiredTestIds)
  }
  $safe06Attestation = ($safe06Attestation | ConvertTo-Json -Depth 8) | ConvertFrom-Json -DateKind String
  Assert-ManualAttestation `
    -Document $safe06Attestation `
    -Contract $safe06Contract `
    -Identity $safe06Identity `
    -SpecificationSha256 $safe06SpecificationSha256 `
    -ReleaseProofSha256 $safe06ReleaseProofSha256 `
    -GateId 'SAFE-06'
  $missingSafe06Branch = ($safe06Attestation | ConvertTo-Json -Depth 8) | ConvertFrom-Json -DateKind String
  $missingSafe06Branch.testIds = @(
    'SAFE-06/extended-disposition-ntfs',
    'SAFE-06/legacy-disposition-ntfs'
  )
  $missingSafe06BranchRejected = $false
  try {
    Assert-ManualAttestation `
      -Document $missingSafe06Branch `
      -Contract $safe06Contract `
      -Identity $safe06Identity `
      -SpecificationSha256 $safe06SpecificationSha256 `
      -ReleaseProofSha256 $safe06ReleaseProofSha256 `
      -GateId 'SAFE-06'
  } catch { $missingSafe06BranchRejected = $true }
  if (-not $missingSafe06BranchRejected) { throw 'Acceptance self-test accepted SAFE-06 without the abrupt-termination result.' }

  $fakeMcpIdentity = [pscustomobject]@{ commit = ('a' * 40); tree = ('b' * 40) }
  $fakeMcpBindings = [pscustomobject]@{
    editions = [pscustomobject]@{
      Connector = [pscustomobject]@{
        signingReceipt = [pscustomobject]@{ sha256 = ('11' * 32) }
        artifacts = [pscustomobject]@{
          mcp = [pscustomobject]@{ sha256 = ('22' * 32); receiptSha256 = ('22' * 32) }
        }
      }
    }
  }
  $absentConfig = [ordered]@{
    exists = $false; bytes = $null; sha256 = $null; attributes = $null;
    creationTimeUtc = $null; lastWriteTimeUtc = $null
  }
  $fakeMcpReport = [pscustomobject][ordered]@{
    schemaVersion = 3
    documentType = 'codehangar/claude-live-mcp-acceptance/3'
    version = $script:ExpectedVersion
    status = 'PASS'
    completedAt = '2026-08-27T00:00:00.0000000Z'
    source = [ordered]@{ gitCommit = $fakeMcpIdentity.commit; gitTree = $fakeMcpIdentity.tree; sourceTreeDirty = $false }
    flow = [ordered]@{
      client = 'Claude Code authenticated live client'; data = 'isolated synthetic fixture';
      config = 'temporary command-line --strict-mcp-config; no registration or unregistration'; ownerAuthorized = $true
    }
    receipt = [ordered]@{
      path = 'C:\proof\code-hangar-signing-receipt.json'; schema = 'codehangar/signing-preparation/3';
      edition = 'Connector'; version = $script:ExpectedVersion; sha256 = ('11' * 32); unchanged = $true
    }
    server = [ordered]@{
      path = 'C:\evidence\input\code-hangar-mcp.exe'; bytes = 101; sha256 = ('22' * 32);
      receiptSha256 = ('22' * 32); unchanged = $true
    }
    client = [ordered]@{
      path = 'C:\Program Files\Claude\claude.exe'; version = '1.0.0'; bytes = 202;
      sha256 = ('33' * 32); unchanged = $true; exitCode = 0
    }
    syntheticFixture = [ordered]@{
      projectObserved = $true; requiredMethods = @('list_catalog', 'project_context');
      audit = 'fixture/mcp-audit-claude.json'; disconnect = 'fixture/mcp-disconnect.json'
    }
    liveConfig = [ordered]@{
      root = 'C:\Users\user'; leaf = '.claude.json'; before = $absentConfig; after = $absentConfig; unchanged = $true
    }
    failure = $null
    disconnectFailure = $null
  }
  $fakeMcpReport = ($fakeMcpReport | ConvertTo-Json -Depth 12) | ConvertFrom-Json -DateKind String
  Assert-ClaudeLiveMcpReport -Document $fakeMcpReport -Identity $fakeMcpIdentity -ReleaseBindings $fakeMcpBindings
  $badMcpReport = ($fakeMcpReport | ConvertTo-Json -Depth 12) | ConvertFrom-Json -DateKind String
  $badMcpReport.syntheticFixture.requiredMethods = @('list_catalog')
  $badMcpRejected = $false
  try { Assert-ClaudeLiveMcpReport -Document $badMcpReport -Identity $fakeMcpIdentity -ReleaseBindings $fakeMcpBindings } catch { $badMcpRejected = $true }
  if (-not $badMcpRejected) { throw 'Acceptance self-test accepted an MCP report missing a required exact method.' }

  $duplicateHashRejected = $false
  try {
    Assert-NoDuplicateGatePayloadHashes -PrivateGates @(
      [pscustomobject]@{ id = 'OWNER-03'; evidenceFiles = @([pscustomobject]@{ sha256 = ('ab' * 32) }) },
      [pscustomobject]@{ id = 'OWNER-04'; evidenceFiles = @([pscustomobject]@{ sha256 = ('ab' * 32) }) }
    )
  } catch { $duplicateHashRejected = $true }
  if (-not $duplicateHashRejected) { throw 'Acceptance self-test accepted copied identical payload bytes across gates.' }

  $tempParent = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $tempRoot = Join-Path $tempParent ('codehangar-acceptance-selftest-' + [guid]::NewGuid().ToString('N'))
  [void][System.IO.Directory]::CreateDirectory($tempRoot)
  try {
    $dummyAccepted = 0
    foreach ($gate in $specification) {
      $directory = Join-Path $tempRoot "evidence\$($gate.id)"
      [void][System.IO.Directory]::CreateDirectory($directory)
      $payloadPath = Join-Path $directory 'dummy.json'
      [System.IO.File]::WriteAllText($payloadPath, "{`"status`":`"PASS`",`"producer`":`"dummy`",`"gate`":`"$($gate.id)`"}", [System.Text.UTF8Encoding]::new($false))
      $payload = Open-LockedFile -Path $payloadPath -Label "Acceptance dummy $($gate.id)"
      try {
        $record = [pscustomobject]@{
          role = 'gate-result'
          documentType = 'codehangar/arbitrary-dummy/1'
          path = "evidence/$($gate.id)/dummy.json"
          bytes = $payload.Bytes
          sha256 = $payload.Sha256
        }
        $opened = Open-GateEvidenceFile -Root $tempRoot -GateId $gate.id -Record $record
        $opened.Stream.Dispose()
        $contract = Get-GateContract -Contracts $contracts -GateId $gate.id
        try {
          Assert-ExactStringSet -Actual @($record.documentType) -Expected @($contract.requiredDocumentTypes) -Label "Dummy $($gate.id) document types"
          $dummyAccepted += 1
        } catch { }
      } finally { $payload.Stream.Dispose() }
    }
    if ($dummyAccepted -ne 0) { throw "Acceptance self-test allowed $dummyAccepted of 50 arbitrary dummy payloads to satisfy a gate contract." }

    $srcRecordPath = Join-Path $tempRoot 'evidence\SRC-01\dummy.json'
    $srcPayload = Open-LockedFile -Path $srcRecordPath -Label 'Acceptance path-scope self-test'
    try {
      $record = [pscustomobject]@{
        role = 'gate-result'; documentType = 'codehangar/arbitrary-dummy/1'
        path = 'evidence/SRC-01/dummy.json'; bytes = $srcPayload.Bytes; sha256 = $srcPayload.Sha256
      }
      $reuseRejected = $false
      try { $bad = Open-GateEvidenceFile -Root $tempRoot -GateId 'AUTO-01' -Record $record; $bad.Stream.Dispose() } catch { $reuseRejected = $true }
      if (-not $reuseRejected) { throw 'Acceptance self-test allowed one gate payload path to satisfy another gate.' }
    } finally { $srcPayload.Stream.Dispose() }

    $fakeValidated = [pscustomobject]@{
      reportSha256 = ('12' * 32)
      report = [pscustomobject]@{
        source = [pscustomobject]@{ gitCommit = ('a' * 40); gitTree = ('b' * 40) }
        specification = [pscustomobject]@{ sha256 = ('34' * 32) }
        contracts = [pscustomobject]@{ sha256 = ('35' * 32) }
        sharedEvidence = @(
          [pscustomobject]@{ id = 'local-ci'; documentType = $script:LocalCiSchema; path = 'private/local-ci/path'; bytes = 1; sha256 = ('36' * 32) }
          [pscustomobject]@{ id = 'publication-audit'; documentType = $script:PublicationAuditSchema; path = 'private/publication/path'; bytes = 2; sha256 = ('37' * 32) }
        )
        releaseBindings = [pscustomobject]@{ signingDecision = [pscustomobject]@{ value = 'Signed' } }
        evidenceInventory = @([pscustomobject]@{ role = 'gate-evidence:SRC-01'; path = 'private/secret/path'; bytes = 1; sha256 = ('56' * 32) })
      }
      state = [pscustomobject]@{ publicGates = @([pscustomobject]@{ id = 'SRC-01'; status = 'PASS'; mode = 'automated'; producerId = 'release-pipeline'; producerSchema = $script:ReleaseProofSchema; proofSha256 = ('78' * 32); evidenceSetSha256 = ('90' * 32) }) }
    }
    $projection = Get-PublicProjection -Validated $fakeValidated
    $publicJson = $projection | ConvertTo-Json -Depth 12 -Compress
    foreach ($forbidden in @('private/secret/path', 'private/publication/path', 'claims', 'note', 'gitBranch', 'EvidenceDir')) {
      if ($publicJson -cmatch [regex]::Escape($forbidden)) { throw "Public projection leaked forbidden private marker: $forbidden" }
    }
  } finally {
    $resolved = [System.IO.Path]::GetFullPath($tempRoot)
    if ([System.IO.Path]::GetDirectoryName($resolved).Equals($tempParent, [System.StringComparison]::OrdinalIgnoreCase) -and
        [System.IO.Path]::GetFileName($resolved).StartsWith('codehangar-acceptance-selftest-', [System.StringComparison]::Ordinal)) {
      [System.IO.Directory]::Delete($resolved, $true)
    } else {
      throw "Refusing unsafe acceptance self-test cleanup: $resolved"
    }
  }
  Write-Host 'v0.1.3 canonical gate-contract, 50-dummy, duplicate-payload, required-test and public-projection self-test passed.' -ForegroundColor Green
  exit 0
}

$modeCount = @(@($Initialize, $Finalize, $ValidateOnly, $ExportPublicProjection) | Where-Object { [bool]$_ }).Count
if ($modeCount -ne 1) { throw 'Choose exactly one mode: -Initialize, -Finalize, -ValidateOnly or -ExportPublicProjection.' }
if ([string]::IsNullOrWhiteSpace($EvidenceDir)) { throw 'EvidenceDir is required.' }

if ($Initialize) {
  if (-not [string]::IsNullOrWhiteSpace($ReleaseArtifactProofDir) -or
      -not [string]::IsNullOrWhiteSpace($ExpectedReleaseArtifactProofSha256) -or
      -not [string]::IsNullOrWhiteSpace($ExpectedPrivateReportSha256) -or
      -not [string]::IsNullOrWhiteSpace($OutputPath)) {
    throw '-Initialize accepts only EvidenceDir.'
  }
  $root = Resolve-CandidateDirectory -Path $EvidenceDir
  Initialize-Acceptance -Root $root
  exit 0
}

foreach ($required in ([ordered]@{
    ReleaseArtifactProofDir = $ReleaseArtifactProofDir
    ExpectedReleaseArtifactProofSha256 = $ExpectedReleaseArtifactProofSha256
  }).GetEnumerator()) {
  if ([string]::IsNullOrWhiteSpace([string]$required.Value)) { throw "-$($required.Key) is required for this mode." }
}
$root = Resolve-CandidateDirectory -Path $EvidenceDir -RequireExisting
$proofDirectory = Resolve-ReleaseProofDirectory -Path $ReleaseArtifactProofDir

if ($Finalize) {
  if (-not [string]::IsNullOrWhiteSpace($ExpectedPrivateReportSha256) -or -not [string]::IsNullOrWhiteSpace($OutputPath)) {
    throw '-Finalize does not accept ExpectedPrivateReportSha256 or OutputPath.'
  }
  Finalize-Acceptance -Root $root -ProofDirectory $proofDirectory -ProofHash $ExpectedReleaseArtifactProofSha256
  exit 0
}
if ([string]::IsNullOrWhiteSpace($ExpectedPrivateReportSha256)) {
  throw '-ValidateOnly and -ExportPublicProjection require the independently recorded ExpectedPrivateReportSha256.'
}
$validated = Validate-Acceptance `
  -Root $root `
  -ProofDirectory $proofDirectory `
  -ProofHash $ExpectedReleaseArtifactProofSha256 `
  -ExpectedReportHash $ExpectedPrivateReportSha256
if ($ValidateOnly) {
  if (-not [string]::IsNullOrWhiteSpace($OutputPath)) { throw '-ValidateOnly does not accept OutputPath.' }
  Write-Host "Private acceptance evidence passed: $(Join-Path $root $script:PrivateReportName)" -ForegroundColor Green
  Write-Host "PRIVATE ACCEPTANCE SHA-256: $($validated.reportSha256)" -ForegroundColor Green
  exit 0
}
if ([string]::IsNullOrWhiteSpace($OutputPath)) { throw '-ExportPublicProjection requires OutputPath.' }
if (-not [System.IO.Path]::IsPathFullyQualified($OutputPath)) { throw 'OutputPath must be fully qualified.' }
$outputFull = Assert-LocalNonReparsePath -Path $OutputPath -Label 'Public acceptance projection output'
if (Test-Path -LiteralPath $outputFull) { throw 'Public acceptance projection refuses to overwrite an existing file.' }
$projection = Get-PublicProjection -Validated $validated
Write-NewJson -Path $outputFull -Value $projection -Depth 14
$projectionEvidence = Open-LockedFile -Path $outputFull -Label 'Public acceptance projection' -MaximumBytes 1048576
try {
  $publicJson = [System.Text.UTF8Encoding]::new($false, $true).GetString([System.IO.File]::ReadAllBytes($outputFull))
  foreach ($forbiddenProperty in @('"path"', '"note"', '"claims"', '"gitBranch"', '"root"', '"command"', '"output"')) {
    if ($publicJson -cmatch [regex]::Escape($forbiddenProperty)) {
      throw "Public acceptance projection contains forbidden private property $forbiddenProperty."
    }
  }
  Write-Host "Public acceptance projection: $outputFull" -ForegroundColor Green
  Write-Host "PUBLIC ACCEPTANCE SHA-256: $($projectionEvidence.Sha256)" -ForegroundColor Green
} finally {
  $projectionEvidence.Stream.Dispose()
}
