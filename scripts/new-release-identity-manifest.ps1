[CmdletBinding()]
param(
  [string]$ParentPath,
  [string]$ParentInstallFileName = "code-hangar-desktop.exe",
  [string]$HelperPath,
  [string]$HelperInstallFileName = "code-hangar-elevated.exe",
  [string]$PrivateKeyPath,
  [string]$ExpectedPublicBlobHex,
  [string]$OutputPath,
  [ValidateSet("Local", "Connector")][string]$Edition,
  [string]$SigningReceiptSha256,
  [string]$ReleaseId,
  [switch]$SelfTest
)

# Creates only the detached, post-Authenticode release identity manifest used
# by the one-shot elevated helper. It never signs executables and never reaches
# the network. The RSA private key remains an owner-supplied, offline input and
# is opened with FileShare.None for the shortest practical interval.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$script:ManifestSchema = "codehangar/release-identity/1"
$script:ReceiptBoundReleaseIdSchema = "codehangar/receipt-bound-release-id/1"

function Convert-BytesToHex {
  param([Parameter(Mandatory = $true)][byte[]]$Bytes)
  return ([System.BitConverter]::ToString($Bytes)).Replace("-", "")
}

function Assert-SafeReleaseFileName {
  param(
    [Parameter(Mandatory = $true)][string]$Value,
    [Parameter(Mandatory = $true)][string]$Label
  )
  if ($Value.Length -lt 1 -or $Value.Length -gt 128 -or
      $Value -cnotmatch '^[A-Za-z0-9._-]+$' -or
      $Value -in @('.', '..') -or
      [System.IO.Path]::GetFileName($Value) -cne $Value) {
    throw "$Label must be a bounded ASCII leaf name containing only letters, digits, dot, dash or underscore."
  }
}

function Assert-LocalNonReparsePath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequireExistingLeaf
  )
  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) {
    throw "$Label must be a fully qualified path."
  }
  $full = [System.IO.Path]::GetFullPath($Path)
  $root = [System.IO.Path]::GetPathRoot($full)
  if ([string]::IsNullOrWhiteSpace($root) -or $root.StartsWith('\\')) {
    throw "$Label must be on a local Windows volume, not a UNC/network path."
  }
  $drive = [System.IO.DriveInfo]::new($root)
  if (-not $drive.IsReady -or $drive.DriveType -notin @(
      [System.IO.DriveType]::Fixed,
      [System.IO.DriveType]::Removable
    )) {
    throw "$Label must be on a ready fixed or removable local volume."
  }
  $current = $root
  $relative = $full.Substring($root.Length)
  foreach ($segment in $relative.Split(
      [char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar),
      [System.StringSplitOptions]::RemoveEmptyEntries
    )) {
    $current = Join-Path $current $segment
    if (-not (Test-Path -LiteralPath $current)) { break }
    $item = Get-Item -LiteralPath $current -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "$Label path contains a reparse point: $current"
    }
  }
  if ($RequireExistingLeaf -and -not (Test-Path -LiteralPath $full -PathType Leaf)) {
    throw "$Label does not identify an existing regular file: $full"
  }
  return $full
}

function Get-LockedSha256 {
  param([Parameter(Mandatory = $true)][System.IO.FileStream]$Stream)
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $Stream.Position = 0
    $digest = $sha.ComputeHash($Stream)
    $Stream.Position = 0
    return (Convert-BytesToHex -Bytes $digest).ToLowerInvariant()
  } finally {
    $sha.Dispose()
  }
}

function Convert-RsaParametersToBCryptPublicBlob {
  param([Parameter(Mandatory = $true)][System.Security.Cryptography.RSAParameters]$Parameters)
  $exponent = [byte[]]$Parameters.Exponent
  $modulus = [byte[]]$Parameters.Modulus
  if ($null -eq $exponent -or $null -eq $modulus -or $exponent.Length -lt 3 -or
      $exponent.Length -gt 8 -or $modulus.Length -lt 384 -or $modulus.Length -gt 1024) {
    throw "Release-root RSA public parameters are outside the audited 3072..8192-bit bounds."
  }
  $bits = $modulus.Length * 8
  $header = [byte[]]::new(24)
  [System.BitConverter]::GetBytes([uint32]0x31415352).CopyTo($header, 0) # BCRYPT_RSAPUBLIC_MAGIC / RSA1
  [System.BitConverter]::GetBytes([uint32]$bits).CopyTo($header, 4)
  [System.BitConverter]::GetBytes([uint32]$exponent.Length).CopyTo($header, 8)
  [System.BitConverter]::GetBytes([uint32]$modulus.Length).CopyTo($header, 12)
  $blob = [byte[]]::new($header.Length + $exponent.Length + $modulus.Length)
  $header.CopyTo($blob, 0)
  $exponent.CopyTo($blob, $header.Length)
  $modulus.CopyTo($blob, $header.Length + $exponent.Length)
  return $blob
}

