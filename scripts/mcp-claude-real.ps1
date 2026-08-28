[CmdletBinding()]
param(
  [string]$EvidenceDir,
  [string]$ServerPath,
  [string]$ExpectedSha256,
  [string]$SigningReceiptPath,
  [string]$ExpectedSigningReceiptSha256,
  [string]$ClaudeExecutablePath,
  [string]$ExpectedClaudeExecutableSha256,
  [string]$ClaudeConfigRoot,
  [switch]$OwnerAuthorized,
  [switch]$SelfTest
)

# Owner-supervised live-client acceptance for v0.1.3. The client may use its
# authenticated remote service, so this script is never an unattended CI gate.
# Code Hangar data stays synthetic. The exact client, Connector signing receipt,
# receipt-bound sidecar and live-config root are explicit, independently hashed
# inputs. No registration or live-config restoration is performed here.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$script:ExpectedVersion = "0.1.3"
$script:SigningReceiptSchema = "codehangar/signing-preparation/3"
$script:ReportSchema = "codehangar/claude-live-mcp-acceptance/3"
$script:SigningReceiptFileName = "code-hangar-signing-receipt.json"

$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$candidateAcceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance\v0.1.3\candidate"))

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

function Assert-NonReparseLocalPathChain {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequireExisting
  )
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) {
    throw "$Label must be an explicit fully qualified path."
  }
  $full = [System.IO.Path]::GetFullPath($Path)
  $root = [System.IO.Path]::GetPathRoot($full)
  if ([string]::IsNullOrWhiteSpace($root) -or $root.StartsWith("\\")) {
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

function Open-LockedInput {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequireExe
  )
  $full = Assert-NonReparseLocalPathChain -Path $Path -Label $Label -RequireExisting
  if (-not (Test-Path -LiteralPath $full -PathType Leaf)) {
    throw "$Label must identify an existing regular file: $full"
  }
  if ($RequireExe -and [System.IO.Path]::GetExtension($full) -ine '.exe') {
    throw "$Label must identify an exact executable file."
  }
  $stream = [System.IO.FileStream]::new(
    $full,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read,
    [System.IO.FileShare]::Read
  )
  if ($stream.Length -le 0) {
    $stream.Dispose()
    throw "$Label must be non-empty."
  }
  return [pscustomobject]@{
    Path = $full
    Stream = $stream
    Bytes = [long]$stream.Length
    Sha256 = Get-LockedStreamSha256 -Stream $stream
  }
}

function Copy-LockedInputToNewFile {
  param(
    [Parameter(Mandatory = $true)]$InputEvidence,
    [Parameter(Mandatory = $true)][string]$DestinationPath,
    [Parameter(Mandatory = $true)][string]$Label
  )
  $destination = Assert-NonReparseLocalPathChain -Path $DestinationPath -Label $Label
  if (Test-Path -LiteralPath $destination) { throw "$Label already exists: $destination" }
  $output = [System.IO.FileStream]::new(
    $destination,
    [System.IO.FileMode]::CreateNew,
    [System.IO.FileAccess]::ReadWrite,
    [System.IO.FileShare]::None
  )
  try {
    $InputEvidence.Stream.Position = 0
    $InputEvidence.Stream.CopyTo($output)
    $output.Flush($true)
    if ($output.Length -ne [long]$InputEvidence.Bytes) {
      throw "$Label length changed while it was snapshotted."
    }
    $copiedHash = Get-LockedStreamSha256 -Stream $output
    if ($copiedHash -cne [string]$InputEvidence.Sha256) {
      throw "$Label hash changed while it was snapshotted."
    }
  } finally {
    $output.Dispose()
  }
  return Open-LockedInput -Path $destination -Label $Label
}

function Read-LockedUtf8Json {
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [Parameter(Mandatory = $true)][string]$Label,
    [long]$MaximumBytes = 131072
  )
  if ($Evidence.Bytes -gt $MaximumBytes) { throw "$Label exceeds its bounded size." }
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

