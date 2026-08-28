[CmdletBinding()]
param(
  [switch]$Create,
  [switch]$ValidateOnly,
  [switch]$SelfTest,
  [string]$EvidenceDir,
  [ValidateSet('Signed', 'Unsigned')][string]$SigningDecision,
  [switch]$OwnerAuthorized,
  [switch]$OwnerAcceptUnsignedOuter,
  [string]$ExpectedSignerSubject,
  [string]$ExpectedSignerThumbprint,
  [string]$ReleaseRootPublicBlobHex,
  [string]$LocalSigningReceiptPath,
  [string]$ExpectedLocalSigningReceiptSha256,
  [string]$ConnectorSigningReceiptPath,
  [string]$ExpectedConnectorSigningReceiptSha256,
  [string]$LocalReleaseIdentityPath,
  [string]$ExpectedLocalReleaseIdentitySha256,
  [string]$ConnectorReleaseIdentityPath,
  [string]$ExpectedConnectorReleaseIdentitySha256,
  [string]$LocalSetupPath,
  [string]$LocalParentPath,
  [string]$LocalHelperPath,
  [string]$LocalUninstallerPath,
  [string]$ConnectorSetupPath,
  [string]$ConnectorParentPath,
  [string]$ConnectorHelperPath,
  [string]$ConnectorUninstallerPath,
  [string]$ConnectorMcpPath,
  [string]$LifecycleManifestPath,
  [string]$ExpectedLifecycleManifestSha256,
  [string]$ExpectedReportSha256
)

# Creates or revalidates the private, owner-supervised release artifact proof.
# Every input is copied from a write/delete-denying handle into a new local,
# non-reparse evidence directory. Hash, Authenticode, RFC3161 timestamp, receipt
# and release-identity checks then operate on those same locked snapshot bytes.
# Nothing is signed, uploaded or sent over the network by this script.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot 'packaging-common.ps1')

$script:ExpectedVersion = '0.1.3'
$script:ProofSchema = 'codehangar/release-artifact-proof/1'
$script:ProofFileName = 'RELEASE-ARTIFACT-PROOF.private.json'
$script:ReceiptSchema = 'codehangar/signing-preparation/3'
$script:ReleaseIdentitySchema = 'codehangar/release-identity/1'
$script:LifecycleSchema = 'codehangar/sandbox-lifecycle/3'
$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$proofRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local\acceptance\v0.1.3\release-proof'))

function Assert-CanonicalSha256 {
  param([object]$Value, [string]$Label)
  if ([string]$Value -cnotmatch '^[0-9a-f]{64}$') {
    throw "$Label must be exactly 64 lowercase hexadecimal characters."
  }
}

function Assert-CanonicalThumbprint {
  param([object]$Value, [string]$Label)
  if ([string]$Value -cnotmatch '^[0-9A-F]{40,128}$' -or ([string]$Value).Length % 2 -ne 0) {
    throw "$Label must be canonical uppercase hexadecimal."
  }
}

function Assert-ExactPropertyNames {
  param([object]$Object, [string[]]$Expected, [string]$Label)
  if ($null -eq $Object) { throw "$Label is missing." }
  $actual = @($Object.PSObject.Properties.Name | Sort-Object)
  $wanted = @($Expected | Sort-Object)
  if (($actual -join "`n") -cne ($wanted -join "`n")) {
    throw "$Label has unexpected or missing fields (found: $($actual -join ', '))."
  }
}

function Assert-ExactStringInventory {
  param([string[]]$Actual, [string[]]$Expected, [string]$Label)
  $actualSorted = @($Actual | Sort-Object -Unique)
  $expectedSorted = @($Expected | Sort-Object -Unique)
  if ($Actual.Count -ne $Expected.Count -or
      ($actualSorted -join "`n") -cne ($expectedSorted -join "`n")) {
    throw "$Label differs. Expected [$($expectedSorted -join ', ')], found [$($actualSorted -join ', ')]."
  }
}

function ConvertFrom-CanonicalTimestamp {
  param([object]$Value, [string]$Label)
  $parsed = [datetimeoffset]::MinValue
  if ([string]::IsNullOrWhiteSpace([string]$Value) -or
      -not [datetimeoffset]::TryParseExact(
        [string]$Value,
        'o',
        [System.Globalization.CultureInfo]::InvariantCulture,
        [System.Globalization.DateTimeStyles]::None,
        [ref]$parsed
      )) {
    throw "$Label must be a canonical round-trip timestamp."
  }
  return $parsed
}

function Get-ExpectedLifecycleResultIds {
  return @(
    '01-clean-install-local-013',
    '02-uninstall-provisioning-local',
    '03-install-baseline-local-011',
    '04-launch-baseline-local-011',
    '05-close-baseline-local-011',
    '06-register-baseline-catalog',
    '08-check-baseline-catalog',
    '09-upgrade-local-013',
    '10-check-upgraded-catalog-013',
    '11-install-connector-013',
    '12-launch-connector-013',
    '13-close-connector-013',
    '14-check-connector-catalog',
    '15-repair-connector-013',
    '16-uninstall-local',
    '17-launch-connector-after-local-uninstall',
    '18-close-connector-after-local-uninstall',
    '19-check-connector-only-catalog',
    '20-uninstall-connector',
    '21-reinstall-local-013',
    '22-launch-reinstalled-local',
    '23-close-reinstalled-local',
    '24-check-reinstalled-local-catalog',
    '25-final-uninstall-local',
    '26-final-inspect'
  )
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

function Get-BytesSha256 {
  param([Parameter(Mandatory = $true)][byte[]]$Bytes)
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    return ([System.BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant()
  } finally {
    $sha.Dispose()
  }
}

function Open-LockedInput {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label
  )
  $full = Assert-LocalNonReparsePath -Path $Path -Label $Label -RequireExisting
  if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "$Label is not a regular file." }
  $stream = [System.IO.FileStream]::new($full, 'Open', 'Read', 'Read')
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

function Copy-LockedInput {
  param(
    [Parameter(Mandatory = $true)]$InputEvidence,
    [Parameter(Mandatory = $true)][string]$DestinationPath,
    [Parameter(Mandatory = $true)][string]$Label
  )
  $destination = Assert-LocalNonReparsePath -Path $DestinationPath -Label $Label
  if (Test-Path -LiteralPath $destination) { throw "$Label destination already exists." }
  $output = [System.IO.FileStream]::new($destination, 'CreateNew', 'ReadWrite', 'None')
  try {
    $InputEvidence.Stream.Position = 0
    $InputEvidence.Stream.CopyTo($output)
    $output.Flush($true)
    $hash = Get-LockedStreamSha256 -Stream $output
    if ($output.Length -ne [long]$InputEvidence.Bytes -or $hash -cne [string]$InputEvidence.Sha256) {
      throw "$Label snapshot does not match the locked input bytes."
    }
  } finally {
    $output.Dispose()
  }
  return Open-LockedInput -Path $destination -Label $Label
}

function Read-LockedJson {
  param([Parameter(Mandatory = $true)]$Evidence, [string]$Label, [long]$MaximumBytes = 262144)
  if ($Evidence.Bytes -gt $MaximumBytes) { throw "$Label exceeds its size bound." }
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

function Write-NewJson {
  param([string]$Path, [object]$Value)
  $full = Assert-LocalNonReparsePath -Path $Path -Label 'Release artifact proof output'
  $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes((($Value | ConvertTo-Json -Depth 18) + "`n"))
  $stream = [System.IO.FileStream]::new($full, 'CreateNew', 'Write', 'None')
  try {
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
}

function Get-CleanGitIdentity {
  $identity = Get-CodeHangarCleanGitIdentity -RepoRoot $repoRoot
  return [pscustomobject]@{ commit = $identity.Commit; tree = $identity.Tree }
}

function Resolve-NewProofDirectory {
  param([string]$Path)
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) { $Path = Join-Path $repoRoot $Path }
  $full = Assert-LocalNonReparsePath -Path $Path -Label 'Release proof evidence directory'
  $prefix = $proofRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "EvidenceDir must be a new child under $proofRoot"
  }
  if (Test-Path -LiteralPath $full) { throw 'Release proof evidence is immutable per attempt; choose a new EvidenceDir.' }
  return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
}

function Resolve-ExistingProofDirectory {
  param([string]$Path)
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) { $Path = Join-Path $repoRoot $Path }
  $full = Assert-LocalNonReparsePath -Path $Path -Label 'Release proof evidence directory' -RequireExisting
  $prefix = $proofRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not (Test-Path -LiteralPath $full -PathType Container)) {
    throw "EvidenceDir must be an existing child under $proofRoot"
  }
  return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
}