function Get-ReceiptBoundReleaseId {
  param(
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$InputEdition,
    [Parameter(Mandatory = $true)][string]$InputSigningReceiptSha256
  )

  if ($InputSigningReceiptSha256 -cnotmatch '^[0-9A-Fa-f]{64}$') {
    throw "Signing receipt SHA-256 must be exactly 64 hexadecimal characters."
  }
  $canonicalEdition = if ($InputEdition -ieq 'Local') { 'Local' } else { 'Connector' }
  $payload = "schema=$script:ReceiptBoundReleaseIdSchema`nedition=$canonicalEdition`nsigning_receipt_sha256=$($InputSigningReceiptSha256.ToLowerInvariant())`n"
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    return (Convert-BytesToHex -Bytes $sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($payload))).ToLowerInvariant()
  } finally {
    $sha.Dispose()
  }
}

function New-ManifestSigningPayload {
  param(
    [Parameter(Mandatory = $true)][string]$ManifestReleaseId,
    [Parameter(Mandatory = $true)][string]$ParentFile,
    [Parameter(Mandatory = $true)][string]$ParentSha256,
    [Parameter(Mandatory = $true)][string]$HelperFile,
    [Parameter(Mandatory = $true)][string]$HelperSha256
  )
  $text = "schema=$script:ManifestSchema`nrelease_id=$ManifestReleaseId`nparent_file=$ParentFile`nparent_sha256=$ParentSha256`nhelper_file=$HelperFile`nhelper_sha256=$HelperSha256`n"
  return [System.Text.Encoding]::UTF8.GetBytes($text)
}