function Read-AndValidateConnectorReceipt {
  param(
    [Parameter(Mandatory = $true)]$ReceiptEvidence,
    [Parameter(Mandatory = $true)]$ServerEvidence,
    [Parameter(Mandatory = $true)]$Identity,
    [Parameter(Mandatory = $true)][string]$ExpectedReceiptHash
  )
  Assert-CanonicalSha256 -Value $ExpectedReceiptHash -Label "Expected signing-receipt SHA-256"
  if ([System.IO.Path]::GetFileName($ReceiptEvidence.Path) -cne $script:SigningReceiptFileName) {
    throw "SigningReceiptPath must identify $script:SigningReceiptFileName."
  }
  if ([string]$ReceiptEvidence.Sha256 -cne $ExpectedReceiptHash) {
    throw "The Connector signing receipt does not match the independently supplied expected hash."
  }
  $receipt = Read-LockedUtf8Json -Evidence $ReceiptEvidence -Label "Connector signing receipt"
  Assert-ExactPropertyNames -Object $receipt -Expected @(
    'schema', 'edition', 'version', 'target_triple', 'release_root_public_blob_hex',
    'cargo_lock_sha256', 'bundle_contract_sha256', 'source', 'prepared_at_utc',
    'frontend', 'artifacts'
  ) -Label "Connector signing receipt"
  if ([string]$receipt.schema -cne $script:SigningReceiptSchema -or
      [string]$receipt.edition -cne 'Connector' -or
      [string]$receipt.version -cne $script:ExpectedVersion -or
      [string]$receipt.target_triple -cne 'x86_64-pc-windows-msvc') {
    throw "The signing receipt is not the exact v0.1.3 Connector preparation receipt."
  }
  Assert-ExactPropertyNames -Object $receipt.source -Expected @(
    'git_commit', 'git_tree', 'source_tree_dirty'
  ) -Label "Connector signing-receipt source"
  if ([bool]$receipt.source.source_tree_dirty -or
      [string]$receipt.source.git_commit -cne [string]$Identity.commit -or
      [string]$receipt.source.git_tree -cne [string]$Identity.tree) {
    throw "The Connector signing receipt is not bound to this exact clean source commit/tree."
  }
  Assert-ExactPropertyNames -Object $receipt.artifacts.mcp -Expected @(
    'file_name', 'length', 'sha256'
  ) -Label "Connector receipt MCP artifact"
  if ([string]$receipt.artifacts.mcp.file_name -cne 'code-hangar-mcp.exe' -or
      [long]$receipt.artifacts.mcp.length -ne [long]$ServerEvidence.Bytes -or
      [string]$receipt.artifacts.mcp.sha256 -cne [string]$ServerEvidence.Sha256) {
    throw "The candidate MCP sidecar does not match the exact receipt-bound Connector artifact."
  }
  return $receipt
}

function Resolve-NewEvidenceDirectory {
  param([Parameter(Mandatory = $true)][string]$Path)
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) { $Path = Join-Path $repoRoot $Path }
  $full = Assert-NonReparseLocalPathChain -Path $Path -Label "Claude acceptance evidence"
  $prefix = $candidateAcceptanceRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "EvidenceDir must be a new child directory under $candidateAcceptanceRoot"
  }
  if (Test-Path -LiteralPath $full) {
    throw "Claude acceptance evidence is immutable per attempt; choose a new EvidenceDir."
  }
  return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
}