function Get-Rfc3161Info {
  param(
    [Parameter(Mandatory = $true)][byte[]]$Content,
    [Parameter(Mandatory = $true)][byte[]]$PrimarySignature,
    [string]$Label = 'RFC3161 timestamp'
  )
  if ($Content.Length -eq 0 -or $PrimarySignature.Length -eq 0) {
    throw "$Label content and primary signature must be non-empty."
  }
  $reader = [System.Formats.Asn1.AsnReader]::new($Content, [System.Formats.Asn1.AsnEncodingRules]::DER)
  $sequence = $reader.ReadSequence()
  [void]$sequence.ReadInteger()
  [void]$sequence.ReadObjectIdentifier()
  $messageImprint = $sequence.ReadSequence()
  $algorithmIdentifier = $messageImprint.ReadSequence()
  $algorithmOid = $algorithmIdentifier.ReadObjectIdentifier()
  if ($algorithmIdentifier.HasData) { $algorithmIdentifier.ReadNull() }
  if ($algorithmIdentifier.HasData) { throw "$Label messageImprint algorithm has trailing fields." }
  $imprint = [byte[]]$messageImprint.ReadOctetString()
  if ($messageImprint.HasData) { throw "$Label messageImprint has trailing fields." }
  [void]$sequence.ReadInteger()
  $signedAt = $sequence.ReadGeneralizedTime().UtcDateTime
  while ($sequence.HasData) { [void]$sequence.ReadEncodedValue() }
  if ($reader.HasData) { throw "$Label TSTInfo has trailing data." }

  $algorithm = switch ($algorithmOid) {
    '1.3.14.3.2.26' { [System.Security.Cryptography.HashAlgorithmName]::SHA1; break }
    '2.16.840.1.101.3.4.2.1' { [System.Security.Cryptography.HashAlgorithmName]::SHA256; break }
    '2.16.840.1.101.3.4.2.2' { [System.Security.Cryptography.HashAlgorithmName]::SHA384; break }
    '2.16.840.1.101.3.4.2.3' { [System.Security.Cryptography.HashAlgorithmName]::SHA512; break }
    default { throw "$Label uses unsupported messageImprint digest OID $algorithmOid." }
  }
  $computed = [System.Security.Cryptography.HashAlgorithm]::Create($algorithm.Name)
  if ($null -eq $computed) { throw "$Label digest $($algorithm.Name) is unavailable." }
  try {
    $expectedImprint = [byte[]]$computed.ComputeHash($PrimarySignature)
  } finally {
    $computed.Dispose()
  }
  if ($imprint.Length -ne $expectedImprint.Length -or
      -not [System.Security.Cryptography.CryptographicOperations]::FixedTimeEquals($imprint, $expectedImprint)) {
    throw "$Label messageImprint does not bind the exact primary Authenticode signature."
  }
  return [pscustomobject]@{
    SignedAtUtc = $signedAt
    MessageImprintAlgorithm = $algorithm.Name
    MessageImprintAlgorithmOid = $algorithmOid
    MessageImprintHex = [System.Convert]::ToHexString($imprint).ToLowerInvariant()
  }
}