function New-CodeHangarReleaseIdentityManifest {
  param(
    [Parameter(Mandatory = $true)][string]$InputParentPath,
    [Parameter(Mandatory = $true)][string]$InputParentFileName,
    [Parameter(Mandatory = $true)][string]$InputHelperPath,
    [Parameter(Mandatory = $true)][string]$InputHelperFileName,
    [Parameter(Mandatory = $true)][string]$InputPrivateKeyPath,
    [Parameter(Mandatory = $true)][string]$InputExpectedPublicBlobHex,
    [Parameter(Mandatory = $true)][string]$InputOutputPath,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$InputEdition,
    [Parameter(Mandatory = $true)][string]$InputSigningReceiptSha256,
    [string]$InputReleaseId
  )
  Assert-SafeReleaseFileName -Value $InputParentFileName -Label "Parent install file name"
  Assert-SafeReleaseFileName -Value $InputHelperFileName -Label "Helper install file name"
  if ($InputParentFileName.Equals($InputHelperFileName, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Parent and helper install file names must be distinct."
  }
  if ($InputExpectedPublicBlobHex -cnotmatch '^[0-9A-Fa-f]+$' -or
      $InputExpectedPublicBlobHex.Length -lt 822 -or
      $InputExpectedPublicBlobHex.Length -gt 2112 -or
      ($InputExpectedPublicBlobHex.Length % 2) -ne 0) {
    throw "Expected release-root BCRYPT public blob must be bounded hexadecimal."
  }

  $parentFull = Assert-LocalNonReparsePath -Path $InputParentPath -Label "Signed parent" -RequireExistingLeaf
  $helperFull = Assert-LocalNonReparsePath -Path $InputHelperPath -Label "Signed helper" -RequireExistingLeaf
  $keyFull = Assert-LocalNonReparsePath -Path $InputPrivateKeyPath -Label "Offline release-root private key" -RequireExistingLeaf
  $outputFull = Assert-LocalNonReparsePath -Path $InputOutputPath -Label "Release manifest output"
  if ($parentFull.Equals($helperFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Signed parent and helper paths must be distinct."
  }
  if (Test-Path -LiteralPath $outputFull) {
    throw "Release manifest output already exists; refusing to overwrite it: $outputFull"
  }
  $outputParent = Split-Path -Parent $outputFull
  if (-not (Test-Path -LiteralPath $outputParent -PathType Container)) {
    throw "Release manifest output directory does not exist: $outputParent"
  }

  $parentStream = $null
  $helperStream = $null
  $keyStream = $null
  $rsa = $null
  try {
    $parentStream = [System.IO.FileStream]::new($parentFull, 'Open', 'Read', 'Read')
    $helperStream = [System.IO.FileStream]::new($helperFull, 'Open', 'Read', 'Read')
    if ($parentStream.Length -le 0 -or $helperStream.Length -le 0) {
      throw "Signed parent/helper inputs must be non-empty."
    }
    $parentSha256 = Get-LockedSha256 -Stream $parentStream
    $helperSha256 = Get-LockedSha256 -Stream $helperStream
    if ($parentSha256 -ceq $helperSha256) {
      throw "Signed parent and helper unexpectedly have identical bytes."
    }

    $keyStream = [System.IO.FileStream]::new($keyFull, 'Open', 'Read', 'None')
    if ($keyStream.Length -le 0 -or $keyStream.Length -gt 64KB) {
      throw "Offline release-root private key has invalid bounds."
    }
    $keyBytes = [byte[]]::new([int]$keyStream.Length)
    $read = $keyStream.Read($keyBytes, 0, $keyBytes.Length)
    if ($read -ne $keyBytes.Length) { throw "Offline release-root private key read was truncated." }
    $pem = [System.Text.Encoding]::UTF8.GetString($keyBytes)
    [System.Array]::Clear($keyBytes, 0, $keyBytes.Length)
    $rsa = [System.Security.Cryptography.RSA]::Create()
    $rsa.ImportFromPem($pem)
    $pem = $null
    if ($rsa.KeySize -lt 3072 -or $rsa.KeySize -gt 8192 -or ($rsa.KeySize % 8) -ne 0) {
      throw "Offline release-root RSA key must contain 3072..8192 bits."
    }
    $publicBlob = Convert-RsaParametersToBCryptPublicBlob -Parameters ($rsa.ExportParameters($false))
    $actualPublicBlobHex = Convert-BytesToHex -Bytes $publicBlob
    if ($actualPublicBlobHex -cne $InputExpectedPublicBlobHex.ToUpperInvariant()) {
      throw "Offline release-root private key does not match the public blob embedded in the build."
    }

    $manifestReleaseId = Get-ReceiptBoundReleaseId `
      -InputEdition $InputEdition `
      -InputSigningReceiptSha256 $InputSigningReceiptSha256
    if (-not [string]::IsNullOrWhiteSpace($InputReleaseId) -and
        $InputReleaseId.ToLowerInvariant() -cne $manifestReleaseId) {
      throw "Release id is receipt-bound; omit -ReleaseId or supply the exact derived value for $InputEdition and the signing receipt SHA-256."
    }
    if ($manifestReleaseId -cnotmatch '^[0-9a-f]{64}$') {
      throw "Release id must be exactly 64 lowercase hexadecimal characters."
    }
    $payload = New-ManifestSigningPayload `
      -ManifestReleaseId $manifestReleaseId `
      -ParentFile $InputParentFileName `
      -ParentSha256 $parentSha256 `
      -HelperFile $InputHelperFileName `
      -HelperSha256 $helperSha256
    $signature = $rsa.SignData(
      $payload,
      [System.Security.Cryptography.HashAlgorithmName]::SHA256,
      [System.Security.Cryptography.RSASignaturePadding]::Pss
    )
    if ($signature.Length -ne ($rsa.KeySize / 8)) {
      throw "Release manifest signature length does not match the release-root modulus."
    }
    $manifest = [ordered]@{
      schema = $script:ManifestSchema
      release_id = $manifestReleaseId
      parent = [ordered]@{ file_name = $InputParentFileName; sha256 = $parentSha256 }
      helper = [ordered]@{ file_name = $InputHelperFileName; sha256 = $helperSha256 }
      signature_rsa_pss_sha256 = (Convert-BytesToHex -Bytes $signature).ToLowerInvariant()
    }
    $json = $manifest | ConvertTo-Json -Depth 5 -Compress
    $output = [System.IO.FileStream]::new($outputFull, 'CreateNew', 'Write', 'None')
    try {
      $jsonBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
      $output.Write($jsonBytes, 0, $jsonBytes.Length)
      $output.Flush($true)
    } finally {
      $output.Dispose()
    }
    return [pscustomobject]@{
      Path = $outputFull
      ReleaseId = $manifestReleaseId
      ParentSha256 = $parentSha256
      HelperSha256 = $helperSha256
      PublicBlobHex = $actualPublicBlobHex
    }
  } finally {
    if ($null -ne $rsa) { $rsa.Dispose() }
    if ($null -ne $keyStream) { $keyStream.Dispose() }
    if ($null -ne $helperStream) { $helperStream.Dispose() }
    if ($null -ne $parentStream) { $parentStream.Dispose() }
  }
}

if ($SelfTest) {
  if (@($ParentPath, $HelperPath, $PrivateKeyPath, $ExpectedPublicBlobHex, $OutputPath, $Edition, $SigningReceiptSha256, $ReleaseId) |
      Where-Object { -not [string]::IsNullOrWhiteSpace($_) }) {
    throw "-SelfTest does not accept release input parameters."
  }
  $rsa = [System.Security.Cryptography.RSA]::Create(3072)
  try {
    $parameters = $rsa.ExportParameters($false)
    $blob = Convert-RsaParametersToBCryptPublicBlob -Parameters $parameters
    if ($blob.Length -ne (24 + $parameters.Exponent.Length + $parameters.Modulus.Length)) {
      throw "BCRYPT public-blob self-test length mismatch."
    }
    $payload = New-ManifestSigningPayload `
      -ManifestReleaseId ('12' * 32) `
      -ParentFile 'code-hangar-desktop.exe' `
      -ParentSha256 ('34' * 32) `
      -HelperFile 'code-hangar-elevated.exe' `
      -HelperSha256 ('56' * 32)
    $signature = $rsa.SignData(
      $payload,
      [System.Security.Cryptography.HashAlgorithmName]::SHA256,
      [System.Security.Cryptography.RSASignaturePadding]::Pss
    )
    if (-not $rsa.VerifyData(
        $payload,
        $signature,
        [System.Security.Cryptography.HashAlgorithmName]::SHA256,
        [System.Security.Cryptography.RSASignaturePadding]::Pss
      )) {
      throw "RSA-PSS manifest self-test verification failed."
    }
    $localReleaseId = Get-ReceiptBoundReleaseId -InputEdition Local -InputSigningReceiptSha256 ('12' * 32)
    $connectorReleaseId = Get-ReceiptBoundReleaseId -InputEdition Connector -InputSigningReceiptSha256 ('12' * 32)
    $caseNormalizedConnectorReleaseId = Get-ReceiptBoundReleaseId -InputEdition connector -InputSigningReceiptSha256 ('12' * 32)
    if ($localReleaseId -cnotmatch '^[0-9a-f]{64}$' -or
        $localReleaseId -ceq $connectorReleaseId -or
        $connectorReleaseId -cne $caseNormalizedConnectorReleaseId) {
      throw "Receipt-bound manifest release-id self-test did not separate Local and Connector."
    }
    Write-Host "Release identity manifest deterministic payload/RSA-PSS self-test passed."
  } finally {
    $rsa.Dispose()
  }
  return
}

foreach ($required in @{
    ParentPath = $ParentPath
    HelperPath = $HelperPath
    PrivateKeyPath = $PrivateKeyPath
    ExpectedPublicBlobHex = $ExpectedPublicBlobHex
    OutputPath = $OutputPath
    Edition = $Edition
    SigningReceiptSha256 = $SigningReceiptSha256
  }.GetEnumerator()) {
  if ([string]::IsNullOrWhiteSpace([string]$required.Value)) {
    throw "-$($required.Key) is required."
  }
}

New-CodeHangarReleaseIdentityManifest `
  -InputParentPath $ParentPath `
  -InputParentFileName $ParentInstallFileName `
  -InputHelperPath $HelperPath `
  -InputHelperFileName $HelperInstallFileName `
  -InputPrivateKeyPath $PrivateKeyPath `
  -InputExpectedPublicBlobHex $ExpectedPublicBlobHex `
  -InputOutputPath $OutputPath `
  -InputEdition $Edition `
  -InputSigningReceiptSha256 $SigningReceiptSha256 `
  -InputReleaseId $ReleaseId