function Write-NewJsonFile {
  param([string]$Path, [object]$Value)
  $full = Assert-NonReparseLocalPathChain -Path $Path -Label "Claude acceptance report"
  $json = ($Value | ConvertTo-Json -Depth 14) + "`n"
  $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
  $stream = [System.IO.FileStream]::new($full, 'CreateNew', 'Write', 'None')
  try {
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
}

function Get-OptionalConfigState {
  param([Parameter(Mandatory = $true)][string]$Path)
  $full = Assert-NonReparseLocalPathChain -Path $Path -Label "Claude live config"
  if (-not (Test-Path -LiteralPath $full)) {
    return [pscustomobject]@{
      exists = $false; bytes = $null; sha256 = $null; attributes = $null;
      creationTimeUtc = $null; lastWriteTimeUtc = $null
    }
  }
  $evidence = Open-LockedInput -Path $full -Label "Claude live config"
  try {
    $item = Get-Item -LiteralPath $full -Force
    return [pscustomobject]@{
      exists = $true
      bytes = $evidence.Bytes
      sha256 = $evidence.Sha256
      attributes = [string]$item.Attributes
      creationTimeUtc = $item.CreationTimeUtc.ToString('o')
      lastWriteTimeUtc = $item.LastWriteTimeUtc.ToString('o')
    }
  } finally {
    $evidence.Stream.Dispose()
  }
}

function Test-ConfigStateEqual {
  param([object]$Before, [object]$After)
  $fields = @('exists', 'bytes', 'sha256', 'attributes', 'creationTimeUtc', 'lastWriteTimeUtc')
  Assert-ExactPropertyNames -Object $Before -Expected $fields -Label 'Claude config before-state'
  Assert-ExactPropertyNames -Object $After -Expected $fields -Label 'Claude config after-state'
  return ((@($fields | ForEach-Object { $Before.$_ }) | ConvertTo-Json -Compress) -ceq
    (@($fields | ForEach-Object { $After.$_ }) | ConvertTo-Json -Compress))
}

function Get-CleanGitIdentity {
  $commit = ([string](& git -C $repoRoot rev-parse HEAD)).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw "Unable to read the MCP acceptance source commit." }
  $tree = ([string](& git -C $repoRoot rev-parse 'HEAD^{tree}')).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw "Unable to read the MCP acceptance source tree." }
  $status = @(& git -C $repoRoot status --porcelain --untracked-files=all)
  if ($LASTEXITCODE -ne 0 -or $status.Count -ne 0) {
    throw "The owner-supervised MCP acceptance requires the exact clean release source tree."
  }
  if ($commit -cnotmatch '^[0-9a-f]{40,64}$' -or $tree -cnotmatch '^[0-9a-f]{40,64}$') {
    throw "The MCP acceptance source identity is malformed."
  }
  return [pscustomobject]@{ commit = $commit; tree = $tree }
}

function Invoke-ExactClaude {
  param([Parameter(Mandatory = $true)][string[]]$Arguments)
  $text = (& $script:ResolvedClaudeExecutable @Arguments 2>&1 | Out-String).Trim()
  return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Text = $text }
}