function Get-AuthenticodeProof {
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [Parameter(Mandatory = $true)][ValidateSet('Valid', 'NotSigned')][string]$ExpectedStatus,
    [string]$SignerSubject,
    [string]$SignerThumbprint,
    [Parameter(Mandatory = $true)][string]$Label
  )
  $before = Get-LockedStreamSha256 -Stream $Evidence.Stream
  $signature = Get-AuthenticodeSignature -LiteralPath $Evidence.Path -ErrorAction Stop
  if ([string]$signature.Status -cne $ExpectedStatus) {
    throw "$Label Authenticode status is $($signature.Status), expected $ExpectedStatus."
  }
  if ($ExpectedStatus -eq 'NotSigned') {
    if ($null -ne $signature.SignerCertificate -or $null -ne $signature.TimeStamperCertificate) {
      throw "$Label is not honestly unsigned."
    }
    if ((Get-LockedStreamSha256 -Stream $Evidence.Stream) -cne $before) {
      throw "$Label changed while its unsigned state was checked."
    }
    return [ordered]@{ status = 'NotSigned'; signer = $null; timestamp = $null }
  }

  Assert-CanonicalThumbprint -Value $SignerThumbprint -Label 'Expected signer thumbprint'
  if ([string]::IsNullOrWhiteSpace($SignerSubject)) { throw 'Expected signer subject is required.' }
  if ($null -eq $signature.SignerCertificate -or $null -eq $signature.TimeStamperCertificate -or
      [string]$signature.SignerCertificate.Subject -cne $SignerSubject -or
      [string]$signature.SignerCertificate.Thumbprint -cne $SignerThumbprint) {
    throw "$Label signer subject/thumbprint or timestamp certificate does not match the owner decision."
  }

  Add-Type -AssemblyName System.Security.Cryptography.Pkcs
  $pe = Get-EmbeddedAuthenticodeCms -Stream $Evidence.Stream
  $cms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
  $cms.Decode($pe.CmsBytes)
  $cms.CheckSignature($true)
  if ($cms.SignerInfos.Count -ne 1) { throw "$Label must contain exactly one primary Authenticode signer." }
  $cmsSigner = $cms.SignerInfos[0].Certificate
  if ($null -eq $cmsSigner -or $cmsSigner.Subject -cne $SignerSubject -or
      $cmsSigner.Thumbprint -cne $SignerThumbprint) {
    throw "$Label embedded Authenticode signer does not match the expected signer."
  }
  Assert-OfflineCertificateChain -Certificate $cmsSigner -ExtraCertificates $cms.Certificates -Label "$Label signer"
  $timestampAttributes = @($cms.SignerInfos[0].UnsignedAttributes | Where-Object {
      $_.Oid.Value -eq '1.3.6.1.4.1.311.3.3.1'
    })
  if ($timestampAttributes.Count -ne 1 -or $timestampAttributes[0].Values.Count -ne 1) {
    throw "$Label must contain exactly one RFC3161 Authenticode timestamp."
  }
  $timestampBytes = [byte[]]$timestampAttributes[0].Values[0].RawData
  $timestampCms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
  $timestampCms.Decode($timestampBytes)
  $timestampCms.CheckSignature($true)
  if ($timestampCms.SignerInfos.Count -ne 1) { throw "$Label RFC3161 token must contain exactly one signer." }
  $timestampSigner = $timestampCms.SignerInfos[0].Certificate
  if ($null -eq $timestampSigner -or
      $timestampSigner.Thumbprint -cne $signature.TimeStamperCertificate.Thumbprint) {
    throw "$Label RFC3161 signer does not match WinTrust evidence."
  }
  Assert-OfflineCertificateChain -Certificate $timestampSigner -ExtraCertificates $timestampCms.Certificates -Label "$Label timestamp signer"
  $timestampInfo = Get-Rfc3161Info `
    -Content $timestampCms.ContentInfo.Content `
    -PrimarySignature ([byte[]]$cms.SignerInfos[0].GetSignature()) `
    -Label "$Label RFC3161 timestamp"
  $signedAt = $timestampInfo.SignedAtUtc
  if ($signedAt -lt $cmsSigner.NotBefore.ToUniversalTime() -or
      $signedAt -gt $cmsSigner.NotAfter.ToUniversalTime() -or
      $signedAt -lt $timestampSigner.NotBefore.ToUniversalTime() -or
      $signedAt -gt $timestampSigner.NotAfter.ToUniversalTime()) {
    throw "$Label RFC3161 time is outside the signer or timestamp-certificate validity window."
  }
  if ($null -eq ('CodeHangar.Packaging.OfflineAuthenticode' -as [type])) {
    Add-Type -Path (Join-Path $PSScriptRoot 'WebView2Authenticode.cs')
  }
  [CodeHangar.Packaging.OfflineAuthenticode]::VerifyFile($Evidence.Path)
  if ((Get-LockedStreamSha256 -Stream $Evidence.Stream) -cne $before) {
    throw "$Label changed while its Authenticode proof was computed."
  }
  return [ordered]@{
    status = 'Valid'
    signer = [ordered]@{
      subject = $cmsSigner.Subject
      thumbprint = $cmsSigner.Thumbprint
      notBeforeUtc = $cmsSigner.NotBefore.ToUniversalTime().ToString('o')
      notAfterUtc = $cmsSigner.NotAfter.ToUniversalTime().ToString('o')
    }
    timestamp = [ordered]@{
      rfc3161 = $true
      signedAtUtc = $signedAt.ToString('o')
      signerSubject = $timestampSigner.Subject
      signerThumbprint = $timestampSigner.Thumbprint
      tokenSha256 = Get-BytesSha256 -Bytes $timestampBytes
      messageImprintAlgorithm = $timestampInfo.MessageImprintAlgorithm
      messageImprintAlgorithmOid = $timestampInfo.MessageImprintAlgorithmOid
      messageImprint = $timestampInfo.MessageImprintHex
    }
  }
}

function Read-AndValidateReceipt {
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [ValidateSet('Local', 'Connector')][string]$Edition,
    [string]$ExpectedHash,
    [object]$Identity,
    [string]$PublicBlobHex
  )
  Assert-CanonicalSha256 -Value $ExpectedHash -Label "$Edition signing receipt expected hash"
  if ($Evidence.Sha256 -cne $ExpectedHash) { throw "$Edition signing receipt hash mismatch." }
  if ([System.IO.Path]::GetFileName($Evidence.Path) -cne 'code-hangar-signing-receipt.json') {
    throw "$Edition signing receipt snapshot lost its canonical filename."
  }
  $publicBlob = Assert-ReleaseRootPublicBlobHex -Value $PublicBlobHex
  $cargoLock = Get-StableReleaseArtifactEvidence -Path (Join-Path $repoRoot 'Cargo.lock') -Label 'Current Cargo.lock'
  $target = Get-CodeHangarHostTargetTriple
  $bundleContract = Get-CodeHangarBundleContractSha256 -RepoRoot $repoRoot -Edition $Edition
  $validated = Read-AndValidateCodeHangarSigningReceipt `
    -SigningDirectory (Split-Path -Parent $Evidence.Path) `
    -Edition $Edition `
    -ExpectedVersion $script:ExpectedVersion `
    -ExpectedTargetTriple $target `
    -ExpectedPublicBlobHex $publicBlob `
    -ExpectedCargoLockSha256 $cargoLock.Sha256 `
    -ExpectedBundleContractSha256 $bundleContract `
    -ExpectedSourceCommit $Identity.commit `
    -ExpectedSourceTree $Identity.tree `
    -ExpectedReceiptSha256 $ExpectedHash
  if ($validated.Evidence.Sha256 -cne $Evidence.Sha256 -or $validated.Evidence.Length -ne $Evidence.Bytes) {
    throw "$Edition signing receipt validator did not consume the locked snapshot bytes."
  }
  return [pscustomobject]@{
    receipt = $validated.Receipt
    frontend = $validated.Frontend
    verifier = $validated.Verifier
    mcp = $validated.Mcp
    cargoLockSha256 = $cargoLock.Sha256
    bundleContractSha256 = $bundleContract
    targetTriple = $target
  }
}

function Convert-PublicBlobToRsa {
  param([string]$PublicBlobHex)
  $normalized = Assert-ReleaseRootPublicBlobHex -Value $PublicBlobHex
  $bytes = [System.Convert]::FromHexString($normalized)
  $exponentLength = [System.BitConverter]::ToUInt32($bytes, 8)
  $modulusLength = [System.BitConverter]::ToUInt32($bytes, 12)
  $exponent = [byte[]]::new($exponentLength)
  $modulus = [byte[]]::new($modulusLength)
  [System.Array]::Copy($bytes, 24, $exponent, 0, $exponent.Length)
  [System.Array]::Copy($bytes, 24 + $exponent.Length, $modulus, 0, $modulus.Length)
  $rsa = [System.Security.Cryptography.RSA]::Create()
  $rsa.ImportParameters([System.Security.Cryptography.RSAParameters]@{
      Exponent = $exponent
      Modulus = $modulus
    })
  return $rsa
}

function Read-AndValidateReleaseIdentity {
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [ValidateSet('Local', 'Connector')][string]$Edition,
    [string]$ExpectedHash,
    [string]$ReceiptHash,
    [object]$Parent,
    [object]$Helper,
    [string]$PublicBlobHex
  )
  Assert-CanonicalSha256 -Value $ExpectedHash -Label "$Edition release identity expected hash"
  if ($Evidence.Sha256 -cne $ExpectedHash) { throw "$Edition release identity hash mismatch." }
  $manifest = Read-LockedJson -Evidence $Evidence -Label "$Edition release identity"
  Assert-ExactPropertyNames -Object $manifest -Expected @(
    'schema', 'release_id', 'parent', 'helper', 'signature_rsa_pss_sha256'
  ) -Label "$Edition release identity"
  if ([string]$manifest.schema -cne $script:ReleaseIdentitySchema) { throw "$Edition release identity schema mismatch." }
  $expectedReleaseId = Get-CodeHangarReceiptBoundReleaseId -Edition $Edition -SigningReceiptSha256 $ReceiptHash
  if ([string]$manifest.release_id -cne $expectedReleaseId -or
      [string]$manifest.parent.file_name -cne 'code-hangar-desktop.exe' -or
      [string]$manifest.helper.file_name -cne 'code-hangar-elevated.exe' -or
      [string]$manifest.parent.sha256 -cne $Parent.Sha256 -or
      [string]$manifest.helper.sha256 -cne $Helper.Sha256) {
    throw "$Edition release identity does not bind its receipt and exact parent/helper bytes."
  }
  if ([string]$manifest.signature_rsa_pss_sha256 -cnotmatch '^[0-9a-f]+$' -or
      ([string]$manifest.signature_rsa_pss_sha256).Length % 2 -ne 0) {
    throw "$Edition release identity signature is malformed."
  }
  $payload = [System.Text.Encoding]::UTF8.GetBytes(
    "schema=$script:ReleaseIdentitySchema`nrelease_id=$expectedReleaseId`nparent_file=code-hangar-desktop.exe`nparent_sha256=$($Parent.Sha256)`nhelper_file=code-hangar-elevated.exe`nhelper_sha256=$($Helper.Sha256)`n"
  )
  $signature = [System.Convert]::FromHexString([string]$manifest.signature_rsa_pss_sha256)
  $rsa = Convert-PublicBlobToRsa -PublicBlobHex $PublicBlobHex
  try {
    if (-not $rsa.VerifyData(
        $payload,
        $signature,
        [System.Security.Cryptography.HashAlgorithmName]::SHA256,
        [System.Security.Cryptography.RSASignaturePadding]::Pss
      )) {
      throw "$Edition release identity RSA-PSS signature is invalid."
    }
  } finally {
    $rsa.Dispose()
  }
  return [pscustomobject]@{ Manifest = $manifest; ReleaseId = $expectedReleaseId }
}

function Assert-PostSigningReceiptLink {
  param([object]$Evidence, [object]$ReceiptArtifact, [string]$ExpectedFileName, [string]$Label)
  Assert-ExactPropertyNames -Object $ReceiptArtifact -Expected @(
    'file_name', 'length', 'sha256', 'authenticode_image_sha256'
  ) -Label "$Label receipt artifact"
  if ([string]$ReceiptArtifact.file_name -cne $ExpectedFileName -or
      [long]$Evidence.Bytes -le [long]$ReceiptArtifact.length -or
      [string]$Evidence.Sha256 -ceq [string]$ReceiptArtifact.sha256) {
    throw "$Label does not show a distinct post-preparation Authenticode image."
  }
  $imageEvidence = Get-CodeHangarAuthenticodeImageEvidence -Path $Evidence.Path -Label $Label
  if ($imageEvidence.Sha256 -cne $Evidence.Sha256 -or
      $imageEvidence.ImageSha256 -cne [string]$ReceiptArtifact.authenticode_image_sha256) {
    throw "$Label is not the Authenticode-only transformation bound by its signing receipt."
  }
}

function Open-ValidatedOriginalPreparation {
  param(
    [ValidateSet('Local', 'Connector')][string]$Edition,
    [object]$ReceiptLock,
    [string]$ExpectedReceiptSha256,
    [object]$Identity,
    [string]$PublicBlobHex
  )
  if ([System.IO.Path]::GetFileName($ReceiptLock.Path) -cne 'code-hangar-signing-receipt.json') {
    throw "$Edition signing receipt input must retain the canonical code-hangar-signing-receipt.json filename."
  }
  $target = Get-CodeHangarHostTargetTriple
  $cargoLock = Get-StableReleaseArtifactEvidence -Path (Join-Path $repoRoot 'Cargo.lock') -Label 'Current Cargo.lock'
  $bundleContract = Get-CodeHangarBundleContractSha256 -RepoRoot $repoRoot -Edition $Edition
  $validated = Read-AndValidateCodeHangarSigningReceipt `
    -SigningDirectory (Split-Path -Parent $ReceiptLock.Path) `
    -Edition $Edition `
    -ExpectedVersion $script:ExpectedVersion `
    -ExpectedTargetTriple $target `
    -ExpectedPublicBlobHex (Assert-ReleaseRootPublicBlobHex -Value $PublicBlobHex) `
    -ExpectedCargoLockSha256 $cargoLock.Sha256 `
    -ExpectedBundleContractSha256 $bundleContract `
    -ExpectedSourceCommit $Identity.commit `
    -ExpectedSourceTree $Identity.tree `
    -ExpectedReceiptSha256 $ExpectedReceiptSha256
  if ($validated.Evidence.Sha256 -cne $ReceiptLock.Sha256 -or $validated.Evidence.Length -ne $ReceiptLock.Bytes) {
    throw "$Edition signing preparation validation did not consume the already locked receipt bytes."
  }
  $frontendLocks = $null
  $verifierLock = $null
  $mcpLock = $null
  $manifestLock = $null
  try {
    $frontendLocks = Open-CodeHangarFrontendSnapshotReadLocks -FrontendSnapshot $validated.Frontend -Label "$Edition original prepared frontend"
    $manifestLock = Open-LockedInput -Path $validated.Frontend.Manifest.Path -Label "$Edition prepared frontend manifest"
    $verifierLock = Open-LockedInput -Path $validated.Verifier.Path -Label "$Edition prepared release verifier"
    if ($Edition -eq 'Connector') {
      $mcpLock = Open-LockedInput -Path $validated.Mcp.Path -Label 'Connector prepared MCP sidecar'
    }
    [void](Assert-CodeHangarFrontendSnapshotState -FrontendSnapshot $validated.Frontend -Label "$Edition locked prepared frontend")
    return [pscustomobject]@{
      Edition = $Edition
      Receipt = $ReceiptLock
      Validated = $validated
      FrontendLocks = $frontendLocks
      Manifest = $manifestLock
      Verifier = $verifierLock
      Mcp = $mcpLock
    }
  } catch {
    if ($null -ne $mcpLock) { $mcpLock.Stream.Dispose() }
    if ($null -ne $verifierLock) { $verifierLock.Stream.Dispose() }
    if ($null -ne $manifestLock) { $manifestLock.Stream.Dispose() }
    if ($null -ne $frontendLocks) {
      $frontendLocks.ManifestLock.Dispose()
      Close-ReleaseArtifactLocks -Locks $frontendLocks.TreeLocks
    }
    throw
  }
}

function Close-ValidatedOriginalPreparation {
  param([object]$Preparation)
  if ($null -eq $Preparation) { return }
  if ($null -ne $Preparation.Mcp) { $Preparation.Mcp.Stream.Dispose() }
  if ($null -ne $Preparation.Verifier) { $Preparation.Verifier.Stream.Dispose() }
  if ($null -ne $Preparation.Manifest) { $Preparation.Manifest.Stream.Dispose() }
  if ($null -ne $Preparation.FrontendLocks) {
    $Preparation.FrontendLocks.ManifestLock.Dispose()
    Close-ReleaseArtifactLocks -Locks $Preparation.FrontendLocks.TreeLocks
  }
}

function Copy-ValidatedOriginalPreparation {
  param([object]$Preparation, [string]$Root)
  $editionName = $Preparation.Edition.ToLowerInvariant()
  $destination = Join-Path $Root "snapshots\$editionName-preparation"
  [void][System.IO.Directory]::CreateDirectory($destination)
  foreach ($copy in @(
      @{ evidence = $Preparation.Receipt; name = 'code-hangar-signing-receipt.json'; label = "$($Preparation.Edition) receipt" },
      @{ evidence = $Preparation.Manifest; name = 'code-hangar-frontend-dist.json'; label = "$($Preparation.Edition) frontend manifest" },
      @{ evidence = $Preparation.Verifier; name = 'code-hangar-release-verify.exe'; label = "$($Preparation.Edition) verifier" }
    )) {
    $output = Copy-LockedInput -InputEvidence $copy.evidence -DestinationPath (Join-Path $destination $copy.name) -Label $copy.label
    $output.Stream.Dispose()
  }
  if ($Preparation.Edition -eq 'Connector') {
    $output = Copy-LockedInput -InputEvidence $Preparation.Mcp -DestinationPath (Join-Path $destination 'code-hangar-mcp.exe') -Label 'Connector prepared MCP'
    $output.Stream.Dispose()
  }
  $treeCopy = Copy-CanonicalReleaseTree `
    -SourceRoot $Preparation.Validated.Frontend.Directory `
    -DestinationRoot (Join-Path $destination 'frontend-dist') `
    -Schema $script:CodeHangarFrontendSnapshotSchema `
    -Label "$($Preparation.Edition) release-proof frontend snapshot"
  if ($treeCopy.Destination.Count -ne $Preparation.Validated.Frontend.Tree.Count -or
      $treeCopy.Destination.Sha256 -cne $Preparation.Validated.Frontend.Tree.Sha256) {
    throw "$($Preparation.Edition) copied frontend tree does not match the locked signing preparation."
  }
}

function Get-SnapshotMap {
  return [ordered]@{
    localReceipt = 'snapshots/local-preparation/code-hangar-signing-receipt.json'
    connectorReceipt = 'snapshots/connector-preparation/code-hangar-signing-receipt.json'
    localIdentity = 'snapshots/local-release-identity.json'
    connectorIdentity = 'snapshots/connector-release-identity.json'
    localSetup = 'snapshots/local-setup.exe'
    localParent = 'snapshots/local-parent.exe'
    localHelper = 'snapshots/local-helper.exe'
    localUninstaller = 'snapshots/local-uninstaller.exe'
    connectorSetup = 'snapshots/connector-setup.exe'
    connectorParent = 'snapshots/connector-parent.exe'
    connectorHelper = 'snapshots/connector-helper.exe'
    connectorUninstaller = 'snapshots/connector-uninstaller.exe'
    connectorMcp = 'snapshots/connector-mcp.exe'
    lifecycle = 'snapshots/lifecycle-manifest.json'
  }
}

function Open-ProofSnapshots {
  param([string]$Root)
  $map = Get-SnapshotMap
  $opened = [ordered]@{}
  try {
    foreach ($entry in $map.GetEnumerator()) {
      $opened[$entry.Key] = Open-LockedInput -Path (Join-Path $Root $entry.Value) -Label "Release proof $($entry.Key)"
    }
    return $opened
  } catch {
    foreach ($item in $opened.Values) { $item.Stream.Dispose() }
    throw
  }
}

function Close-ProofSnapshots {
  param([object]$Snapshots)
  if ($null -eq $Snapshots) { return }
  foreach ($item in $Snapshots.Values) { $item.Stream.Dispose() }
}

function Assert-ProofInventory {
  param([string]$Root)
  $expectedFixed = @((Get-SnapshotMap).Values) + @(
    $script:ProofFileName,
    'snapshots/local-preparation/code-hangar-frontend-dist.json',
    'snapshots/local-preparation/code-hangar-release-verify.exe',
    'snapshots/connector-preparation/code-hangar-frontend-dist.json',
    'snapshots/connector-preparation/code-hangar-release-verify.exe',
    'snapshots/connector-preparation/code-hangar-mcp.exe'
  )
  $actual = [System.Collections.Generic.List[string]]::new()
  $pending = [System.Collections.Generic.Stack[string]]::new()
  $pending.Push($Root)
  while ($pending.Count -gt 0) {
    $directory = $pending.Pop()
    foreach ($item in Get-ChildItem -LiteralPath $directory -Force) {
      if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Release proof contains a reparse point: $($item.FullName)"
      }
      if ($item.PSIsContainer) {
        $pending.Push($item.FullName)
      } else {
        $actual.Add([System.IO.Path]::GetRelativePath($Root, $item.FullName).Replace('\', '/'))
      }
    }
  }
  $frontendPrefixes = @(
    'snapshots/local-preparation/frontend-dist/',
    'snapshots/connector-preparation/frontend-dist/'
  )
  $nonFrontend = @($actual | Where-Object {
      $path = [string]$_
      -not @($frontendPrefixes | Where-Object { $path.StartsWith($_, [System.StringComparison]::Ordinal) }).Count
    })
  if ((@($nonFrontend | Sort-Object) -join "`n") -cne (@($expectedFixed | Sort-Object) -join "`n")) {
    throw 'Release proof inventory contains a missing or unexpected fixed file.'
  }
  foreach ($prefix in $frontendPrefixes) {
    if (@($actual | Where-Object { ([string]$_).StartsWith($prefix, [System.StringComparison]::Ordinal) }).Count -lt 1) {
      throw "Release proof inventory has no receipt-bound frontend files below $prefix"
    }
  }
}

function Get-SigningPreparationBinding {
  param([object]$Preparation, [object]$ReceiptEvidence, [string]$Label)
  $receipt = $Preparation.receipt
  $preparedAt = ConvertFrom-CanonicalTimestamp -Value $receipt.prepared_at_utc -Label "$Label prepared_at_utc"
  if ($preparedAt -gt [datetimeoffset]::UtcNow.AddMinutes(5)) {
    throw "$Label signing receipt timestamp is implausibly in the future."
  }
  $binding = [ordered]@{
    schema = [string]$receipt.schema
    sha256 = [string]$ReceiptEvidence.Sha256
    edition = [string]$receipt.edition
    version = [string]$receipt.version
    targetTriple = [string]$receipt.target_triple
    releaseRootPublicBlobSha256 = Get-BytesSha256 -Bytes ([System.Convert]::FromHexString([string]$receipt.release_root_public_blob_hex))
    cargoLockSha256 = [string]$receipt.cargo_lock_sha256
    bundleContractSha256 = [string]$receipt.bundle_contract_sha256
    source = [ordered]@{
      gitCommit = [string]$receipt.source.git_commit
      gitTree = [string]$receipt.source.git_tree
      sourceTreeDirty = [bool]$receipt.source.source_tree_dirty
    }
    preparedAtUtc = [string]$receipt.prepared_at_utc
    frontend = [ordered]@{
      schema = $script:CodeHangarFrontendSnapshotSchema
      directoryName = [string]$receipt.frontend.directory_name
      fileCount = [long]$Preparation.frontend.Tree.Count
      treeSha256 = [string]$Preparation.frontend.Tree.Sha256
      manifestBytes = [long]$Preparation.frontend.Manifest.Length
      manifestSha256 = [string]$Preparation.frontend.Manifest.Sha256
    }
    preparedArtifacts = $receipt.artifacts
  }
  return $binding
}

function Assert-LifecycleInstalledArtifactInventory {
  param(
    [object]$Inventory,
    [ValidateSet('Local', 'Connector')][string]$Edition,
    [object]$Setup,
    [hashtable]$ExpectedArtifacts,
    [string]$ExpectedObservationResultId
  )
  Assert-ExactPropertyNames -Object $Inventory -Expected @(
    'edition', 'setupSha256', 'observationResultId', 'installLocation', 'artifacts'
  ) -Label "$Edition lifecycle installed-artifact inventory"
  if ([string]$Inventory.edition -cne $Edition -or
      [string]$Inventory.setupSha256 -cne [string]$Setup.Sha256 -or
      [string]$Inventory.observationResultId -cne $ExpectedObservationResultId) {
    throw "$Edition lifecycle installed-artifact inventory is not tied to the exact setup and observation."
  }
  if (-not [System.IO.Path]::IsPathFullyQualified([string]$Inventory.installLocation) -or
      ([string]$Inventory.installLocation).StartsWith('\\')) {
    throw "$Edition lifecycle install location must be an absolute local path."
  }
  $installRoot = [System.IO.Path]::GetFullPath([string]$Inventory.installLocation).TrimEnd('\')
  $artifacts = @($Inventory.artifacts | Where-Object { $null -ne $_ })
  Assert-ExactStringInventory `
    -Actual @($artifacts | ForEach-Object { [string]$_.role }) `
    -Expected @($ExpectedArtifacts.Keys) `
    -Label "$Edition lifecycle installed-artifact roles"
  $projection = [System.Collections.Generic.List[object]]::new()
  foreach ($artifact in $artifacts) {
    Assert-ExactPropertyNames -Object $artifact -Expected @(
      'role', 'relativePath', 'canonicalPath', 'bytes', 'sha256'
    ) -Label "$Edition lifecycle installed artifact $($artifact.role)"
    $role = [string]$artifact.role
    if (-not $ExpectedArtifacts.ContainsKey($role)) { throw "$Edition lifecycle installed-artifact role is unexpected: $role" }
    Assert-CanonicalSha256 -Value $artifact.sha256 -Label "$Edition installed $role SHA-256"
    if ([long]$artifact.bytes -le 0 -or
        -not [System.IO.Path]::IsPathFullyQualified([string]$artifact.canonicalPath) -or
        ([string]$artifact.canonicalPath).StartsWith('\\')) {
      throw "$Edition lifecycle installed $role has invalid byte/path identity."
    }
    $canonical = [System.IO.Path]::GetFullPath([string]$artifact.canonicalPath)
    $prefix = $installRoot + '\'
    if (-not $canonical.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "$Edition lifecycle installed $role escapes its install root."
    }
    $expectedRelative = [System.IO.Path]::GetRelativePath($installRoot, $canonical).Replace('\', '/')
    if ([string]$artifact.relativePath -cne $expectedRelative -or
        $expectedRelative.StartsWith('../', [System.StringComparison]::Ordinal) -or
        $expectedRelative.Contains('/../', [System.StringComparison]::Ordinal)) {
      throw "$Edition lifecycle installed $role has a mismatched relative/canonical path identity."
    }
    $knownName = switch ($role) {
      'parent' { 'code-hangar-desktop.exe' }
      'helper' { 'code-hangar-elevated.exe' }
      'mcp' { 'code-hangar-mcp.exe' }
      default { $null }
    }
    if ($null -ne $knownName -and $expectedRelative -cne $knownName) {
      throw "$Edition lifecycle installed $role path is not the canonical $knownName."
    }
    if ($role -eq 'uninstaller' -and
        ($expectedRelative.Contains('/') -or -not $expectedRelative.EndsWith('.exe', [System.StringComparison]::OrdinalIgnoreCase))) {
      throw "$Edition lifecycle uninstaller must be a root-level executable."
    }
    $expected = $ExpectedArtifacts[$role]
    if ([long]$artifact.bytes -ne [long]$expected.Bytes -or
        [string]$artifact.sha256 -cne [string]$expected.Sha256) {
      throw "$Edition lifecycle installed $role bytes do not match the supplied release-proof snapshot."
    }
    $projection.Add([ordered]@{
        role = $role
        relativePath = $expectedRelative
        canonicalPath = $canonical
        bytes = [long]$artifact.bytes
        sha256 = [string]$artifact.sha256
      })
  }
  return [ordered]@{
    edition = $Edition
    setupSha256 = [string]$Inventory.setupSha256
    observationResultId = [string]$Inventory.observationResultId
    installLocation = $installRoot
    artifacts = @($projection | Sort-Object role)
  }
}

function Read-AndValidateLifecycleManifest {
  param([object]$Evidence, [object]$Snapshots, [object]$Identity, [string]$ExpectedHash)
  Assert-CanonicalSha256 -Value $ExpectedHash -Label 'Expected lifecycle manifest SHA-256'
  if ($Evidence.Sha256 -cne $ExpectedHash) { throw 'Lifecycle manifest hash mismatch.' }
  $manifest = Read-LockedJson -Evidence $Evidence -Label 'Lifecycle manifest' -MaximumBytes 1048576
  Assert-ExactPropertyNames -Object $manifest -Expected @(
    'schemaVersion', 'documentType', 'generatedAt', 'evidenceRoot', 'machine',
    'gitCommit', 'gitBranch', 'baselineVersion', 'candidateVersion', 'status',
    'checks', 'results', 'historicalFailuresAccepted', 'sourceProvenance', 'installedArtifacts'
  ) -Label 'Lifecycle manifest'
  $generatedAt = ConvertFrom-CanonicalTimestamp -Value $manifest.generatedAt -Label 'Lifecycle generatedAt'
  if ($generatedAt -gt [datetimeoffset]::UtcNow.AddMinutes(5)) { throw 'Lifecycle generatedAt is implausibly in the future.' }
  if ([int]$manifest.schemaVersion -ne 3 -or [string]$manifest.documentType -cne $script:LifecycleSchema -or
      [string]$manifest.status -cne 'PASS' -or [bool]$manifest.historicalFailuresAccepted -or
      [string]$manifest.baselineVersion -cne '0.1.1' -or [string]$manifest.candidateVersion -cne $script:ExpectedVersion -or
      [string]$manifest.gitCommit -cne $Identity.commit -or [string]::IsNullOrWhiteSpace([string]$manifest.gitBranch) -or
      [string]::IsNullOrWhiteSpace([string]$manifest.machine) -or
      -not [System.IO.Path]::IsPathFullyQualified([string]$manifest.evidenceRoot) -or
      ([string]$manifest.evidenceRoot).StartsWith('\\')) {
    throw 'Lifecycle manifest identity/status fields are not canonical release evidence.'
  }

  $provenance = $manifest.sourceProvenance
  Assert-ExactPropertyNames -Object $provenance -Expected @(
    'schemaVersion', 'recordedAt', 'gitCommit', 'gitTree', 'gitBranch', 'sourceTreeDirty',
    'baselineVersion', 'candidateVersion', 'baselineLocalSha256', 'candidateLocalSha256',
    'candidateConnectorSha256', 'baselineCatalogHelperSha256', 'candidateCatalogHelperSha256',
    'sharedInputs'
  ) -Label 'Lifecycle source provenance'
  [void](ConvertFrom-CanonicalTimestamp -Value $provenance.recordedAt -Label 'Lifecycle provenance recordedAt')
  foreach ($field in @(
      'baselineLocalSha256', 'candidateLocalSha256', 'candidateConnectorSha256',
      'baselineCatalogHelperSha256', 'candidateCatalogHelperSha256'
    )) { Assert-CanonicalSha256 -Value $provenance.$field -Label "Lifecycle $field" }
  if ([int]$provenance.schemaVersion -ne 2 -or [bool]$provenance.sourceTreeDirty -or
      [string]$provenance.gitCommit -cne $Identity.commit -or [string]$provenance.gitTree -cne $Identity.tree -or
      [string]$provenance.gitBranch -cne [string]$manifest.gitBranch -or
      [string]$provenance.baselineVersion -cne '0.1.1' -or [string]$provenance.candidateVersion -cne $script:ExpectedVersion -or
      [string]$provenance.candidateLocalSha256 -cne $Snapshots.localSetup.Sha256 -or
      [string]$provenance.candidateConnectorSha256 -cne $Snapshots.connectorSetup.Sha256) {
    throw 'Lifecycle source provenance does not bind the exact source and setup snapshots.'
  }
  $expectedResultIds = @(Get-ExpectedLifecycleResultIds)
  $expectedInputs = @(
    'Code Hangar_0.1.1_x64-setup.exe',
    'Code Hangar_0.1.3_x64-setup.exe',
    'Code Hangar AI Connector_0.1.3_x64-setup.exe',
    'acceptance_catalog_011.exe',
    'acceptance_catalog_013.exe',
    'sandbox-lifecycle-agent.ps1',
    'VCRUNTIME140.dll',
    'test-project\README.md',
    'test-project\AGENTS.md',
    'test-project\src\main.rs'
  ) + @($expectedResultIds | ForEach-Object { "commands\$_.json" })
  $sharedInputs = @($provenance.sharedInputs | Where-Object { $null -ne $_ })
  Assert-ExactStringInventory -Actual @($sharedInputs | ForEach-Object { [string]$_.path }) -Expected $expectedInputs -Label 'Lifecycle shared-input inventory'
  $inputHashes = @{}
  foreach ($input in $sharedInputs) {
    Assert-ExactPropertyNames -Object $input -Expected @('path', 'sha256') -Label "Lifecycle shared input $($input.path)"
    Assert-CanonicalSha256 -Value $input.sha256 -Label "Lifecycle shared input $($input.path) SHA-256"
    $inputHashes[[string]$input.path] = [string]$input.sha256
  }
  foreach ($binding in @(
      @{ path = 'Code Hangar_0.1.1_x64-setup.exe'; hash = [string]$provenance.baselineLocalSha256 },
      @{ path = 'Code Hangar_0.1.3_x64-setup.exe'; hash = [string]$provenance.candidateLocalSha256 },
      @{ path = 'Code Hangar AI Connector_0.1.3_x64-setup.exe'; hash = [string]$provenance.candidateConnectorSha256 },
      @{ path = 'acceptance_catalog_011.exe'; hash = [string]$provenance.baselineCatalogHelperSha256 },
      @{ path = 'acceptance_catalog_013.exe'; hash = [string]$provenance.candidateCatalogHelperSha256 }
    )) {
    if ([string]$inputHashes[$binding.path] -cne [string]$binding.hash) {
      throw "Lifecycle shared input $($binding.path) disagrees with its direct provenance hash."
    }
  }

  $results = @($manifest.results | Where-Object { $null -ne $_ })
  Assert-ExactStringInventory -Actual @($results | ForEach-Object { [string]$_.id }) -Expected $expectedResultIds -Label 'Lifecycle result inventory'
  foreach ($result in $results) {
    Assert-ExactPropertyNames -Object $result -Expected @('id', 'status', 'startedAt', 'completedAt') -Label "Lifecycle result $($result.id)"
    if ([string]$result.status -cne 'PASS') { throw "Lifecycle result $($result.id) did not pass." }
    $started = ConvertFrom-CanonicalTimestamp -Value $result.startedAt -Label "Lifecycle result $($result.id) startedAt"
    $completed = ConvertFrom-CanonicalTimestamp -Value $result.completedAt -Label "Lifecycle result $($result.id) completedAt"
    if ($completed -lt $started -or $completed -gt $generatedAt.AddMinutes(5)) {
      throw "Lifecycle result $($result.id) timestamps are not monotonic."
    }
  }
  $checks = @($manifest.checks | Where-Object { $null -ne $_ })
  Assert-ExactStringInventory -Actual @($checks | ForEach-Object { [string]$_.name }) -Expected @(
    'source-provenance', 'guest-agent-lifecycle', 'clean-offline-local-install', 'baseline-catalog',
    'upgrade-0.1.1-to-0.1.3', 'edition-coexistence-and-repair', 'uninstall-switching-and-final-state'
  ) -Label 'Lifecycle check inventory'
  foreach ($check in $checks) {
    if ([string]$check.status -cne 'PASS') { throw "Lifecycle check $($check.name) did not pass." }
  }

  Assert-ExactPropertyNames -Object $manifest.installedArtifacts -Expected @('Local', 'Connector') -Label 'Lifecycle installed-artifact editions'
  $localInstalled = Assert-LifecycleInstalledArtifactInventory `
    -Inventory $manifest.installedArtifacts.Local `
    -Edition Local `
    -Setup $Snapshots.localSetup `
    -ExpectedArtifacts @{
      parent = $Snapshots.localParent
      helper = $Snapshots.localHelper
      uninstaller = $Snapshots.localUninstaller
    } `
    -ExpectedObservationResultId '01-clean-install-local-013'
  $connectorInstalled = Assert-LifecycleInstalledArtifactInventory `
    -Inventory $manifest.installedArtifacts.Connector `
    -Edition Connector `
    -Setup $Snapshots.connectorSetup `
    -ExpectedArtifacts @{
      parent = $Snapshots.connectorParent
      helper = $Snapshots.connectorHelper
      mcp = $Snapshots.connectorMcp
      uninstaller = $Snapshots.connectorUninstaller
    } `
    -ExpectedObservationResultId '11-install-connector-013'
  return [pscustomobject]@{
    Manifest = $manifest
    ResultIds = @($expectedResultIds | Sort-Object)
    InstalledArtifacts = [ordered]@{ Local = $localInstalled; Connector = $connectorInstalled }
    SharedInputs = @($sharedInputs | Sort-Object path)
  }
}