if ($SelfTest) {
  $unexpected = @(@(
    $EvidenceDir, $ServerPath, $ExpectedSha256, $SigningReceiptPath,
    $ExpectedSigningReceiptSha256, $ClaudeExecutablePath,
    $ExpectedClaudeExecutableSha256, $ClaudeConfigRoot
  ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  if ($OwnerAuthorized -or $unexpected.Count -ne 0) {
    throw "-SelfTest accepts no live-flow arguments and never starts Claude."
  }
  $tempParent = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $tempRoot = Join-Path $tempParent ("codehangar-mcp-receipt-selftest-" + [guid]::NewGuid().ToString('N'))
  [void][System.IO.Directory]::CreateDirectory($tempRoot)
  $serverEvidence = $null
  $receiptEvidence = $null
  $snapshotEvidence = $null
  try {
    $serverPathFixture = Join-Path $tempRoot 'code-hangar-mcp.exe'
    [System.IO.File]::WriteAllBytes($serverPathFixture, [System.Text.Encoding]::UTF8.GetBytes('synthetic-mcp-sidecar'))
    $serverEvidence = Open-LockedInput -Path $serverPathFixture -Label 'Self-test server'
    $identity = [pscustomobject]@{ commit = ('ab' * 20); tree = ('cd' * 20) }
    $receipt = [ordered]@{
      schema = $script:SigningReceiptSchema
      edition = 'Connector'
      version = $script:ExpectedVersion
      target_triple = 'x86_64-pc-windows-msvc'
      release_root_public_blob_hex = ('AA' * 411)
      cargo_lock_sha256 = ('12' * 32)
      bundle_contract_sha256 = ('34' * 32)
      source = [ordered]@{ git_commit = $identity.commit; git_tree = $identity.tree; source_tree_dirty = $false }
      prepared_at_utc = '2026-08-27T00:00:00.0000000Z'
      frontend = [ordered]@{}
      artifacts = [ordered]@{
        parent = [ordered]@{}
        helper = [ordered]@{}
        verifier = [ordered]@{}
        mcp = [ordered]@{ file_name = 'code-hangar-mcp.exe'; length = $serverEvidence.Bytes; sha256 = $serverEvidence.Sha256 }
      }
    }
    $receiptPath = Join-Path $tempRoot $script:SigningReceiptFileName
    Write-NewJsonFile -Path $receiptPath -Value $receipt
    $receiptEvidence = Open-LockedInput -Path $receiptPath -Label 'Self-test receipt'
    [void](Read-AndValidateConnectorReceipt -ReceiptEvidence $receiptEvidence -ServerEvidence $serverEvidence -Identity $identity -ExpectedReceiptHash $receiptEvidence.Sha256)

    $wrongReceiptRejected = $false
    try {
      [void](Read-AndValidateConnectorReceipt -ReceiptEvidence $receiptEvidence -ServerEvidence $serverEvidence -Identity $identity -ExpectedReceiptHash ('00' * 32))
    } catch { $wrongReceiptRejected = $true }
    if (-not $wrongReceiptRejected) { throw "MCP self-test accepted the wrong independent receipt hash." }

    $wrongSourceRejected = $false
    try {
      [void](Read-AndValidateConnectorReceipt -ReceiptEvidence $receiptEvidence -ServerEvidence $serverEvidence -Identity ([pscustomobject]@{ commit = ('ef' * 20); tree = $identity.tree }) -ExpectedReceiptHash $receiptEvidence.Sha256)
    } catch { $wrongSourceRejected = $true }
    if (-not $wrongSourceRejected) { throw "MCP self-test accepted a receipt from another source commit." }

    $writerRejected = $false
    try {
      $writer = [System.IO.FileStream]::new($serverPathFixture, 'Open', 'Write', 'Read')
      $writer.Dispose()
    } catch [System.IO.IOException] { $writerRejected = $true }
    if (-not $writerRejected) { throw "MCP self-test did not deny writes to a locked input." }

    $snapshotPath = Join-Path $tempRoot 'receipt-bound-snapshot.exe'
    $snapshotEvidence = Copy-LockedInputToNewFile -InputEvidence $serverEvidence -DestinationPath $snapshotPath -Label 'Self-test MCP snapshot'
    if ($snapshotEvidence.Sha256 -cne $serverEvidence.Sha256) {
      throw "MCP self-test snapshot changed receipt-bound bytes."
    }
    $same = [pscustomobject]@{
      exists = $true; bytes = 7; sha256 = ('12' * 32); attributes = 'Archive';
      creationTimeUtc = '2026-08-27T00:00:00.0000000Z'; lastWriteTimeUtc = '2026-08-27T00:00:00.0000000Z'
    }
    $changed = [pscustomobject]@{
      exists = $true; bytes = 8; sha256 = ('34' * 32); attributes = 'Archive';
      creationTimeUtc = '2026-08-27T00:00:00.0000000Z'; lastWriteTimeUtc = '2026-08-27T00:00:01.0000000Z'
    }
    $metadataChanged = [pscustomobject]@{
      exists = $true; bytes = 7; sha256 = ('12' * 32); attributes = 'Archive';
      creationTimeUtc = '2026-08-27T00:00:00.0000000Z'; lastWriteTimeUtc = '2026-08-27T00:00:01.0000000Z'
    }
    if (-not (Test-ConfigStateEqual -Before $same -After $same) -or
        (Test-ConfigStateEqual -Before $same -After $changed) -or
        (Test-ConfigStateEqual -Before $same -After $metadataChanged)) {
      throw "MCP self-test detected incorrect live-config comparison."
    }
  } finally {
    foreach ($evidence in @($snapshotEvidence, $receiptEvidence, $serverEvidence)) {
      if ($null -ne $evidence) { $evidence.Stream.Dispose() }
    }
    $resolved = [System.IO.Path]::GetFullPath($tempRoot)
    if ([System.IO.Path]::GetDirectoryName($resolved).Equals($tempParent, [System.StringComparison]::OrdinalIgnoreCase) -and
        [System.IO.Path]::GetFileName($resolved).StartsWith('codehangar-mcp-receipt-selftest-', [System.StringComparison]::Ordinal)) {
      [System.IO.Directory]::Delete($resolved, $true)
    } else {
      throw "Refusing unsafe MCP self-test cleanup: $resolved"
    }
  }
  Write-Host "v0.1.3 Claude live-client MCP receipt/snapshot self-test passed without starting Claude or touching live config." -ForegroundColor Green
  exit 0
}

if (-not $OwnerAuthorized) {
  throw "The real Claude client may use an authenticated remote service. Rerun only during the supervised owner gate with -OwnerAuthorized. For offline synthetic coverage use scripts/mcp-fixture-smoke.ps1."
}
foreach ($required in ([ordered]@{
    EvidenceDir = $EvidenceDir
    ServerPath = $ServerPath
    ExpectedSha256 = $ExpectedSha256
    SigningReceiptPath = $SigningReceiptPath
    ExpectedSigningReceiptSha256 = $ExpectedSigningReceiptSha256
    ClaudeExecutablePath = $ClaudeExecutablePath
    ExpectedClaudeExecutableSha256 = $ExpectedClaudeExecutableSha256
    ClaudeConfigRoot = $ClaudeConfigRoot
  }).GetEnumerator()) {
  if ([string]::IsNullOrWhiteSpace([string]$required.Value)) {
    throw "-$($required.Key) is required for the owner-supervised live-client flow."
  }
}
foreach ($hashInput in @(
    @{ Value = $ExpectedSha256; Label = 'Expected MCP sidecar SHA-256' },
    @{ Value = $ExpectedSigningReceiptSha256; Label = 'Expected signing-receipt SHA-256' },
    @{ Value = $ExpectedClaudeExecutableSha256; Label = 'Expected Claude executable SHA-256' }
  )) {
  Assert-CanonicalSha256 -Value $hashInput.Value -Label $hashInput.Label
}

$identity = Get-CleanGitIdentity
$evidenceRoot = Resolve-NewEvidenceDirectory -Path $EvidenceDir
$configRoot = Assert-NonReparseLocalPathChain -Path $ClaudeConfigRoot -Label "Claude config root" -RequireExisting
if (-not (Test-Path -LiteralPath $configRoot -PathType Container)) {
  throw "ClaudeConfigRoot must identify an existing real directory."
}
$liveConfigPath = Join-Path $configRoot '.claude.json'
[void](Assert-NonReparseLocalPathChain -Path $liveConfigPath -Label "Claude live config")

$serverEvidence = $null
$receiptEvidence = $null
$clientEvidence = $null
$snapshotEvidence = $null
$previousCargoOffline = $env:CARGO_NET_OFFLINE
try {
  $serverEvidence = Open-LockedInput -Path $ServerPath -Label "Receipt-bound MCP server" -RequireExe
  if ($serverEvidence.Sha256 -cne $ExpectedSha256 -or
      [System.IO.Path]::GetFileName($serverEvidence.Path) -cne 'code-hangar-mcp.exe') {
    throw "ServerPath does not match the exact expected code-hangar-mcp.exe candidate."
  }
  $receiptEvidence = Open-LockedInput -Path $SigningReceiptPath -Label "Connector signing receipt"
  $receipt = Read-AndValidateConnectorReceipt `
    -ReceiptEvidence $receiptEvidence `
    -ServerEvidence $serverEvidence `
    -Identity $identity `
    -ExpectedReceiptHash $ExpectedSigningReceiptSha256
  $clientEvidence = Open-LockedInput -Path $ClaudeExecutablePath -Label "Claude executable" -RequireExe
  if ($clientEvidence.Sha256 -cne $ExpectedClaudeExecutableSha256) {
    throw "ClaudeExecutablePath does not match the independently supplied client hash."
  }
  $script:ResolvedClaudeExecutable = $clientEvidence.Path

  [void][System.IO.Directory]::CreateDirectory($evidenceRoot)
  Assert-NonReparseLocalPathChain -Path $evidenceRoot -Label "Claude acceptance evidence" -RequireExisting | Out-Null
  $inputDir = Join-Path $evidenceRoot 'input'
  [void][System.IO.Directory]::CreateDirectory($inputDir)
  $serverSnapshotPath = Join-Path $inputDir 'code-hangar-mcp.exe'
  $snapshotEvidence = Copy-LockedInputToNewFile `
    -InputEvidence $serverEvidence `
    -DestinationPath $serverSnapshotPath `
    -Label "Receipt-bound MCP server snapshot"
  $server = $snapshotEvidence.Path

  $reportPath = Join-Path $evidenceRoot 'claude-live-mcp.private.json'
  $configBefore = Get-OptionalConfigState -Path $liveConfigPath
  $auth = Invoke-ExactClaude -Arguments @('auth', 'status')
  $authJson = try { $auth.Text | ConvertFrom-Json } catch { $null }
  if ($auth.ExitCode -ne 0 -or $null -eq $authJson -or -not [bool]$authJson.loggedIn) {
    $configAfter = Get-OptionalConfigState -Path $liveConfigPath
    $configUnchanged = Test-ConfigStateEqual -Before $configBefore -After $configAfter
    $blocked = [ordered]@{
      schemaVersion = 3
      documentType = $script:ReportSchema
      version = $script:ExpectedVersion
      status = if ($configUnchanged) { 'BLOCKED' } else { 'FAIL' }
      checkedAt = [datetime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffffffZ', [System.Globalization.CultureInfo]::InvariantCulture)
      source = [ordered]@{ gitCommit = $identity.commit; gitTree = $identity.tree; sourceTreeDirty = $false }
      receipt = [ordered]@{ path = $receiptEvidence.Path; schema = $receipt.schema; sha256 = $receiptEvidence.Sha256; edition = $receipt.edition }
      server = [ordered]@{ path = $snapshotEvidence.Path; bytes = $snapshotEvidence.Bytes; sha256 = $snapshotEvidence.Sha256; unchanged = $true }
      client = [ordered]@{ path = $clientEvidence.Path; bytes = $clientEvidence.Bytes; sha256 = $clientEvidence.Sha256; authExitCode = $auth.ExitCode }
      liveConfig = [ordered]@{ root = $configRoot; leaf = '.claude.json'; before = $configBefore; after = $configAfter; unchanged = $configUnchanged }
      reason = 'Claude Code is not logged in. Authenticate manually, then use a new EvidenceDir for the supervised retry.'
    }
    Write-NewJsonFile -Path $reportPath -Value $blocked
    if (-not $configUnchanged) { throw "Claude authentication preflight changed the explicit live config. Evidence: $reportPath" }
    Write-Host "Claude live-client MCP test is blocked until manual login. Evidence: $reportPath" -ForegroundColor Yellow
    exit 2
  }

  $fixtureRoot = Join-Path $evidenceRoot 'fixture'
  $fixturePath = Join-Path $fixtureRoot 'fixture.json'
  $strictConfigPath = Join-Path $evidenceRoot 'claude-strict-mcp.json'
  $prepared = $false
  $failure = $null
  $disconnectFailure = $null
  $clientExitCode = $null
  $projectObserved = $false
  $auditPath = $null
  $disconnectPath = $null
  $env:CARGO_NET_OFFLINE = 'true'
  try {
    cargo run --locked --offline -q -p code-hangar-mcp --example acceptance_fixture -- prepare $fixtureRoot $server
    if ($LASTEXITCODE -ne 0) { throw "MCP fixture preparation failed with exit code $LASTEXITCODE" }
    $prepared = $true
    $fixture = Get-Content -LiteralPath $fixturePath -Raw | ConvertFrom-Json
    $preparedConfigPath = [System.IO.Path]::GetFullPath([string]$fixture.clients.claude.configPath)
    $fixturePrefix = $fixtureRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $preparedConfigPath.StartsWith($fixturePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Prepared Claude config escaped the synthetic fixture root."
    }
    $claudeConfig = Get-Content -LiteralPath $preparedConfigPath -Raw | ConvertFrom-Json
    $serverSpec = $claudeConfig.mcpServers.'code-hangar'
    if ($null -eq $serverSpec -or
        -not [System.IO.Path]::GetFullPath([string]$serverSpec.command).Equals($server, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Prepared synthetic Claude config does not bind the exact snapshotted MCP server."
    }
    Write-NewJsonFile -Path $strictConfigPath -Value ([ordered]@{
      mcpServers = [ordered]@{ 'code-hangar' = $serverSpec }
    })

    $prompt = "Use only the Code Hangar MCP server. Call list_catalog, find the project named exactly 'Fixture Git-like Project', then call get_project_context with its numeric projectId. Reply with the exact project name and its context file names."
    $claudeArgs = @(
      '-p', $prompt,
      '--output-format', 'json',
      '--permission-mode', 'dontAsk',
      '--strict-mcp-config',
      '--mcp-config', $strictConfigPath,
      '--allowedTools', 'mcp__code-hangar__list_catalog,mcp__code-hangar__get_project_context',
      '--setting-sources', 'project'
    )
    Push-Location $fixtureRoot
    try { $clientRun = Invoke-ExactClaude -Arguments $claudeArgs } finally { Pop-Location }
    $clientExitCode = $clientRun.ExitCode
    if ($clientExitCode -ne 0) { throw "Claude live-client invocation failed with exit code $clientExitCode." }
    $clientJson = $clientRun.Text | ConvertFrom-Json
    if ([bool]$clientJson.is_error) { throw "Claude returned an error result." }
    $projectObserved = [string]$clientJson.result -match 'Fixture Git-like Project'
    if (-not $projectObserved) { throw "Claude result did not identify the synthetic fixture project." }

    cargo run --locked --offline -q -p code-hangar-mcp --example acceptance_fixture -- audit-host $fixturePath claude list_catalog project_context
    if ($LASTEXITCODE -ne 0) { throw "Claude MCP activity audit failed with exit code $LASTEXITCODE" }
    $auditPath = Join-Path $fixtureRoot 'mcp-audit-claude.json'
    $audit = Get-Content -LiteralPath $auditPath -Raw | ConvertFrom-Json
    if ([string]$audit.status -cne 'PASS') { throw "Claude MCP activity audit did not pass." }
  } catch {
    $failure = $_.Exception.Message
  } finally {
    if ($prepared -and (Test-Path -LiteralPath $fixturePath -PathType Leaf)) {
      try {
        cargo run --locked --offline -q -p code-hangar-mcp --example acceptance_fixture -- disconnect $fixturePath
        if ($LASTEXITCODE -ne 0) { throw "MCP fixture disconnect failed with exit code $LASTEXITCODE" }
        $disconnectPath = Join-Path $fixtureRoot 'mcp-disconnect.json'
      } catch {
        $disconnectFailure = $_.Exception.Message
      }
    }
  }

  $configAfter = Get-OptionalConfigState -Path $liveConfigPath
  $configUnchanged = Test-ConfigStateEqual -Before $configBefore -After $configAfter
  if (-not $configUnchanged -and $null -eq $failure) {
    $failure = 'The explicit Claude live config changed during the temporary strict-config test.'
  }
  $serverUnchanged = (Get-LockedStreamSha256 -Stream $snapshotEvidence.Stream) -ceq $ExpectedSha256
  $receiptUnchanged = (Get-LockedStreamSha256 -Stream $receiptEvidence.Stream) -ceq $ExpectedSigningReceiptSha256
  $clientUnchanged = (Get-LockedStreamSha256 -Stream $clientEvidence.Stream) -ceq $ExpectedClaudeExecutableSha256
  if ((-not $serverUnchanged -or -not $receiptUnchanged -or -not $clientUnchanged) -and $null -eq $failure) {
    $failure = 'A locked client, receipt or sidecar input changed during the live-client test.'
  }
  $clientVersion = try { (Invoke-ExactClaude -Arguments @('--version')).Text } catch { $null }
  if ([string]::IsNullOrWhiteSpace([string]$clientVersion) -and $null -eq $failure) {
    $failure = 'The exact Claude client version could not be recorded.'
  }
  $report = [ordered]@{
    schemaVersion = 3
    documentType = $script:ReportSchema
    version = $script:ExpectedVersion
    status = if ($null -eq $failure -and $null -eq $disconnectFailure) { 'PASS' } else { 'FAIL' }
    completedAt = [datetime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffffffZ', [System.Globalization.CultureInfo]::InvariantCulture)
    source = [ordered]@{ gitCommit = $identity.commit; gitTree = $identity.tree; sourceTreeDirty = $false }
    flow = [ordered]@{
      client = 'Claude Code authenticated live client'
      data = 'isolated synthetic fixture'
      config = 'temporary command-line --strict-mcp-config; no registration or unregistration'
      ownerAuthorized = $true
    }
    receipt = [ordered]@{ path = $receiptEvidence.Path; schema = $receipt.schema; edition = $receipt.edition; version = $receipt.version; sha256 = $receiptEvidence.Sha256; unchanged = $receiptUnchanged }
    server = [ordered]@{ path = $snapshotEvidence.Path; bytes = $snapshotEvidence.Bytes; sha256 = $snapshotEvidence.Sha256; receiptSha256 = [string]$receipt.artifacts.mcp.sha256; unchanged = $serverUnchanged }
    client = [ordered]@{ path = $clientEvidence.Path; version = $clientVersion; bytes = $clientEvidence.Bytes; sha256 = $clientEvidence.Sha256; unchanged = $clientUnchanged; exitCode = $clientExitCode }
    syntheticFixture = [ordered]@{
      projectObserved = $projectObserved
      requiredMethods = @('list_catalog', 'project_context')
      audit = if ($null -ne $auditPath) { [System.IO.Path]::GetRelativePath($evidenceRoot, $auditPath).Replace('\', '/') } else { $null }
      disconnect = if ($null -ne $disconnectPath) { [System.IO.Path]::GetRelativePath($evidenceRoot, $disconnectPath).Replace('\', '/') } else { $null }
    }
    liveConfig = [ordered]@{ root = $configRoot; leaf = '.claude.json'; before = $configBefore; after = $configAfter; unchanged = $configUnchanged }
    failure = $failure
    disconnectFailure = $disconnectFailure
  }
  Write-NewJsonFile -Path $reportPath -Value $report
  if ($null -ne $disconnectFailure) { throw "Claude MCP synthetic-fixture cleanup failed: $disconnectFailure. Evidence: $reportPath" }
  if ($null -ne $failure) { throw "Claude owner-supervised live-client MCP test failed: $failure. Evidence: $reportPath" }
  Write-Host "Claude owner-supervised live-client MCP lifecycle passed on synthetic fixture data. Evidence: $reportPath" -ForegroundColor Green
} finally {
  $env:CARGO_NET_OFFLINE = $previousCargoOffline
  foreach ($evidence in @($snapshotEvidence, $clientEvidence, $receiptEvidence, $serverEvidence)) {
    if ($null -ne $evidence) { $evidence.Stream.Dispose() }
  }
}