function Get-ValidatedProofBindings {
  param(
    [string]$Root,
    [object]$Snapshots,
    [string]$Decision,
    [string]$SignerSubject,
    [string]$SignerThumbprint,
    [string]$PublicBlobHex,
    [object]$Identity,
    [string]$LocalReceiptHash,
    [string]$ConnectorReceiptHash,
    [string]$LocalIdentityHash,
    [string]$ConnectorIdentityHash,
    [string]$LifecycleHash
  )
  $localPreparation = Read-AndValidateReceipt -Evidence $Snapshots.localReceipt -Edition Local -ExpectedHash $LocalReceiptHash -Identity $Identity -PublicBlobHex $PublicBlobHex
  $connectorPreparation = Read-AndValidateReceipt -Evidence $Snapshots.connectorReceipt -Edition Connector -ExpectedHash $ConnectorReceiptHash -Identity $Identity -PublicBlobHex $PublicBlobHex
  $localReceipt = $localPreparation.receipt
  $connectorReceipt = $connectorPreparation.receipt
  if ($Snapshots.localParent.Sha256 -ceq $Snapshots.localHelper.Sha256 -or
      $Snapshots.connectorParent.Sha256 -ceq $Snapshots.connectorHelper.Sha256) {
    throw 'An edition parent and elevated helper must be distinct authenticated binaries.'
  }
  Assert-PostSigningReceiptLink -Evidence $Snapshots.localParent -ReceiptArtifact $localReceipt.artifacts.parent -ExpectedFileName 'code-hangar-desktop.exe' -Label 'Local parent'
  Assert-PostSigningReceiptLink -Evidence $Snapshots.localHelper -ReceiptArtifact $localReceipt.artifacts.helper -ExpectedFileName 'code-hangar-elevated.exe' -Label 'Local helper'
  Assert-PostSigningReceiptLink -Evidence $Snapshots.connectorParent -ReceiptArtifact $connectorReceipt.artifacts.parent -ExpectedFileName 'code-hangar-desktop.exe' -Label 'Connector parent'
  Assert-PostSigningReceiptLink -Evidence $Snapshots.connectorHelper -ReceiptArtifact $connectorReceipt.artifacts.helper -ExpectedFileName 'code-hangar-elevated.exe' -Label 'Connector helper'
  Assert-ExactPropertyNames -Object $connectorReceipt.artifacts.mcp -Expected @('file_name', 'length', 'sha256') -Label 'Connector receipt MCP'
  if ([string]$connectorReceipt.artifacts.mcp.file_name -cne 'code-hangar-mcp.exe' -or
      [long]$connectorReceipt.artifacts.mcp.length -ne $Snapshots.connectorMcp.Bytes -or
      [string]$connectorReceipt.artifacts.mcp.sha256 -cne $Snapshots.connectorMcp.Sha256) {
    throw 'Connector MCP snapshot does not match the receipt-bound sidecar.'
  }
  $localIdentity = Read-AndValidateReleaseIdentity -Evidence $Snapshots.localIdentity -Edition Local -ExpectedHash $LocalIdentityHash -ReceiptHash $LocalReceiptHash -Parent $Snapshots.localParent -Helper $Snapshots.localHelper -PublicBlobHex $PublicBlobHex
  $connectorIdentity = Read-AndValidateReleaseIdentity -Evidence $Snapshots.connectorIdentity -Edition Connector -ExpectedHash $ConnectorIdentityHash -ReceiptHash $ConnectorReceiptHash -Parent $Snapshots.connectorParent -Helper $Snapshots.connectorHelper -PublicBlobHex $PublicBlobHex

  $innerLocalParent = Get-AuthenticodeProof -Evidence $Snapshots.localParent -ExpectedStatus Valid -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Local installed parent'
  $innerLocalHelper = Get-AuthenticodeProof -Evidence $Snapshots.localHelper -ExpectedStatus Valid -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Local installed helper'
  $innerConnectorParent = Get-AuthenticodeProof -Evidence $Snapshots.connectorParent -ExpectedStatus Valid -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Connector installed parent'
  $innerConnectorHelper = Get-AuthenticodeProof -Evidence $Snapshots.connectorHelper -ExpectedStatus Valid -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Connector installed helper'
  $outerStatus = if ($Decision -eq 'Signed') { 'Valid' } else { 'NotSigned' }
  $localSetupSignature = Get-AuthenticodeProof -Evidence $Snapshots.localSetup -ExpectedStatus $outerStatus -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Local setup'
  $localUninstallerSignature = Get-AuthenticodeProof -Evidence $Snapshots.localUninstaller -ExpectedStatus $outerStatus -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Local installed uninstaller'
  $connectorSetupSignature = Get-AuthenticodeProof -Evidence $Snapshots.connectorSetup -ExpectedStatus $outerStatus -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Connector setup'
  $connectorUninstallerSignature = Get-AuthenticodeProof -Evidence $Snapshots.connectorUninstaller -ExpectedStatus $outerStatus -SignerSubject $SignerSubject -SignerThumbprint $SignerThumbprint -Label 'Connector installed uninstaller'

  $lifecycleValidation = Read-AndValidateLifecycleManifest `
    -Evidence $Snapshots.lifecycle `
    -Snapshots $Snapshots `
    -Identity $Identity `
    -ExpectedHash $LifecycleHash
  $lifecycle = $lifecycleValidation.Manifest

  return [ordered]@{
    signingDecision = [ordered]@{
      value = $Decision
      ownerAuthorized = $true
      unsignedOuterAccepted = $Decision -eq 'Unsigned'
      smartScreenDisclosureRequired = $Decision -eq 'Unsigned'
      signerSubject = $SignerSubject
      signerThumbprint = $SignerThumbprint
    }
    releaseRootPublicBlobSha256 = Get-BytesSha256 -Bytes ([System.Convert]::FromHexString((Assert-ReleaseRootPublicBlobHex -Value $PublicBlobHex)))
    lifecycle = [ordered]@{
      schemaVersion = [int]$lifecycle.schemaVersion
      documentType = [string]$lifecycle.documentType
      sha256 = $Snapshots.lifecycle.Sha256
      localSetupSha256 = $Snapshots.localSetup.Sha256
      connectorSetupSha256 = $Snapshots.connectorSetup.Sha256
      resultIds = @($lifecycleValidation.ResultIds)
      sharedInputs = @($lifecycleValidation.SharedInputs | ForEach-Object {
          [ordered]@{ name = [string]$_.path; sha256 = [string]$_.sha256 }
        })
      installedArtifacts = $lifecycleValidation.InstalledArtifacts
    }
    editions = [ordered]@{
      Local = [ordered]@{
        signingReceipt = Get-SigningPreparationBinding -Preparation $localPreparation -ReceiptEvidence $Snapshots.localReceipt -Label 'Local'
        releaseIdentity = [ordered]@{ schema = $localIdentity.Manifest.schema; sha256 = $Snapshots.localIdentity.Sha256; releaseId = $localIdentity.ReleaseId }
        artifacts = [ordered]@{
          setup = [ordered]@{ bytes = $Snapshots.localSetup.Bytes; sha256 = $Snapshots.localSetup.Sha256; authenticode = $localSetupSignature }
          parent = [ordered]@{ bytes = $Snapshots.localParent.Bytes; sha256 = $Snapshots.localParent.Sha256; authenticode = $innerLocalParent }
          helper = [ordered]@{ bytes = $Snapshots.localHelper.Bytes; sha256 = $Snapshots.localHelper.Sha256; authenticode = $innerLocalHelper }
          uninstaller = [ordered]@{ bytes = $Snapshots.localUninstaller.Bytes; sha256 = $Snapshots.localUninstaller.Sha256; authenticode = $localUninstallerSignature }
        }
      }
      Connector = [ordered]@{
        signingReceipt = Get-SigningPreparationBinding -Preparation $connectorPreparation -ReceiptEvidence $Snapshots.connectorReceipt -Label 'Connector'
        releaseIdentity = [ordered]@{ schema = $connectorIdentity.Manifest.schema; sha256 = $Snapshots.connectorIdentity.Sha256; releaseId = $connectorIdentity.ReleaseId }
        artifacts = [ordered]@{
          setup = [ordered]@{ bytes = $Snapshots.connectorSetup.Bytes; sha256 = $Snapshots.connectorSetup.Sha256; authenticode = $connectorSetupSignature }
          parent = [ordered]@{ bytes = $Snapshots.connectorParent.Bytes; sha256 = $Snapshots.connectorParent.Sha256; authenticode = $innerConnectorParent }
          helper = [ordered]@{ bytes = $Snapshots.connectorHelper.Bytes; sha256 = $Snapshots.connectorHelper.Sha256; authenticode = $innerConnectorHelper }
          uninstaller = [ordered]@{ bytes = $Snapshots.connectorUninstaller.Bytes; sha256 = $Snapshots.connectorUninstaller.Sha256; authenticode = $connectorUninstallerSignature }
          mcp = [ordered]@{ bytes = $Snapshots.connectorMcp.Bytes; sha256 = $Snapshots.connectorMcp.Sha256; receiptSha256 = [string]$connectorReceipt.artifacts.mcp.sha256 }
        }
      }
    }
  }
}

function Create-ReleaseProof {
  param([string]$Root)
  if (-not $OwnerAuthorized) { throw '-Create requires the explicit supervised owner gate: -OwnerAuthorized.' }
  if ($SigningDecision -eq 'Unsigned' -and -not $OwnerAcceptUnsignedOuter) {
    throw 'Unsigned outer setup/uninstaller publication requires -OwnerAcceptUnsignedOuter.'
  }
  if ($SigningDecision -eq 'Signed' -and $OwnerAcceptUnsignedOuter) {
    throw '-OwnerAcceptUnsignedOuter conflicts with SigningDecision Signed.'
  }
  if ([string]::IsNullOrWhiteSpace($ExpectedSignerSubject)) { throw 'ExpectedSignerSubject is required.' }
  Assert-CanonicalThumbprint -Value $ExpectedSignerThumbprint -Label 'Expected signer thumbprint'
  [void](Assert-ReleaseRootPublicBlobHex -Value $ReleaseRootPublicBlobHex)
  $identity = Get-CleanGitIdentity

  $inputs = [ordered]@{
    localReceipt = $LocalSigningReceiptPath
    connectorReceipt = $ConnectorSigningReceiptPath
    localIdentity = $LocalReleaseIdentityPath
    connectorIdentity = $ConnectorReleaseIdentityPath
    localSetup = $LocalSetupPath
    localParent = $LocalParentPath
    localHelper = $LocalHelperPath
    localUninstaller = $LocalUninstallerPath
    connectorSetup = $ConnectorSetupPath
    connectorParent = $ConnectorParentPath
    connectorHelper = $ConnectorHelperPath
    connectorUninstaller = $ConnectorUninstallerPath
    connectorMcp = $ConnectorMcpPath
    lifecycle = $LifecycleManifestPath
  }
  foreach ($entry in $inputs.GetEnumerator()) {
    if ([string]::IsNullOrWhiteSpace([string]$entry.Value)) { throw "-$($entry.Key) input is required." }
  }
  foreach ($hash in @(
      @{ Value = $ExpectedLocalSigningReceiptSha256; Label = 'Expected Local receipt hash' },
      @{ Value = $ExpectedConnectorSigningReceiptSha256; Label = 'Expected Connector receipt hash' },
      @{ Value = $ExpectedLocalReleaseIdentitySha256; Label = 'Expected Local identity hash' },
      @{ Value = $ExpectedConnectorReleaseIdentitySha256; Label = 'Expected Connector identity hash' },
      @{ Value = $ExpectedLifecycleManifestSha256; Label = 'Expected lifecycle hash' }
    )) { Assert-CanonicalSha256 -Value $hash.Value -Label $hash.Label }

  [void][System.IO.Directory]::CreateDirectory($Root)
  $snapshotDir = Join-Path $Root 'snapshots'
  [void][System.IO.Directory]::CreateDirectory($snapshotDir)
  $sourceLocks = [ordered]@{}
  $snapshots = $null
  $localPreparation = $null
  $connectorPreparation = $null
  try {
    foreach ($entry in $inputs.GetEnumerator()) {
      $sourceLocks[$entry.Key] = Open-LockedInput -Path $entry.Value -Label "Release proof input $($entry.Key)"
    }
    $localPreparation = Open-ValidatedOriginalPreparation `
      -Edition Local `
      -ReceiptLock $sourceLocks.localReceipt `
      -ExpectedReceiptSha256 $ExpectedLocalSigningReceiptSha256 `
      -Identity $identity `
      -PublicBlobHex $ReleaseRootPublicBlobHex
    $connectorPreparation = Open-ValidatedOriginalPreparation `
      -Edition Connector `
      -ReceiptLock $sourceLocks.connectorReceipt `
      -ExpectedReceiptSha256 $ExpectedConnectorSigningReceiptSha256 `
      -Identity $identity `
      -PublicBlobHex $ReleaseRootPublicBlobHex
    Copy-ValidatedOriginalPreparation -Preparation $localPreparation -Root $Root
    Copy-ValidatedOriginalPreparation -Preparation $connectorPreparation -Root $Root
    $map = Get-SnapshotMap
    foreach ($entry in $map.GetEnumerator()) {
      if ($entry.Key -in @('localReceipt', 'connectorReceipt')) { continue }
      $copy = Copy-LockedInput -InputEvidence $sourceLocks[$entry.Key] -DestinationPath (Join-Path $Root $entry.Value) -Label "Release proof snapshot $($entry.Key)"
      $copy.Stream.Dispose()
    }
    $snapshots = Open-ProofSnapshots -Root $Root
    $bindings = Get-ValidatedProofBindings `
      -Root $Root `
      -Snapshots $snapshots `
      -Decision $SigningDecision `
      -SignerSubject $ExpectedSignerSubject `
      -SignerThumbprint $ExpectedSignerThumbprint `
      -PublicBlobHex $ReleaseRootPublicBlobHex `
      -Identity $identity `
      -LocalReceiptHash $ExpectedLocalSigningReceiptSha256 `
      -ConnectorReceiptHash $ExpectedConnectorSigningReceiptSha256 `
      -LocalIdentityHash $ExpectedLocalReleaseIdentitySha256 `
      -ConnectorIdentityHash $ExpectedConnectorReleaseIdentitySha256 `
      -LifecycleHash $ExpectedLifecycleManifestSha256
    $report = [ordered]@{
      schemaVersion = 1
      documentType = $script:ProofSchema
      version = $script:ExpectedVersion
      status = 'PASS'
      sealedAt = (Get-Date).ToString('o')
      source = [ordered]@{ gitCommit = $identity.commit; gitTree = $identity.tree; sourceTreeDirty = $false }
      releaseRootPublicBlobHex = (Assert-ReleaseRootPublicBlobHex -Value $ReleaseRootPublicBlobHex)
      bindings = $bindings
      snapshots = [ordered]@{}
    }
    foreach ($entry in (Get-SnapshotMap).GetEnumerator()) {
      $evidence = $snapshots[$entry.Key]
      $report.snapshots[$entry.Key] = [ordered]@{ path = $entry.Value; bytes = $evidence.Bytes; sha256 = $evidence.Sha256 }
    }
    $reportPath = Join-Path $Root $script:ProofFileName
    Write-NewJson -Path $reportPath -Value $report
    $reportEvidence = Open-LockedInput -Path $reportPath -Label 'Release proof report'
    try {
      $reportHash = $reportEvidence.Sha256
    } finally {
      $reportEvidence.Stream.Dispose()
    }
    Write-Host "Sealed private release artifact proof: $reportPath" -ForegroundColor Green
    Write-Host "RELEASE-ARTIFACT-PROOF SHA-256: $reportHash" -ForegroundColor Green
  } finally {
    Close-ProofSnapshots -Snapshots $snapshots
    Close-ValidatedOriginalPreparation -Preparation $connectorPreparation
    Close-ValidatedOriginalPreparation -Preparation $localPreparation
    foreach ($item in $sourceLocks.Values) { $item.Stream.Dispose() }
  }
}

function Validate-ReleaseProof {
  param([string]$Root, [string]$ExpectedHash)
  Assert-CanonicalSha256 -Value $ExpectedHash -Label 'Expected release artifact proof SHA-256'
  Assert-ProofInventory -Root $Root
  $identity = Get-CleanGitIdentity
  $reportEvidence = Open-LockedInput -Path (Join-Path $Root $script:ProofFileName) -Label 'Release artifact proof report'
  $snapshots = $null
  try {
    if ($reportEvidence.Sha256 -cne $ExpectedHash) { throw 'Release artifact proof report hash mismatch.' }
    $report = Read-LockedJson -Evidence $reportEvidence -Label 'Release artifact proof report' -MaximumBytes 1048576
    Assert-ExactPropertyNames -Object $report -Expected @(
      'schemaVersion', 'documentType', 'version', 'status', 'sealedAt', 'source',
      'releaseRootPublicBlobHex', 'bindings', 'snapshots'
    ) -Label 'Release artifact proof report'
    if ([int]$report.schemaVersion -ne 1 -or [string]$report.documentType -cne $script:ProofSchema -or
        [string]$report.version -cne $script:ExpectedVersion -or [string]$report.status -cne 'PASS' -or
        [string]$report.source.gitCommit -cne $identity.commit -or
        [string]$report.source.gitTree -cne $identity.tree -or [bool]$report.source.sourceTreeDirty) {
      throw 'Release artifact proof does not bind the exact clean release source.'
    }
    $snapshots = Open-ProofSnapshots -Root $Root
    Assert-ExactPropertyNames -Object $report.snapshots -Expected @((Get-SnapshotMap).Keys) -Label 'Release proof snapshot inventory'
    foreach ($entry in (Get-SnapshotMap).GetEnumerator()) {
      $record = $report.snapshots.($entry.Key)
      if ([string]$record.path -cne $entry.Value -or [long]$record.bytes -ne $snapshots[$entry.Key].Bytes -or
          [string]$record.sha256 -cne $snapshots[$entry.Key].Sha256) {
        throw "Release proof snapshot $($entry.Key) changed after sealing."
      }
    }
    $decision = [string]$report.bindings.signingDecision.value
    if ($decision -notin @('Signed', 'Unsigned') -or -not [bool]$report.bindings.signingDecision.ownerAuthorized -or
        ($decision -eq 'Unsigned' -and -not [bool]$report.bindings.signingDecision.unsignedOuterAccepted)) {
      throw 'Release proof has no valid owner signing decision.'
    }
    $recomputed = Get-ValidatedProofBindings `
      -Root $Root `
      -Snapshots $snapshots `
      -Decision $decision `
      -SignerSubject ([string]$report.bindings.signingDecision.signerSubject) `
      -SignerThumbprint ([string]$report.bindings.signingDecision.signerThumbprint) `
      -PublicBlobHex ([string]$report.releaseRootPublicBlobHex) `
      -Identity $identity `
      -LocalReceiptHash ([string]$report.bindings.editions.Local.signingReceipt.sha256) `
      -ConnectorReceiptHash ([string]$report.bindings.editions.Connector.signingReceipt.sha256) `
      -LocalIdentityHash ([string]$report.bindings.editions.Local.releaseIdentity.sha256) `
      -ConnectorIdentityHash ([string]$report.bindings.editions.Connector.releaseIdentity.sha256) `
      -LifecycleHash ([string]$report.bindings.lifecycle.sha256)
    if (($recomputed | ConvertTo-Json -Depth 16 -Compress) -cne
        ($report.bindings | ConvertTo-Json -Depth 16 -Compress)) {
      throw 'Release artifact proof bindings no longer match the locked snapshot bytes.'
    }
    return [pscustomobject]@{ Path = $reportEvidence.Path; Sha256 = $reportEvidence.Sha256; Report = $report }
  } finally {
    Close-ProofSnapshots -Snapshots $snapshots
    $reportEvidence.Stream.Dispose()
  }
}

if ($SelfTest) {
  if ($Create -or $ValidateOnly -or $OwnerAuthorized -or $OwnerAcceptUnsignedOuter) {
    throw '-SelfTest accepts no release mode or owner authorization.'
  }
  # PowerShell's bundled executable carries an embedded signature; many Windows
  # inbox executables are catalog-signed and therefore cannot exercise the
  # receipt-bound embedded Authenticode/RFC3161 parser used for release files.
  $signedPath = Join-Path $PSHOME 'pwsh.exe'
  $signed = Open-LockedInput -Path $signedPath -Label 'Authenticode self-test executable'
  $tempParent = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $tempRoot = Join-Path $tempParent ('codehangar-release-proof-selftest-' + [guid]::NewGuid().ToString('N'))
  [void][System.IO.Directory]::CreateDirectory($tempRoot)
  $snapshot = $null
  try {
    $signature = Get-AuthenticodeSignature -LiteralPath $signed.Path
    if ([string]$signature.Status -cne 'Valid' -or $null -eq $signature.SignerCertificate) {
      throw 'Release-proof self-test requires the embedded-signed PowerShell executable.'
    }
    [void](Get-AuthenticodeProof `
      -Evidence $signed `
      -ExpectedStatus Valid `
      -SignerSubject $signature.SignerCertificate.Subject `
      -SignerThumbprint $signature.SignerCertificate.Thumbprint `
      -Label 'Authenticode self-test executable')
    $primaryPe = Get-EmbeddedAuthenticodeCms -Stream $signed.Stream
    $primaryCms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
    $primaryCms.Decode($primaryPe.CmsBytes)
    $timestampAttribute = @($primaryCms.SignerInfos[0].UnsignedAttributes | Where-Object {
        $_.Oid.Value -eq '1.3.6.1.4.1.311.3.3.1'
      })[0]
    $timestampCms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
    $timestampCms.Decode([byte[]]$timestampAttribute.Values[0].RawData)
    $primarySignature = [byte[]]$primaryCms.SignerInfos[0].GetSignature()
    [void](Get-Rfc3161Info -Content $timestampCms.ContentInfo.Content -PrimarySignature $primarySignature -Label 'RFC3161 self-test')
    $tamperedPrimarySignature = [byte[]]$primarySignature.Clone()
    $tamperedPrimarySignature[0] = $tamperedPrimarySignature[0] -bxor 1
    $imprintTamperRejected = $false
    try {
      [void](Get-Rfc3161Info -Content $timestampCms.ContentInfo.Content -PrimarySignature $tamperedPrimarySignature -Label 'RFC3161 tamper self-test')
    } catch { $imprintTamperRejected = $true }
    if (-not $imprintTamperRejected) {
      throw 'RFC3161 messageImprint self-test accepted a transplanted or changed primary signature.'
    }

    $fakeSetup = [pscustomobject]@{ Sha256 = ('aa' * 32) }
    $fakeParent = [pscustomobject]@{ Bytes = 101; Sha256 = ('11' * 32) }
    $fakeHelper = [pscustomobject]@{ Bytes = 102; Sha256 = ('22' * 32) }
    $fakeMcp = [pscustomobject]@{ Bytes = 103; Sha256 = ('33' * 32) }
    $fakeUninstaller = [pscustomobject]@{ Bytes = 104; Sha256 = ('44' * 32) }
    $fakeInventory = [pscustomobject]@{
      edition = 'Connector'
      setupSha256 = $fakeSetup.Sha256
      observationResultId = '11-install-connector-013'
      installLocation = 'C:\Program Files\Code Hangar AI Connector'
      artifacts = @(
        [pscustomobject]@{ role = 'parent'; relativePath = 'code-hangar-desktop.exe'; canonicalPath = 'C:\Program Files\Code Hangar AI Connector\code-hangar-desktop.exe'; bytes = 101; sha256 = $fakeParent.Sha256 },
        [pscustomobject]@{ role = 'helper'; relativePath = 'code-hangar-elevated.exe'; canonicalPath = 'C:\Program Files\Code Hangar AI Connector\code-hangar-elevated.exe'; bytes = 102; sha256 = $fakeHelper.Sha256 },
        [pscustomobject]@{ role = 'mcp'; relativePath = 'code-hangar-mcp.exe'; canonicalPath = 'C:\Program Files\Code Hangar AI Connector\code-hangar-mcp.exe'; bytes = 103; sha256 = $fakeMcp.Sha256 },
        [pscustomobject]@{ role = 'uninstaller'; relativePath = 'uninstall.exe'; canonicalPath = 'C:\Program Files\Code Hangar AI Connector\uninstall.exe'; bytes = 104; sha256 = $fakeUninstaller.Sha256 }
      )
    }
    [void](Assert-LifecycleInstalledArtifactInventory `
      -Inventory $fakeInventory `
      -Edition Connector `
      -Setup $fakeSetup `
      -ExpectedArtifacts @{ parent = $fakeParent; helper = $fakeHelper; mcp = $fakeMcp; uninstaller = $fakeUninstaller } `
      -ExpectedObservationResultId '11-install-connector-013')
    $foreignParent = [pscustomobject]@{ Bytes = 101; Sha256 = ('99' * 32) }
    $substitutionRejected = $false
    try {
      [void](Assert-LifecycleInstalledArtifactInventory `
        -Inventory $fakeInventory `
        -Edition Connector `
        -Setup $fakeSetup `
        -ExpectedArtifacts @{ parent = $foreignParent; helper = $fakeHelper; mcp = $fakeMcp; uninstaller = $fakeUninstaller } `
        -ExpectedObservationResultId '11-install-connector-013')
    } catch { $substitutionRejected = $true }
    if (-not $substitutionRejected) {
      throw 'Release-proof self-test accepted a parent snapshot from another installation or build.'
    }
    $snapshot = Copy-LockedInput -InputEvidence $signed -DestinationPath (Join-Path $tempRoot 'snapshot.exe') -Label 'Release-proof self-test snapshot'
    $writerRejected = $false
    try {
      $writer = [System.IO.FileStream]::new($snapshot.Path, 'Open', 'Write', 'Read')
      $writer.Dispose()
    } catch [System.IO.IOException] { $writerRejected = $true }
    if (-not $writerRejected) { throw 'Release-proof self-test snapshot allowed a tampering writer.' }
    $unsignedPath = Join-Path $tempRoot 'unsigned.exe'
    Write-PackagingSelfTestPe -Path $unsignedPath
    $unsigned = Open-LockedInput -Path $unsignedPath -Label 'Unsigned self-test artifact'
    try {
      [void](Get-AuthenticodeProof -Evidence $unsigned -ExpectedStatus NotSigned -Label 'Unsigned self-test artifact')
      $signedDecisionRejected = $false
      try {
        [void](Get-AuthenticodeProof -Evidence $unsigned -ExpectedStatus Valid -SignerSubject $signature.SignerCertificate.Subject -SignerThumbprint $signature.SignerCertificate.Thumbprint -Label 'Unsigned self-test artifact')
      } catch { $signedDecisionRejected = $true }
      if (-not $signedDecisionRejected) { throw 'Release-proof self-test accepted unsigned bytes as Signed.' }
    } finally {
      $unsigned.Stream.Dispose()
    }
  } finally {
    if ($null -ne $snapshot) { $snapshot.Stream.Dispose() }
    $signed.Stream.Dispose()
    $resolved = [System.IO.Path]::GetFullPath($tempRoot)
    if ([System.IO.Path]::GetDirectoryName($resolved).Equals($tempParent, [System.StringComparison]::OrdinalIgnoreCase) -and
        [System.IO.Path]::GetFileName($resolved).StartsWith('codehangar-release-proof-selftest-', [System.StringComparison]::Ordinal)) {
      [System.IO.Directory]::Delete($resolved, $true)
    } else {
      throw "Refusing unsafe release-proof self-test cleanup: $resolved"
    }
  }
  Write-Host 'Release artifact proof locked-copy, RFC3161 imprint, lifecycle substitution and tamper self-test passed.' -ForegroundColor Green
  exit 0
}

$modeCount = @(@($Create, $ValidateOnly) | Where-Object { [bool]$_ }).Count
if ($modeCount -ne 1) { throw 'Choose exactly one mode: -Create or -ValidateOnly.' }
if ([string]::IsNullOrWhiteSpace($EvidenceDir)) { throw 'EvidenceDir is required.' }

if ($Create) {
  $root = Resolve-NewProofDirectory -Path $EvidenceDir
  Create-ReleaseProof -Root $root
} else {
  if ([string]::IsNullOrWhiteSpace($ExpectedReportSha256)) {
    throw '-ValidateOnly requires the independently recorded -ExpectedReportSha256.'
  }
  $root = Resolve-ExistingProofDirectory -Path $EvidenceDir
  $validated = Validate-ReleaseProof -Root $root -ExpectedHash $ExpectedReportSha256
  Write-Host "Release artifact proof passed: $($validated.Path)" -ForegroundColor Green
  Write-Host "RELEASE-ARTIFACT-PROOF SHA-256: $($validated.Sha256)" -ForegroundColor Green
}
