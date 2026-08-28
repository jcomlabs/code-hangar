Set-StrictMode -Version Latest
$script:CodeHangarPackagingScriptsRoot = $PSScriptRoot

function Assert-PackagingEnvironmentOverrides {
  param([System.Collections.IDictionary]$Environment)

  if ($null -eq $Environment) {
    $Environment = [System.Environment]::GetEnvironmentVariables()
  }
  foreach ($name in @(
      "NODE_PATH",
      "NODE_OPTIONS",
      "NAPI_RS_NATIVE_LIBRARY_PATH",
      "NAPI_RS_FORCE_WASI",
      "NAPI_RS_ENFORCE_VERSION_CHECK",
      "RUSTFLAGS",
      "RUSTDOCFLAGS",
      "RUSTC",
      "RUSTC_WRAPPER",
      "RUSTC_WORKSPACE_WRAPPER",
      "CARGO_ENCODED_RUSTFLAGS"
    )) {
    $value = $Environment[$name]
    if ($null -ne $value -and -not [string]::IsNullOrWhiteSpace([string]$value)) {
      throw "$name must be empty for worktree-bound packaging."
    }
  }
  foreach ($entry in $Environment.GetEnumerator()) {
    $name = [string]$entry.Key
    $value = [string]$entry.Value
    if ($name.StartsWith("TAURI_", [System.StringComparison]::OrdinalIgnoreCase) -and
        -not [string]::IsNullOrWhiteSpace($value)) {
      throw "$name is a build-affecting TAURI_* override and must be empty for HOLD packaging."
    }
    if (($name.StartsWith("CARGO_PROFILE_", [System.StringComparison]::OrdinalIgnoreCase) -or
         $name -match '^CARGO_TARGET_.+_(RUSTFLAGS|LINKER|RUNNER)$') -and
        -not [string]::IsNullOrWhiteSpace($value)) {
      throw "$name is a build-affecting Cargo override and must be empty for HOLD packaging."
    }
  }
}

function Assert-FixedLocalPathChain {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequireExisting
  )

  $fullPath = [System.IO.Path]::GetFullPath($Path)
  if ($fullPath.StartsWith("\\", [System.StringComparison]::Ordinal)) {
    throw "$Label must not be a UNC/network path: $fullPath"
  }
  $root = [System.IO.Path]::GetPathRoot($fullPath)
  if ([string]::IsNullOrWhiteSpace($root)) {
    throw "$Label has no local drive root: $fullPath"
  }
  $drive = [System.IO.DriveInfo]::new($root)
  if (-not $drive.IsReady -or $drive.DriveType -ne [System.IO.DriveType]::Fixed) {
    throw "$Label must be on a ready fixed local drive, not $($drive.DriveType): $fullPath"
  }

  $current = $root
  $relative = $fullPath.Substring($root.Length)
  if ($relative.Contains(':')) {
    throw "$Label must not address an alternate data stream: $fullPath"
  }
  $segments = $relative.Split(
    [char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar),
    [System.StringSplitOptions]::RemoveEmptyEntries
  )
  foreach ($segment in $segments) {
    $current = Join-Path $current $segment
    if (-not (Test-Path -LiteralPath $current)) {
      if ($RequireExisting) {
        throw "$Label path component does not exist: $current"
      }
      break
    }
    $item = Get-Item -LiteralPath $current -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "$Label path contains a reparse point: $current"
    }
  }
}

function Enter-WorktreePackagingLock {
  param([Parameter(Mandatory = $true)][string]$RepoRoot)

  Assert-FixedLocalPathChain -Path $RepoRoot -Label "The packaging worktree" -RequireExisting
  $lockDirectory = Join-Path $RepoRoot ".local"
  if (-not (Test-Path -LiteralPath $lockDirectory)) {
    [void][System.IO.Directory]::CreateDirectory($lockDirectory)
  }
  Assert-FixedLocalPathChain -Path $lockDirectory -Label "The packaging lock directory" -RequireExisting
  $lockPath = Join-Path $lockDirectory "packaging.lock"
  if (Test-Path -LiteralPath $lockPath) {
    $lockItem = Get-Item -LiteralPath $lockPath -Force
    if ($lockItem.PSIsContainer -or (($lockItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
      throw "The packaging lock must be a regular file: $lockPath"
    }
  }

  try {
    return [System.IO.FileStream]::new(
      $lockPath,
      [System.IO.FileMode]::OpenOrCreate,
      [System.IO.FileAccess]::ReadWrite,
      [System.IO.FileShare]::None
    )
  } catch [System.IO.IOException] {
    throw "Another Code Hangar packaging/preflight process already holds the worktree lock: $lockPath"
  }
}

function Get-EditionInstallerRegex {
  param([Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition)
  if ($Edition -eq "Local") {
    return '^Code Hangar_.+_x64-setup\.exe$'
  }
  return '^Code Hangar AI Connector_.+_x64-setup\.exe$'
}

function Get-EditionInstallerItems {
  param(
    [Parameter(Mandatory = $true)][string]$NsisDir,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition
  )
  if (-not (Test-Path -LiteralPath $NsisDir)) { return @() }
  $directory = Get-Item -LiteralPath $NsisDir -Force
  if (-not $directory.PSIsContainer -or (($directory.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "The NSIS output path must be a real directory: $NsisDir"
  }
  $regex = Get-EditionInstallerRegex -Edition $Edition
  return @(
    Get-ChildItem -LiteralPath $NsisDir -Force -ErrorAction Stop |
      Where-Object { $_.Name -match $regex }
  )
}

function Remove-EditionRawInstallers {
  param(
    [Parameter(Mandatory = $true)][string]$NsisDir,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition
  )
  foreach ($item in @(Get-EditionInstallerItems -NsisDir $NsisDir -Edition $Edition)) {
    if ($item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
      throw "Refusing to remove a non-regular/reparse raw $Edition installer candidate: $($item.FullName)"
    }
    Remove-Item -LiteralPath $item.FullName -Force -ErrorAction Stop
  }
  $remaining = @(Get-EditionInstallerItems -NsisDir $NsisDir -Edition $Edition)
  if ($remaining.Count -ne 0) {
    throw "Failed to clear prior raw $Edition installers: $($remaining.FullName -join '; ')"
  }
}

function Test-BasicPortableExecutable {
  param([Parameter(Mandatory = $true)][string]$Path)
  $stream = $null
  $reader = $null
  try {
    $stream = [System.IO.FileStream]::new(
      $Path,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    if ($stream.Length -lt 256) { return $false }
    $reader = [System.IO.BinaryReader]::new($stream)
    if ($reader.ReadUInt16() -ne 0x5A4D) { return $false }
    $stream.Position = 0x3C
    $peOffset = $reader.ReadUInt32()
    if ($peOffset -lt 0x40 -or $peOffset -gt ($stream.Length - 4)) { return $false }
    $stream.Position = $peOffset
    return $reader.ReadUInt32() -eq 0x00004550
  } catch {
    return $false
  } finally {
    if ($null -ne $reader) { $reader.Dispose() }
    elseif ($null -ne $stream) { $stream.Dispose() }
  }
}

function Get-ValidatedFreshInstaller {
  param(
    [Parameter(Mandatory = $true)][string]$NsisDir,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$ExpectedFileName,
    [Parameter(Mandatory = $true)][datetime]$StartedAtUtc
  )

  if ([System.IO.Path]::GetFileName($ExpectedFileName) -cne $ExpectedFileName -or
      $ExpectedFileName -notmatch (Get-EditionInstallerRegex -Edition $Edition)) {
    throw "The expected $Edition installer name is not a safe edition filename: $ExpectedFileName"
  }

  $candidates = @(Get-EditionInstallerItems -NsisDir $NsisDir -Edition $Edition)
  if ($candidates.Count -ne 1) {
    $found = if ($candidates.Count -eq 0) { "none" } else { $candidates.FullName -join "; " }
    throw "Expected exactly one newly created raw $Edition installer; found $found."
  }
  $installer = $candidates[0]
  if ($installer.Name -cne $ExpectedFileName) {
    throw "The $Edition installer filename is not the exact version-bound name '$ExpectedFileName': $($installer.Name)"
  }
  if ($installer.PSIsContainer -or (($installer.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "The $Edition installer is not a regular non-reparse file: $($installer.FullName)"
  }
  if ($installer.Length -le 0) {
    throw "The $Edition installer is empty: $($installer.FullName)"
  }
  if ($installer.CreationTimeUtc -le $StartedAtUtc) {
    throw "The $Edition installer was not newly created after this run started: $($installer.FullName)"
  }
  if ($installer.LastWriteTimeUtc -le $StartedAtUtc) {
    throw "The $Edition installer was not written after this run started: $($installer.FullName)"
  }
  if (-not (Test-BasicPortableExecutable -Path $installer.FullName)) {
    throw "The $Edition installer does not have a valid basic PE/MZ structure: $($installer.FullName)"
  }
  $lengthBeforeHash = $installer.Length
  $creationBeforeHash = $installer.CreationTimeUtc
  $writeBeforeHash = $installer.LastWriteTimeUtc
  $sha256 = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  $afterHash = Get-Item -LiteralPath $installer.FullName -Force
  if ($afterHash.PSIsContainer -or (($afterHash.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) -or
      $afterHash.Length -ne $lengthBeforeHash -or $afterHash.CreationTimeUtc -ne $creationBeforeHash -or
      $afterHash.LastWriteTimeUtc -ne $writeBeforeHash) {
    throw "The $Edition installer changed while its release hash was being calculated: $($installer.FullName)"
  }
  return [pscustomobject]@{
    Path = $installer.FullName
    Sha256 = $sha256
  }
}

function Assert-RawUnsignedHoldInstaller {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$ExpectedSha256
  )

  $before = Get-StableReleaseArtifactEvidence -Path $Path -Label "The raw NSIS HOLD candidate" -RequirePe
  if ($ExpectedSha256 -cnotmatch '^[0-9A-Fa-f]{64}$' -or
      $before.Sha256 -cne $ExpectedSha256.ToLowerInvariant()) {
    throw "The raw NSIS HOLD candidate no longer matches its freshly created SHA-256."
  }
  $signature = Get-AuthenticodeSignature -LiteralPath $before.Path -ErrorAction Stop
  if ([string]$signature.Status -cne "NotSigned" -or $null -ne $signature.SignerCertificate) {
    throw "tauri bundle --no-sign unexpectedly produced an Authenticode-bearing setup; refusing to mislabel or advance this raw HOLD candidate (status: $($signature.Status))."
  }
  $after = Get-StableReleaseArtifactEvidence -Path $before.Path -Label "The raw unsigned NSIS HOLD candidate" -RequirePe
  if ($after.Length -ne $before.Length -or $after.Sha256 -cne $before.Sha256) {
    throw "The raw NSIS HOLD candidate changed during its unsigned-state check."
  }
  return $after
}

function Remove-StagedConnectorSidecars {
  param([Parameter(Mandatory = $true)][string]$SidecarDir)

  if (-not (Test-Path -LiteralPath $SidecarDir)) { return }
  $directory = Get-Item -LiteralPath $SidecarDir -Force
  if (-not $directory.PSIsContainer -or (($directory.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "The staged-sidecar path must be a real directory: $SidecarDir"
  }
  foreach ($item in @(Get-ChildItem -LiteralPath $SidecarDir -Filter "code-hangar-mcp-*.exe" -Force -ErrorAction Stop)) {
    if ($item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
      throw "Refusing to remove a non-regular/reparse staged sidecar: $($item.FullName)"
    }
    Remove-Item -LiteralPath $item.FullName -Force -ErrorAction Stop
  }
}

function Read-PinnedWebView2Manifest {
  param([Parameter(Mandatory = $true)][string]$ManifestPath)

  Assert-FixedLocalPathChain -Path $ManifestPath -Label "The pinned WebView2 manifest" -RequireExisting
  $manifestItem = Get-Item -LiteralPath $ManifestPath -Force
  if ($manifestItem.PSIsContainer -or (($manifestItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "The pinned WebView2 manifest must be a regular non-reparse file: $ManifestPath"
  }
  $manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
  $required = @(
    "schemaVersion", "filename", "length", "sha256", "fileVersion", "peMachine",
    "signerSubject", "signerThumbprint", "signerIssuer", "timestampThumbprint"
  )
  $actual = @($manifest.PSObject.Properties.Name)
  $missing = @($required | Where-Object { $_ -notin $actual })
  $extra = @($actual | Where-Object { $_ -notin $required })
  if ($missing.Count -ne 0 -or $extra.Count -ne 0) {
    throw "Pinned WebView2 manifest fields are not exact (missing: $($missing -join ', '); extra: $($extra -join ', '))."
  }
  if ($manifest.schemaVersion -ne 1) { throw "Unsupported pinned WebView2 manifest schemaVersion." }
  if ($manifest.filename -cne "MicrosoftEdgeWebView2RuntimeInstallerX64.exe") {
    throw "Pinned WebView2 manifest has an unsafe/unexpected filename."
  }
  if ([long]$manifest.length -le 0) { throw "Pinned WebView2 manifest length must be positive." }
  if ([string]$manifest.sha256 -cnotmatch "^[0-9A-F]{64}$") {
    throw "Pinned WebView2 manifest SHA-256 must be 64 uppercase hexadecimal characters."
  }
  if ([string]$manifest.fileVersion -cnotmatch "^[0-9]+(\.[0-9]+){3}$") {
    throw "Pinned WebView2 manifest fileVersion is invalid."
  }
  if ([string]$manifest.peMachine -cne "014C") {
    throw "Pinned WebView2 PE Machine must be the audited 014C bootstrap executable."
  }
  foreach ($thumbprintName in @("signerThumbprint", "timestampThumbprint")) {
    if ([string]$manifest.$thumbprintName -cnotmatch "^[0-9A-F]{40}$") {
      throw "Pinned WebView2 manifest $thumbprintName must be 40 uppercase hexadecimal characters."
    }
  }
  return $manifest
}

function Get-LockedStreamSha256 {
  param([Parameter(Mandatory = $true)][System.IO.FileStream]$Stream)

  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $Stream.Position = 0
    $bytes = $sha.ComputeHash($Stream)
    $Stream.Position = 0
    return ([System.BitConverter]::ToString($bytes)).Replace("-", "")
  } finally {
    $sha.Dispose()
  }
}

function Add-AuthenticodeImageHashRange {
  param(
    [Parameter(Mandatory = $true)][System.IO.Stream]$Stream,
    [Parameter(Mandatory = $true)][System.Security.Cryptography.HashAlgorithm]$Hash,
    [Parameter(Mandatory = $true)][long]$Offset,
    [Parameter(Mandatory = $true)][long]$Length,
    [Parameter(Mandatory = $true)][string]$Label
  )

  if ($Offset -lt 0 -or $Length -lt 0 -or $Offset -gt ($Stream.Length - $Length)) {
    throw "$Label has an invalid Authenticode image-hash range."
  }
  if ($Length -eq 0) { return }
  $Stream.Position = $Offset
  $buffer = [byte[]]::new([Math]::Min(1048576, [int]$Length))
  $remaining = $Length
  while ($remaining -gt 0) {
    $wanted = [int][Math]::Min([long]$buffer.Length, $remaining)
    $read = $Stream.Read($buffer, 0, $wanted)
    if ($read -ne $wanted) { throw "$Label was truncated while computing its Authenticode image digest." }
    [void]$Hash.TransformBlock($buffer, 0, $read, $buffer, 0)
    $remaining -= $read
  }
}

function Get-CodeHangarAuthenticodeImageSha256FromLockedStream {
  param(
    [Parameter(Mandatory = $true)][System.IO.FileStream]$Stream,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $reader = $null
  $sha = $null
  try {
    if (-not $Stream.CanRead -or $Stream.Length -lt 512) { throw "$Label is too small for Authenticode image hashing." }
    $reader = [System.IO.BinaryReader]::new($Stream, [System.Text.Encoding]::UTF8, $true)
    $Stream.Position = 0
    if ($reader.ReadUInt16() -ne 0x5A4D) { throw "$Label has no MZ header." }
    $Stream.Position = 0x3C
    $peOffset = [long]$reader.ReadUInt32()
    if ($peOffset -lt 0x40 -or $peOffset -gt ($Stream.Length - 24)) {
      throw "$Label has an invalid PE header offset."
    }
    $Stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) { throw "$Label has no PE signature." }
    $Stream.Position = $peOffset + 20
    $optionalHeaderLength = [long]$reader.ReadUInt16()
    $optionalHeaderOffset = $peOffset + 24
    if ($optionalHeaderLength -lt 112 -or $optionalHeaderOffset -gt ($Stream.Length - $optionalHeaderLength)) {
      throw "$Label has an invalid PE optional-header length."
    }
    $Stream.Position = $optionalHeaderOffset
    $optionalMagic = $reader.ReadUInt16()
    if ($optionalMagic -eq 0x10B) {
      $numberOfRvaAndSizesOffset = $optionalHeaderOffset + 92
      $dataDirectoryOffset = $optionalHeaderOffset + 96
    } elseif ($optionalMagic -eq 0x20B) {
      $numberOfRvaAndSizesOffset = $optionalHeaderOffset + 108
      $dataDirectoryOffset = $optionalHeaderOffset + 112
    } else {
      throw "$Label has an unsupported PE optional-header format."
    }
    $checksumOffset = $optionalHeaderOffset + 64
    $certificateDirectoryOffset = $dataDirectoryOffset + (8 * 4)
    $optionalHeaderEnd = $optionalHeaderOffset + $optionalHeaderLength
    if ($checksumOffset -lt $optionalHeaderOffset -or $checksumOffset + 4 -gt $optionalHeaderEnd -or
        $numberOfRvaAndSizesOffset + 4 -gt $optionalHeaderEnd -or
        $certificateDirectoryOffset + 8 -gt $optionalHeaderEnd) {
      throw "$Label has no complete checksum/security-directory fields for Authenticode image hashing."
    }
    $Stream.Position = $numberOfRvaAndSizesOffset
    if ($reader.ReadUInt32() -lt 5) { throw "$Label has no PE security data-directory entry." }
    $Stream.Position = $certificateDirectoryOffset
    $certificateOffset = [long]$reader.ReadUInt32()
    $certificateLength = [long]$reader.ReadUInt32()
    if (($certificateOffset -eq 0) -xor ($certificateLength -eq 0)) {
      throw "$Label has an invalid partial PE certificate-table reference."
    }
    if ($certificateOffset -ne 0) {
      if ($certificateOffset -lt ($certificateDirectoryOffset + 8) -or $certificateLength -lt 8 -or
           $certificateOffset -gt ($Stream.Length - $certificateLength)) {
        throw "$Label has an invalid PE certificate-table range."
      }
    }

    $sha = [System.Security.Cryptography.SHA256]::Create()
    Add-AuthenticodeImageHashRange -Stream $Stream -Hash $sha -Offset 0 -Length $checksumOffset -Label $Label
    Add-AuthenticodeImageHashRange `
      -Stream $Stream `
      -Hash $sha `
      -Offset ($checksumOffset + 4) `
      -Length ($certificateDirectoryOffset - ($checksumOffset + 4)) `
      -Label $Label
    $afterDirectoryOffset = $certificateDirectoryOffset + 8
    if ($certificateOffset -eq 0) {
      Add-AuthenticodeImageHashRange `
        -Stream $Stream `
        -Hash $sha `
        -Offset $afterDirectoryOffset `
        -Length ($Stream.Length - $afterDirectoryOffset) `
        -Label $Label
    } else {
      Add-AuthenticodeImageHashRange `
        -Stream $Stream `
        -Hash $sha `
        -Offset $afterDirectoryOffset `
        -Length ($certificateOffset - $afterDirectoryOffset) `
        -Label $Label
      $afterCertificateOffset = $certificateOffset + $certificateLength
      Add-AuthenticodeImageHashRange `
        -Stream $Stream `
        -Hash $sha `
        -Offset $afterCertificateOffset `
        -Length ($Stream.Length - $afterCertificateOffset) `
        -Label $Label
    }
    [void]$sha.TransformFinalBlock([byte[]]::new(0), 0, 0)
    return (([System.BitConverter]::ToString($sha.Hash)).Replace('-', '')).ToLowerInvariant()
  } finally {
    if ($null -ne $sha) { $sha.Dispose() }
    if ($null -ne $reader) { $reader.Dispose() }
  }
}

function Get-CodeHangarAuthenticodeImageEvidence {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $fullPath = Assert-RegularReleaseFile -Path $Path -Label $Label -RequirePe
  $stream = $null
  try {
    # A single FileShare.Read handle denies concurrent writers/deleters while
    # both the ordinary receipt hash and the Authenticode-stable image digest
    # are derived. Separate opens here would let a file change between them.
    $stream = [System.IO.FileStream]::new(
      $fullPath,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    $length = [long]$stream.Length
    $sha256 = (Get-LockedStreamSha256 -Stream $stream).ToLowerInvariant()
    $imageSha256 = Get-CodeHangarAuthenticodeImageSha256FromLockedStream -Stream $stream -Label $Label
    if ($stream.Length -ne $length) {
      throw "$Label changed while its Authenticode image evidence was computed: $fullPath"
    }
    return [pscustomobject]@{
      Path = $fullPath
      FileName = [System.IO.Path]::GetFileName($fullPath)
      Length = $length
      Sha256 = $sha256
      ImageSha256 = $imageSha256
    }
  } finally {
    if ($null -ne $stream) { $stream.Dispose() }
  }
}

function Get-CodeHangarAuthenticodeImageSha256 {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label
  )

  return (Get-CodeHangarAuthenticodeImageEvidence -Path $Path -Label $Label).ImageSha256
}

function Get-EmbeddedAuthenticodeCms {
  param([Parameter(Mandatory = $true)][System.IO.FileStream]$Stream)

  $reader = [System.IO.BinaryReader]::new($Stream, [System.Text.Encoding]::UTF8, $true)
  try {
    if ($Stream.Length -lt 512) { throw "Pinned WebView2 input is too small to be a PE file." }
    $Stream.Position = 0
    if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Pinned WebView2 input has no MZ header." }
    $Stream.Position = 0x3C
    $peOffset = $reader.ReadUInt32()
    if ($peOffset -lt 0x40 -or $peOffset -gt ($Stream.Length - 256)) {
      throw "Pinned WebView2 input has an invalid PE offset."
    }
    $Stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) { throw "Pinned WebView2 input has no PE signature." }
    $machine = $reader.ReadUInt16()
    $Stream.Position = $peOffset + 24
    $optionalMagic = $reader.ReadUInt16()
    if ($optionalMagic -eq 0x10B) {
      $dataDirectory = $peOffset + 24 + 96
    } elseif ($optionalMagic -eq 0x20B) {
      $dataDirectory = $peOffset + 24 + 112
    } else {
      throw "Pinned WebView2 input has an unsupported PE optional-header format."
    }
    $Stream.Position = $dataDirectory + 32
    $certificateOffset = $reader.ReadUInt32()
    $certificateSize = $reader.ReadUInt32()
    if ($certificateOffset -le 0 -or $certificateSize -lt 16 -or
        ([long]$certificateOffset + [long]$certificateSize) -gt $Stream.Length) {
      throw "Pinned WebView2 input has no bounded embedded Authenticode table."
    }
    $Stream.Position = $certificateOffset
    $certificateLength = $reader.ReadUInt32()
    $certificateRevision = $reader.ReadUInt16()
    $certificateType = $reader.ReadUInt16()
    if ($certificateLength -lt 16 -or $certificateLength -gt $certificateSize -or
        $certificateRevision -ne 0x0200 -or $certificateType -ne 0x0002) {
      throw "Pinned WebView2 input has an invalid WIN_CERTIFICATE record."
    }
    $cmsBytes = $reader.ReadBytes([int]$certificateLength - 8)
    if ($cmsBytes.Length -ne ([int]$certificateLength - 8)) {
      throw "Pinned WebView2 Authenticode record is truncated."
    }
    return [pscustomobject]@{
      Machine = ("{0:X4}" -f $machine)
      CmsBytes = $cmsBytes
    }
  } finally {
    $Stream.Position = 0
    $reader.Dispose()
  }
}

function Assert-OfflineCertificateChain {
  param(
    [Parameter(Mandatory = $true)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
    [Parameter(Mandatory = $true)]$ExtraCertificates,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
  try {
    $chain.ChainPolicy.RevocationMode = [System.Security.Cryptography.X509Certificates.X509RevocationMode]::Offline
    $chain.ChainPolicy.RevocationFlag = [System.Security.Cryptography.X509Certificates.X509RevocationFlag]::ExcludeRoot
    $chain.ChainPolicy.VerificationFlags = [System.Security.Cryptography.X509Certificates.X509VerificationFlags]::NoFlag
    $chain.ChainPolicy.DisableCertificateDownloads = $true
    foreach ($extra in $ExtraCertificates) { [void]$chain.ChainPolicy.ExtraStore.Add($extra) }
    if (-not $chain.Build($Certificate)) {
      $statuses = @($chain.ChainStatus | ForEach-Object { "$($_.Status): $($_.StatusInformation.Trim())" })
      throw "$Label does not build a valid offline certificate chain: $($statuses -join '; ')"
    }
  } finally {
    $chain.Dispose()
  }
}

function Assert-PinnedWebView2EvidenceMatchesManifest {
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [Parameter(Mandatory = $true)]$Manifest
  )
  $comparisons = [ordered]@{
    Length = [long]$Manifest.length
    Sha256 = [string]$Manifest.sha256
    FileVersion = [string]$Manifest.fileVersion
    PeMachine = [string]$Manifest.peMachine
    SignerSubject = [string]$Manifest.signerSubject
    SignerThumbprint = [string]$Manifest.signerThumbprint
    SignerIssuer = [string]$Manifest.signerIssuer
    TimestampThumbprint = [string]$Manifest.timestampThumbprint
  }
  foreach ($entry in $comparisons.GetEnumerator()) {
    if ([string]$Evidence.($entry.Key) -cne [string]$entry.Value) {
      throw "Pinned WebView2 $($entry.Key) does not match the manifest."
    }
  }
}

function Test-PinnedWebView2LockedInput {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][System.IO.FileStream]$Stream,
    [Parameter(Mandatory = $true)]$Manifest
  )

  if ($Stream.Length -ne [long]$Manifest.length) {
    throw "Pinned WebView2 length mismatch: $($Stream.Length), expected $($Manifest.length)."
  }
  $sha256 = Get-LockedStreamSha256 -Stream $Stream
  if ($sha256 -cne [string]$Manifest.sha256) {
    throw "Pinned WebView2 SHA-256 mismatch: $sha256."
  }
  $pe = Get-EmbeddedAuthenticodeCms -Stream $Stream
  if ($pe.Machine -cne [string]$Manifest.peMachine) {
    throw "Pinned WebView2 PE Machine mismatch: $($pe.Machine), expected $($Manifest.peMachine)."
  }
  $fileVersion = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($Path).FileVersion
  if ($fileVersion -cne [string]$Manifest.fileVersion) {
    throw "Pinned WebView2 fileVersion mismatch: $fileVersion."
  }

  Add-Type -AssemblyName System.Security.Cryptography.Pkcs
  $cms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
  $cms.Decode($pe.CmsBytes)
  $cms.CheckSignature($true)
  if ($cms.SignerInfos.Count -ne 1) { throw "Pinned WebView2 must have exactly one primary Authenticode signer." }
  $signer = $cms.SignerInfos[0].Certificate
  if ($signer.Subject -cne [string]$Manifest.signerSubject -or
      $signer.Thumbprint -cne [string]$Manifest.signerThumbprint -or
      $signer.Issuer -cne [string]$Manifest.signerIssuer) {
    throw "Pinned WebView2 signer metadata does not match the manifest."
  }
  Assert-OfflineCertificateChain -Certificate $signer -ExtraCertificates $cms.Certificates -Label "Pinned WebView2 signer"

  $timestampAttributes = @($cms.SignerInfos[0].UnsignedAttributes | Where-Object {
      $_.Oid.Value -eq "1.3.6.1.4.1.311.3.3.1"
    })
  if ($timestampAttributes.Count -ne 1 -or $timestampAttributes[0].Values.Count -ne 1) {
    throw "Pinned WebView2 must have exactly one RFC3161 Authenticode timestamp."
  }
  $timestampCms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
  $timestampCms.Decode($timestampAttributes[0].Values[0].RawData)
  $timestampCms.CheckSignature($true)
  if ($timestampCms.SignerInfos.Count -ne 1) { throw "Pinned WebView2 timestamp must have exactly one signer." }
  $timestampSigner = $timestampCms.SignerInfos[0].Certificate
  if ($timestampSigner.Thumbprint -cne [string]$Manifest.timestampThumbprint) {
    throw "Pinned WebView2 timestamp thumbprint does not match the manifest."
  }
  Assert-OfflineCertificateChain -Certificate $timestampSigner -ExtraCertificates $timestampCms.Certificates -Label "Pinned WebView2 timestamp signer"

  if ($null -eq ("CodeHangar.Packaging.OfflineAuthenticode" -as [type])) {
    Add-Type -Path (Join-Path $script:CodeHangarPackagingScriptsRoot "WebView2Authenticode.cs")
  }
  [CodeHangar.Packaging.OfflineAuthenticode]::VerifyFile($Path)
  $Stream.Position = 0
  $evidence = [pscustomobject]@{
    Path = $Path
    Length = $Stream.Length
    Sha256 = $sha256
    FileVersion = $fileVersion
    PeMachine = $pe.Machine
    SignerSubject = $signer.Subject
    SignerThumbprint = $signer.Thumbprint
    SignerIssuer = $signer.Issuer
    TimestampThumbprint = $timestampSigner.Thumbprint
  }
  Assert-PinnedWebView2EvidenceMatchesManifest -Evidence $evidence -Manifest $Manifest
  return $evidence
}

function Open-PinnedWebView2Installer {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)]$Manifest,
    [Parameter(Mandatory = $true)][string]$Label
  )

  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) {
    throw "$Label path must be fully qualified, not relative or drive-ambiguous: $Path"
  }
  Assert-FixedLocalPathChain -Path $Path -Label $Label -RequireExisting
  if ([System.IO.Path]::GetFileName($Path) -cne [string]$Manifest.filename) {
    throw "$Label must have the exact manifest filename '$($Manifest.filename)'."
  }
  $item = Get-Item -LiteralPath $Path -Force
  if ($item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "$Label must be a regular non-reparse file: $Path"
  }
  $stream = $null
  try {
    $stream = [System.IO.FileStream]::new(
      $item.FullName,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    $evidence = Test-PinnedWebView2LockedInput -Path $item.FullName -Stream $stream -Manifest $Manifest
    return [pscustomobject]@{ Stream = $stream; Evidence = $evidence }
  } catch {
    if ($null -ne $stream) { $stream.Dispose() }
    throw
  }
}

function Assert-SafeNsisLiteralPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  if ($Path -cnotmatch "^[A-Za-z]:\\[A-Za-z0-9 _.\\-]+$" -or $Path -match '[\r\n"$`]') {
    throw "NSIS source/include path contains unsafe quoting or preprocessor characters: $Path"
  }
}

function Get-TauriConfigArguments {
  param(
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][string]$OverridePath
  )
  if ($Edition -eq "Connector") {
    return @(
      "--config", (Join-Path $RepoRoot "apps\desktop\src-tauri\tauri.connector.conf.json"),
      "--config", (Join-Path $RepoRoot "apps\desktop\src-tauri\tauri.release-connector.conf.json"),
      "--config", $OverridePath
    )
  }
  return @(
    "--config", (Join-Path $RepoRoot "apps\desktop\src-tauri\tauri.release-local.conf.json"),
    "--config", $OverridePath
  )
}

function Assert-LocalInstallerHookIsolation {
  param(
    [Parameter(Mandatory = $true)][string]$BaseHookContent,
    [Parameter(Mandatory = $true)][string]$GeneratedHookTemplate
  )

  foreach ($forbiddenMarker in @(
      'Connector',
      'AI Assist',
      'MCP',
      'API key',
      'agent automation',
      'agent_automation',
      'hangar-agent',
      'hangar-ai',
      'code-hangar-mcp',
      'provider'
    )) {
    if ($BaseHookContent.IndexOf($forbiddenMarker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $GeneratedHookTemplate.IndexOf($forbiddenMarker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
      throw "The effective Local NSIS hook contains a Connector/AI capability, product name or path marker: $forbiddenMarker"
    }
  }
}

function Get-TauriCompileConfigArguments {
  param(
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][string]$OverridePath
  )
  # Release overlays name bundle-only sidecars/resources that deliberately do
  # not exist until the post-Authenticode BundleSigned phase. The compile-only
  # pass still needs the edition overlay and generated offline WebView policy.
  if ($Edition -eq "Connector") {
    return @(
      "--config", (Join-Path $RepoRoot "apps\desktop\src-tauri\tauri.connector.conf.json"),
      "--config", $OverridePath
    )
  }
  return @("--config", $OverridePath)
}

function New-PinnedWebView2PackagingContext {
  param(
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][string]$ManifestPath
  )

  $manifest = Read-PinnedWebView2Manifest -ManifestPath $ManifestPath
  $source = $null
  $staged = $null
  $runDirectory = $null
  try {
    $source = Open-PinnedWebView2Installer -Path $InstallerPath -Manifest $manifest -Label "The explicit WebView2 release input"
    $generatedRoot = Join-Path $RepoRoot ".local\packaging-generated"
    if (-not (Test-Path -LiteralPath $generatedRoot)) {
      [void][System.IO.Directory]::CreateDirectory($generatedRoot)
    }
    Assert-FixedLocalPathChain -Path $generatedRoot -Label "The generated packaging root" -RequireExisting
    $runDirectory = Join-Path $generatedRoot ("webview2-{0}-{1}" -f $Edition.ToLowerInvariant(), [guid]::NewGuid().ToString("N"))
    [void][System.IO.Directory]::CreateDirectory($runDirectory)
    Assert-FixedLocalPathChain -Path $runDirectory -Label "The generated packaging run" -RequireExisting

    $stagedPath = Join-Path $runDirectory ([string]$manifest.filename)
    $destination = [System.IO.FileStream]::new(
      $stagedPath,
      [System.IO.FileMode]::CreateNew,
      [System.IO.FileAccess]::Write,
      [System.IO.FileShare]::None
    )
    try {
      $source.Stream.Position = 0
      $source.Stream.CopyTo($destination, 1048576)
      $destination.Flush($true)
    } finally {
      $destination.Dispose()
      $source.Stream.Position = 0
    }
    $staged = Open-PinnedWebView2Installer -Path $stagedPath -Manifest $manifest -Label "The staged WebView2 release input"
    if ($source.Evidence.Sha256 -cne $staged.Evidence.Sha256) {
      throw "Source and staged WebView2 SHA-256 values differ."
    }

    $baseHookPath = Join-Path $RepoRoot "apps\desktop\src-tauri\windows\shell-integration.nsh"
    Assert-FixedLocalPathChain -Path $baseHookPath -Label "The tracked NSIS shell hook" -RequireExisting
    $baseHookContent = [System.IO.File]::ReadAllText($baseHookPath)
    Assert-SafeNsisLiteralPath -Path $stagedPath
    Assert-SafeNsisLiteralPath -Path $baseHookPath
    $hookPath = Join-Path $runDirectory "codehangar-pinned-webview2.nsh"
    $hookTemplate = @'
; GENERATED BY scripts/packaging-common.ps1. DO NOT REUSE.
!define CODEHANGAR_PINNED_WEBVIEW2_READY 1
!macro CODEHANGAR_INSTALL_PINNED_WEBVIEW2
  InitPluginsDir
  File /oname=$PLUGINSDIR\__FILENAME__ "__STAGED_PATH__"
  System::Call 'kernel32::SetEnvironmentVariableW(w "CODEHANGAR_PINNED_WEBVIEW2_PATH", w "$PLUGINSDIR\__FILENAME__") i .r2'
  ${If} $2 = 0
    Abort "Could not bind the extracted pinned WebView2 path for verification."
  ${EndIf}
  nsExec::ExecToStack `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -Command "$$path=[Environment]::GetEnvironmentVariable('CODEHANGAR_PINNED_WEBVIEW2_PATH','Process'); if ([String]::IsNullOrWhiteSpace($$path)) { exit 85 }; $$stream=[System.IO.File]::OpenRead($$path); try { $$sha=[System.Security.Cryptography.SHA256]::Create(); try { $$actual=([System.BitConverter]::ToString($$sha.ComputeHash($$stream))).Replace('-',''); if ($$actual -cne '__SHA256__') { exit 86 }; [Console]::Out.Write($$actual) } finally { $$sha.Dispose() } } finally { $$stream.Dispose() }"`
  Pop $0
  Pop $1
  System::Call 'kernel32::SetEnvironmentVariableW(w "CODEHANGAR_PINNED_WEBVIEW2_PATH", p 0) i .r2'
  ${If} $2 = 0
    Abort "Could not clear the pinned WebView2 verification path."
  ${EndIf}
  ${If} $0 != 0
    Abort "Pinned WebView2 hash verification failed before execution (exit $0)."
  ${EndIf}
  ${If} $1 != "__SHA256__"
    Abort "Pinned WebView2 hash verifier returned unexpected evidence."
  ${EndIf}
  DetailPrint "Pinned WebView2 extracted SHA256 verified: $1"
  ExecWait `"$PLUGINSDIR\__FILENAME__" /silent /install` $0
  ${If} $0 != 0
    Abort "Pinned WebView2 runtime installation failed before Code Hangar installation (exit $0)."
  ${EndIf}
!macroend
!include "__BASE_HOOK__"
'@
    $hook = $hookTemplate.
      Replace("__FILENAME__", [string]$manifest.filename).
      Replace("__STAGED_PATH__", $stagedPath).
      Replace("__SHA256__", [string]$manifest.sha256).
      Replace("__BASE_HOOK__", $baseHookPath)
    if ($Edition -eq 'Local') {
      # Source/include paths are preprocessor inputs rather than installed copy.
      # Normalize them before scanning the fully rendered hook so a checkout
      # directory name cannot create a false capability leak.
      $reviewedGeneratedHook = $hook.
        Replace($stagedPath, '__STAGED_PATH__').
        Replace($baseHookPath, '__BASE_HOOK__')
      Assert-LocalInstallerHookIsolation -BaseHookContent $baseHookContent -GeneratedHookTemplate $reviewedGeneratedHook
    }
    [System.IO.File]::WriteAllText($hookPath, $hook, [System.Text.UTF8Encoding]::new($false))

    $overridePath = Join-Path $runDirectory "tauri.webview2.override.json"
    $override = [ordered]@{
      bundle = [ordered]@{
        createUpdaterArtifacts = $false
        windows = [ordered]@{
          minimumWebview2Version = $null
          webviewInstallMode = [ordered]@{ type = "skip" }
          nsis = [ordered]@{ installerHooks = $hookPath }
        }
      }
    }
    [System.IO.File]::WriteAllText(
      $overridePath,
      ($override | ConvertTo-Json -Depth 8),
      [System.Text.UTF8Encoding]::new($false)
    )
    $configArguments = @(Get-TauriConfigArguments -Edition $Edition -RepoRoot $RepoRoot -OverridePath $overridePath)
    return [pscustomobject]@{
      Edition = $Edition
      ManifestPath = $ManifestPath
      Manifest = $manifest
      Source = $source
      Staged = $staged
      GeneratedDirectory = $runDirectory
      HookPath = $hookPath
      OverridePath = $overridePath
      BaseHookPath = $baseHookPath
      TauriConfigArguments = $configArguments
    }
  } catch {
    if ($null -ne $staged) { $staged.Stream.Dispose() }
    if ($null -ne $source) { $source.Stream.Dispose() }
    if ($null -ne $runDirectory -and (Test-Path -LiteralPath $runDirectory)) {
      Remove-Item -LiteralPath $runDirectory -Recurse -Force
    }
    throw
  }
}

function Get-CanonicalReleaseTreeEvidence {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Schema,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  Assert-FixedLocalPathChain -Path $rootFull -Label $Label -RequireExisting
  $rootItem = Get-Item -LiteralPath $rootFull -Force
  if (-not $rootItem.PSIsContainer -or (($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "$Label must be a real non-reparse directory: $rootFull"
  }
  $items = @(Get-ChildItem -LiteralPath $rootFull -Recurse -Force -ErrorAction Stop)
  foreach ($item in $items) {
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "$Label contains a reparse point: $($item.FullName)"
    }
  }
  $files = @($items | Where-Object { -not $_.PSIsContainer })
  if ($files.Count -eq 0) { throw "$Label contains no files." }
  $lines = [System.Collections.Generic.List[string]]::new()
  foreach ($file in $files) {
    $relative = [System.IO.Path]::GetRelativePath($rootFull, $file.FullName).Replace('\', '/')
    if ([string]::IsNullOrWhiteSpace($relative) -or $relative -match '[=\r\n]') {
      throw "$Label contains an unsafe canonical relative path: $relative"
    }
    $evidence = Get-StableReleaseArtifactEvidence -Path $file.FullName -Label "$Label file $relative"
    $lines.Add("$relative=$($evidence.Length):$($evidence.Sha256)")
  }
  $ordered = $lines.ToArray()
  [System.Array]::Sort($ordered, [System.StringComparer]::Ordinal)
  $payload = [System.Text.Encoding]::UTF8.GetBytes("schema=$Schema`n" + ($ordered -join "`n") + "`n")
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    return [pscustomobject]@{
      Count = $files.Count
      Sha256 = (([System.BitConverter]::ToString($sha.ComputeHash($payload))).Replace('-', '')).ToLowerInvariant()
    }
  } finally {
    $sha.Dispose()
  }
}

function Assert-CanonicalReleaseTreeEvidence {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Schema,
    [Parameter(Mandatory = $true)][string]$Label,
    [Parameter(Mandatory = $true)][long]$ExpectedCount,
    [Parameter(Mandatory = $true)][string]$ExpectedSha256
  )

  if ($ExpectedCount -le 0 -or $ExpectedSha256 -cnotmatch '^[0-9a-f]{64}$') {
    throw "$Label expected tree evidence is invalid."
  }
  $actual = Get-CanonicalReleaseTreeEvidence -Root $Root -Schema $Schema -Label $Label
  if ($actual.Count -ne $ExpectedCount -or $actual.Sha256 -cne $ExpectedSha256) {
    throw "$Label no longer matches the receipt-bound tree ($($actual.Count):$($actual.Sha256) != ${ExpectedCount}:$ExpectedSha256)."
  }
  return $actual
}

function Get-CanonicalReleaseTreeFiles {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  Assert-FixedLocalPathChain -Path $rootFull -Label $Label -RequireExisting
  $rootItem = Get-Item -LiteralPath $rootFull -Force
  if (-not $rootItem.PSIsContainer -or (($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "$Label must be a real non-reparse directory: $rootFull"
  }
  $files = @()
  foreach ($item in @(Get-ChildItem -LiteralPath $rootFull -Recurse -Force -ErrorAction Stop)) {
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "$Label contains a reparse point: $($item.FullName)"
    }
    if (-not $item.PSIsContainer) {
      $relative = [System.IO.Path]::GetRelativePath($rootFull, $item.FullName).Replace('\', '/')
      if ([string]::IsNullOrWhiteSpace($relative) -or $relative -match '[=\r\n]' -or
          $relative.StartsWith('../', [System.StringComparison]::Ordinal) -or
          $relative.Equals('..', [System.StringComparison]::Ordinal)) {
        throw "$Label contains an unsafe canonical relative path: $relative"
      }
      $files += $item
    }
  }
  if ($files.Count -eq 0) { throw "$Label contains no files." }
  return @($files | Sort-Object -Property FullName)
}

function Close-ReleaseArtifactLocks {
  param($Locks)

  if ($null -eq $Locks) { return }
  foreach ($lock in $Locks) {
    if ($null -ne $lock) { $lock.Dispose() }
  }
  if ($Locks -is [System.Collections.IList]) { $Locks.Clear() }
}

function Open-CanonicalReleaseTreeReadLocks {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Schema,
    [Parameter(Mandatory = $true)][string]$Label,
    [Parameter(Mandatory = $true)][long]$ExpectedCount,
    [Parameter(Mandatory = $true)][string]$ExpectedSha256
  )

  [void](Assert-CanonicalReleaseTreeEvidence `
    -Root $Root `
    -Schema $Schema `
    -Label $Label `
    -ExpectedCount $ExpectedCount `
    -ExpectedSha256 $ExpectedSha256)
  $locks = [System.Collections.Generic.List[System.IDisposable]]::new()
  try {
    $files = @(Get-CanonicalReleaseTreeFiles -Root $Root -Label $Label)
    if ($files.Count -ne $ExpectedCount) {
      throw "$Label changed while its read locks were being established."
    }
    foreach ($file in $files) {
      $locks.Add((Open-ReleaseArtifactReadLock -Path $file.FullName -Label "$Label file $($file.Name)"))
    }
    $tree = Assert-CanonicalReleaseTreeEvidence `
      -Root $Root `
      -Schema $Schema `
      -Label $Label `
      -ExpectedCount $ExpectedCount `
      -ExpectedSha256 $ExpectedSha256
    return [pscustomobject]@{
      Tree = $tree
      Locks = $locks
    }
  } catch {
    Close-ReleaseArtifactLocks -Locks $locks
    throw
  }
}

function Copy-CanonicalReleaseTree {
  param(
    [Parameter(Mandatory = $true)][string]$SourceRoot,
    [Parameter(Mandatory = $true)][string]$DestinationRoot,
    [Parameter(Mandatory = $true)][string]$Schema,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $sourceFull = [System.IO.Path]::GetFullPath($SourceRoot).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $destinationFull = [System.IO.Path]::GetFullPath($DestinationRoot).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  if ($sourceFull.Equals($destinationFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "$Label source and destination directories must be distinct."
  }
  $sourceTree = Get-CanonicalReleaseTreeEvidence -Root $sourceFull -Schema $Schema -Label "$Label source"
  if (Test-Path -LiteralPath $destinationFull) {
    throw "$Label destination directory already exists; refusing to overwrite it: $destinationFull"
  }
  $destinationParent = Split-Path -Parent $destinationFull
  Assert-FixedLocalPathChain -Path $destinationParent -Label "$Label destination parent" -RequireExisting
  [void][System.IO.Directory]::CreateDirectory($destinationFull)
  Assert-FixedLocalPathChain -Path $destinationFull -Label "$Label destination" -RequireExisting

  $files = @(Get-CanonicalReleaseTreeFiles -Root $sourceFull -Label "$Label source")
  if ($files.Count -ne $sourceTree.Count) {
    throw "$Label source changed before its snapshot could be copied."
  }
  foreach ($file in $files) {
    $relative = [System.IO.Path]::GetRelativePath($sourceFull, $file.FullName).Replace('\', '/')
    if ([string]::IsNullOrWhiteSpace($relative) -or $relative -match '[=\r\n]' -or
        $relative.StartsWith('../', [System.StringComparison]::Ordinal) -or
        $relative.Equals('..', [System.StringComparison]::Ordinal)) {
      throw "$Label source contains an unsafe canonical relative path: $relative"
    }
    $destinationPath = [System.IO.Path]::GetFullPath((Join-Path $destinationFull ($relative.Replace('/', '\'))))
    if (-not $destinationPath.StartsWith(($destinationFull + [System.IO.Path]::DirectorySeparatorChar), [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "$Label destination escaped its snapshot root: $destinationPath"
    }
    $destinationDirectory = Split-Path -Parent $destinationPath
    if (-not (Test-Path -LiteralPath $destinationDirectory)) {
      [void][System.IO.Directory]::CreateDirectory($destinationDirectory)
    }
    Assert-FixedLocalPathChain -Path $destinationDirectory -Label "$Label destination directory" -RequireExisting
    $sourceEvidence = Get-StableReleaseArtifactEvidence -Path $file.FullName -Label "$Label source file $relative"
    [void](Set-ExactReleaseArtifact `
      -SourcePath $sourceEvidence.Path `
      -DestinationPath $destinationPath `
      -Label "$Label snapshot file $relative" `
      -ExpectedSha256 $sourceEvidence.Sha256)
  }

  [void](Assert-CanonicalReleaseTreeEvidence `
    -Root $sourceFull `
    -Schema $Schema `
    -Label "$Label source" `
    -ExpectedCount $sourceTree.Count `
    -ExpectedSha256 $sourceTree.Sha256)
  $destinationTree = Assert-CanonicalReleaseTreeEvidence `
    -Root $destinationFull `
    -Schema $Schema `
    -Label "$Label destination" `
    -ExpectedCount $sourceTree.Count `
    -ExpectedSha256 $sourceTree.Sha256
  return [pscustomobject]@{
    Source = $sourceTree
    Destination = $destinationTree
  }
}

function Get-ValidatedNsisCompiler {
  if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    throw "LOCALAPPDATA is required to locate the pinned NSIS toolchain."
  }
  $nsisRoot = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA "tauri\NSIS"))
  Assert-FixedLocalPathChain -Path $nsisRoot -Label "The Tauri NSIS toolchain" -RequireExisting
  $required = @(
    "makensis.exe",
    "Bin\makensis.exe",
    "Stubs\lzma-x86-unicode",
    "Stubs\lzma_solid-x86-unicode",
    "Plugins\x86-unicode\additional\nsis_tauri_utils.dll",
    "Include\MUI2.nsh",
    "Include\FileFunc.nsh",
    "Include\x64.nsh",
    "Include\nsDialogs.nsh",
    "Include\WinMessages.nsh",
    "Include\Win\COM.nsh",
    "Include\Win\Propkey.nsh",
    "Include\Win\RestartManager.nsh"
  )
  foreach ($relative in $required) {
    $path = Join-Path $nsisRoot $relative
    Assert-FixedLocalPathChain -Path $path -Label "Required pinned NSIS artifact" -RequireExisting
    $item = Get-Item -LiteralPath $path -Force
    if ($item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) -or $item.Length -le 0) {
      throw "Required pinned NSIS artifact is not a regular non-empty file: $path"
    }
  }
  $plugin = Join-Path $nsisRoot "Plugins\x86-unicode\additional\nsis_tauri_utils.dll"
  $pluginSha1 = (Get-FileHash -LiteralPath $plugin -Algorithm SHA1).Hash
  if ($pluginSha1 -cne "75197FEE3C6A814FE035788D1C34EAD39349B860") {
    throw "Pinned nsis_tauri_utils.dll SHA-1 does not match tauri-bundler 2.9.2."
  }
  $makensis = Join-Path $nsisRoot "makensis.exe"
  if (-not (Test-BasicPortableExecutable -Path $makensis)) {
    throw "Pinned NSIS makensis.exe does not have a basic PE/MZ structure."
  }
  $versionInfo = [System.Diagnostics.ProcessStartInfo]::new()
  $versionInfo.FileName = $makensis
  $versionInfo.ArgumentList.Add("/VERSION")
  $versionInfo.UseShellExecute = $false
  $versionInfo.RedirectStandardOutput = $true
  $versionInfo.RedirectStandardError = $true
  $versionInfo.CreateNoWindow = $true
  $versionProcess = [System.Diagnostics.Process]::Start($versionInfo)
  $versionOutput = $versionProcess.StandardOutput.ReadToEnd().Trim()
  $versionError = $versionProcess.StandardError.ReadToEnd().Trim()
  $versionProcess.WaitForExit()
  if ($versionProcess.ExitCode -ne 0 -or $versionOutput -cne "v3.11") {
    throw "Pinned NSIS compiler is not v3.11 (exit $($versionProcess.ExitCode); stdout '$versionOutput'; stderr '$versionError')."
  }
  $tree = Get-CanonicalReleaseTreeEvidence `
    -Root $nsisRoot `
    -Schema "codehangar/nsis-tree/1" `
    -Label "The pinned Tauri NSIS 3.11 tree"
  if ($tree.Count -ne 442 -or $tree.Sha256 -cne "037d77f1f7359f9cc5e5f90842ea28dd8b8f17c8f5d35f0a7266f534e700e619") {
    throw "Pinned Tauri NSIS 3.11 tree does not match the audited 442-file digest (found $($tree.Count) files, $($tree.Sha256))."
  }
  return $makensis
}

function Invoke-GeneratedNsisHookSyntaxCheck {
  param([Parameter(Mandatory = $true)]$Context)

  $makensis = Get-ValidatedNsisCompiler
  Assert-SafeNsisLiteralPath -Path $Context.HookPath
  $syntaxPath = Join-Path $Context.GeneratedDirectory "hook-syntax-check.nsi"
  $unexpectedOutput = Join-Path $Context.GeneratedDirectory "hook-syntax-check-must-not-exist.exe"
  Assert-SafeNsisLiteralPath -Path $unexpectedOutput
  $template = @'
Unicode true
!include "LogicLib.nsh"
Name "Code Hangar generated hook syntax check"
OutFile "__OUTPUT__"
!include "__HOOK__"
Section
  !insertmacro CODEHANGAR_INSTALL_PINNED_WEBVIEW2
SectionEnd
'@
  $content = $template.Replace("__OUTPUT__", $unexpectedOutput).Replace("__HOOK__", $Context.HookPath)
  [System.IO.File]::WriteAllText($syntaxPath, $content, [System.Text.UTF8Encoding]::new($false))
  $start = [System.Diagnostics.ProcessStartInfo]::new()
  $start.FileName = $makensis
  $start.ArgumentList.Add("/NOCD")
  $start.ArgumentList.Add("/PPO")
  $start.ArgumentList.Add($syntaxPath)
  $start.UseShellExecute = $false
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  $start.CreateNoWindow = $true
  $process = [System.Diagnostics.Process]::Start($start)
  $stdout = $process.StandardOutput.ReadToEnd()
  $stderr = $process.StandardError.ReadToEnd()
  $process.WaitForExit()
  if ($process.ExitCode -ne 0) {
    throw "Generated NSIS hook failed real makensis /PPO syntax validation (exit $($process.ExitCode)): $stderr $stdout"
  }
  if (Test-Path -LiteralPath $unexpectedOutput) {
    throw "NSIS /PPO syntax validation unexpectedly produced an executable."
  }
}

function Remove-PinnedWebView2PackagingContext {
  param(
    [Parameter(Mandatory = $true)]$Context,
    [Parameter(Mandatory = $true)][string]$RepoRoot
  )
  if ($null -ne $Context.Staged) { $Context.Staged.Stream.Dispose() }
  if ($null -ne $Context.Source) { $Context.Source.Stream.Dispose() }
  $generatedRoot = [System.IO.Path]::GetFullPath((Join-Path $RepoRoot ".local\packaging-generated"))
  $runDirectory = [System.IO.Path]::GetFullPath([string]$Context.GeneratedDirectory)
  if (-not ([System.IO.Path]::GetDirectoryName($runDirectory)).Equals($generatedRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not ([System.IO.Path]::GetFileName($runDirectory)).StartsWith("webview2-", [System.StringComparison]::Ordinal)) {
    throw "Refusing unsafe generated packaging cleanup: $runDirectory"
  }
  if (Test-Path -LiteralPath $runDirectory) {
    foreach ($item in @(Get-ChildItem -LiteralPath $runDirectory -Recurse -Force -ErrorAction Stop)) {
      if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing generated packaging cleanup containing a reparse point: $($item.FullName)"
      }
    }
    Remove-Item -LiteralPath $runDirectory -Recurse -Force -ErrorAction Stop
  }
}

$script:CodeHangarSigningReceiptSchema = "codehangar/signing-preparation/3"
$script:CodeHangarSigningReceiptFileName = "code-hangar-signing-receipt.json"
$script:CodeHangarFrontendSnapshotSchema = "codehangar/frontend-dist/1"
$script:CodeHangarFrontendSnapshotDirectoryName = "frontend-dist"
$script:CodeHangarFrontendSnapshotManifestFileName = "code-hangar-frontend-dist.json"
$script:CodeHangarReleaseManifestFileName = "code-hangar-release-manifest.json"
$script:CodeHangarReceiptBoundReleaseIdSchema = "codehangar/receipt-bound-release-id/1"

function Get-CodeHangarReceiptBoundReleaseId {
  param(
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$SigningReceiptSha256
  )

  if ($SigningReceiptSha256 -cnotmatch '^[0-9A-Fa-f]{64}$') {
    throw "The signing-receipt SHA-256 must be exactly 64 hexadecimal characters."
  }
  $canonicalEdition = if ($Edition -ieq "Local") { "Local" } else { "Connector" }
  $payload = "schema=$script:CodeHangarReceiptBoundReleaseIdSchema`nedition=$canonicalEdition`nsigning_receipt_sha256=$($SigningReceiptSha256.ToLowerInvariant())`n"
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $digest = $sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($payload))
    return (([System.BitConverter]::ToString($digest)).Replace('-', '')).ToLowerInvariant()
  } finally {
    $sha.Dispose()
  }
}

function Get-CodeHangarCleanGitIdentity {
  param([Parameter(Mandatory = $true)][string]$RepoRoot)

  $commit = ([string](& git -C $RepoRoot rev-parse HEAD)).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw "Unable to read the packaging source commit." }
  $tree = ([string](& git -C $RepoRoot rev-parse 'HEAD^{tree}')).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw "Unable to read the packaging source tree." }
  $status = @(& git -C $RepoRoot status --porcelain --untracked-files=all)
  if ($LASTEXITCODE -ne 0) { throw "Unable to prove that the packaging source tree is clean." }
  if ($status.Count -ne 0) {
    throw "PrepareSigning and BundleSigned require the exact clean release source tree."
  }
  if ($commit -cnotmatch '^[0-9a-f]{40,64}$' -or $tree -cnotmatch '^[0-9a-f]{40,64}$') {
    throw "The packaging source commit/tree identity is malformed."
  }
  return [pscustomobject]@{ Commit = $commit; Tree = $tree }
}

function Assert-ReleaseRootPublicBlobHex {
  param([Parameter(Mandatory = $true)][string]$Value)

  if ($Value -cnotmatch '^[0-9A-Fa-f]+$' -or
      $Value.Length -lt 822 -or
      $Value.Length -gt 2112 -or
      ($Value.Length % 2) -ne 0) {
    throw "-ReleaseRootPublicBlobHex must be a bounded 3072..8192-bit BCRYPT RSA public blob encoded as hexadecimal."
  }
  $normalized = $Value.ToUpperInvariant()
  $bytes = [System.Convert]::FromHexString($normalized)
  $magic = [System.BitConverter]::ToUInt32($bytes, 0)
  $bitLength = [System.BitConverter]::ToUInt32($bytes, 4)
  $exponentLength = [System.BitConverter]::ToUInt32($bytes, 8)
  $modulusLength = [System.BitConverter]::ToUInt32($bytes, 12)
  $prime1Length = [System.BitConverter]::ToUInt32($bytes, 16)
  $prime2Length = [System.BitConverter]::ToUInt32($bytes, 20)
  if ($magic -ne 0x31415352 -or
      $bitLength -lt 3072 -or $bitLength -gt 8192 -or ($bitLength % 8) -ne 0 -or
      $exponentLength -lt 3 -or $exponentLength -gt 8 -or
      $modulusLength -ne ($bitLength / 8) -or
      $prime1Length -ne 0 -or $prime2Length -ne 0 -or
      $bytes.Length -ne (24 + $exponentLength + $modulusLength)) {
    throw "-ReleaseRootPublicBlobHex is not a canonical BCRYPT_RSAPUBLIC_BLOB within the audited bounds."
  }
  $modulusOffset = 24 + $exponentLength
  if ($bytes[24] -eq 0 -or ($bytes[$modulusOffset - 1] % 2) -eq 0 -or
      ($bytes[$modulusOffset] -band 0x80) -eq 0 -or
      ($bytes[$bytes.Length - 1] % 2) -eq 0) {
    throw "-ReleaseRootPublicBlobHex contains invalid RSA public parameters."
  }
  return $normalized
}

function Get-CodeHangarHostTargetTriple {
  $details = @(& rustc -Vv)
  if ($LASTEXITCODE -ne 0) { throw "rustc -Vv failed with exit code $LASTEXITCODE." }
  $hostLine = $details | Where-Object { $_ -like "host:*" } | Select-Object -First 1
  if ([string]::IsNullOrWhiteSpace($hostLine)) {
    throw "Could not determine the host target triple from rustc."
  }
  $triple = ($hostLine -replace "host:\s*", "").Trim()
  if ($triple -cnotmatch '^[A-Za-z0-9_.-]+$') {
    throw "Could not determine a safe host target triple from rustc: '$triple'"
  }
  if ($triple -cne "x86_64-pc-windows-msvc") {
    throw "Release packaging requires the audited x86_64-pc-windows-msvc host, not '$triple'."
  }
  return $triple
}

function Get-CodeHangarBundleContractSha256 {
  param(
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition
  )

  $relativePaths = [System.Collections.Generic.List[string]]::new()
  foreach ($path in @(
      "package.json",
      "package-lock.json",
      "Cargo.toml",
      "apps/desktop/package.json",
      "apps/desktop/src-tauri/Cargo.toml",
      "apps/desktop/src-tauri/tauri.conf.json",
      "apps/desktop/src-tauri/windows/shell-integration.nsh",
      "scripts/release-inputs/webview2-x64.json",
      "scripts/WebView2Authenticode.cs",
      "scripts/check-frontend-edition.mjs",
      "scripts/new-release-identity-manifest.ps1",
      "scripts/packaging-common.ps1",
      "scripts/packaging-preflight.mjs"
    )) {
    $relativePaths.Add($path)
  }
  if ($Edition -eq "Connector") {
    $relativePaths.Add("apps/desktop/src-tauri/tauri.connector.conf.json")
    $relativePaths.Add("apps/desktop/src-tauri/tauri.release-connector.conf.json")
    $relativePaths.Add("scripts/package-connector.ps1")
  } else {
    $relativePaths.Add("apps/desktop/src-tauri/tauri.release-local.conf.json")
    $relativePaths.Add("scripts/package-local.ps1")
  }

  $lines = [System.Collections.Generic.List[string]]::new()
  $lines.Add("schema=codehangar/bundle-contract/1")
  $lines.Add("edition=$Edition")
  foreach ($relative in @($relativePaths | Sort-Object)) {
    $fullPath = Join-Path $RepoRoot ($relative.Replace('/', '\'))
    $evidence = Get-StableReleaseArtifactEvidence -Path $fullPath -Label "Bundle-contract input $relative"
    $lines.Add("$relative=$($evidence.Length):$($evidence.Sha256)")
  }
  $payload = [System.Text.Encoding]::UTF8.GetBytes(($lines -join "`n") + "`n")
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    return (([System.BitConverter]::ToString($sha.ComputeHash($payload))).Replace('-', '')).ToLowerInvariant()
  } finally {
    $sha.Dispose()
  }
}

function Assert-RegularReleaseFile {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequirePe
  )

  if (-not [System.IO.Path]::IsPathFullyQualified($Path)) {
    throw "$Label path must be fully qualified: $Path"
  }
  $fullPath = [System.IO.Path]::GetFullPath($Path)
  Assert-FixedLocalPathChain -Path $fullPath -Label $Label -RequireExisting
  $item = Get-Item -LiteralPath $fullPath -Force
  if ($item.PSIsContainer -or
      (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) -or
      $item.Length -le 0) {
    throw "$Label must be a regular, non-reparse, non-empty file: $fullPath"
  }
  if ($RequirePe -and -not (Test-BasicPortableExecutable -Path $fullPath)) {
    throw "$Label does not have a basic PE/MZ structure: $fullPath"
  }
  return $fullPath
}

function Get-StableReleaseArtifactEvidence {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequirePe
  )

  $fullPath = Assert-RegularReleaseFile -Path $Path -Label $Label -RequirePe:$RequirePe
  $stream = $null
  try {
    $stream = [System.IO.FileStream]::new(
      $fullPath,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    $length = $stream.Length
    $sha256 = (Get-LockedStreamSha256 -Stream $stream).ToLowerInvariant()
    if ($stream.Length -ne $length) {
      throw "$Label changed while it was hashed: $fullPath"
    }
    return [pscustomobject]@{
      Path = $fullPath
      FileName = [System.IO.Path]::GetFileName($fullPath)
      Length = [long]$length
      Sha256 = $sha256
    }
  } finally {
    if ($null -ne $stream) { $stream.Dispose() }
  }
}

function Set-ExactReleaseArtifact {
  param(
    [Parameter(Mandatory = $true)][string]$SourcePath,
    [Parameter(Mandatory = $true)][string]$DestinationPath,
    [Parameter(Mandatory = $true)][string]$Label,
    [string]$ExpectedSha256,
    [switch]$RequirePe
  )

  $sourceFull = Assert-RegularReleaseFile -Path $SourcePath -Label "$Label source" -RequirePe:$RequirePe
  $destinationFull = [System.IO.Path]::GetFullPath($DestinationPath)
  $destinationDirectory = Split-Path -Parent $destinationFull
  Assert-FixedLocalPathChain -Path $destinationDirectory -Label "$Label destination directory" -RequireExisting
  if ([System.IO.Path]::GetFileName($destinationFull) -in @('', '.', '..')) {
    throw "$Label destination must be a file path."
  }

  if ($sourceFull.Equals($destinationFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    $sameEvidence = Get-StableReleaseArtifactEvidence -Path $sourceFull -Label $Label -RequirePe:$RequirePe
    if (-not [string]::IsNullOrWhiteSpace($ExpectedSha256) -and
        $sameEvidence.Sha256 -cne $ExpectedSha256.ToLowerInvariant()) {
      throw "$Label source SHA-256 does not match the expected value."
    }
    return $sameEvidence
  }

  if (Test-Path -LiteralPath $destinationFull) {
    $existing = Get-Item -LiteralPath $destinationFull -Force
    if ($existing.PSIsContainer -or (($existing.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
      throw "Refusing to replace a non-regular/reparse $Label destination: $destinationFull"
    }
  }

  $temporaryPath = Join-Path $destinationDirectory (".codehangar-stage-{0}.tmp" -f [guid]::NewGuid().ToString("N"))
  $source = $null
  $destination = $null
  try {
    $source = [System.IO.FileStream]::new(
      $sourceFull,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    $sourceSha256 = (Get-LockedStreamSha256 -Stream $source).ToLowerInvariant()
    if (-not [string]::IsNullOrWhiteSpace($ExpectedSha256) -and
        $sourceSha256 -cne $ExpectedSha256.ToLowerInvariant()) {
      throw "$Label source SHA-256 does not match the expected value."
    }
    $destination = [System.IO.FileStream]::new(
      $temporaryPath,
      [System.IO.FileMode]::CreateNew,
      [System.IO.FileAccess]::Write,
      [System.IO.FileShare]::None
    )
    $source.Position = 0
    $source.CopyTo($destination, 1048576)
    $destination.Flush($true)
    $destination.Dispose()
    $destination = $null
    $copied = Get-StableReleaseArtifactEvidence -Path $temporaryPath -Label "$Label temporary copy" -RequirePe:$RequirePe
    if ($copied.Sha256 -cne $sourceSha256) {
      throw "$Label copy changed bytes ($($copied.Sha256) != $sourceSha256)."
    }
    [System.IO.File]::Move($temporaryPath, $destinationFull, $true)
    $result = Get-StableReleaseArtifactEvidence -Path $destinationFull -Label $Label -RequirePe:$RequirePe
    if ($result.Sha256 -cne $sourceSha256) {
      throw "$Label destination changed bytes after atomic placement."
    }
    return $result
  } finally {
    if ($null -ne $destination) { $destination.Dispose() }
    if ($null -ne $source) { $source.Dispose() }
    if (Test-Path -LiteralPath $temporaryPath -PathType Leaf) {
      Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction Stop
    }
  }
}

function Remove-ExactReleaseArtifact {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label
  )
  if (-not (Test-Path -LiteralPath $Path)) { return }
  $item = Get-Item -LiteralPath $Path -Force
  if ($item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "Refusing to remove a non-regular/reparse ${Label}: $Path"
  }
  Remove-Item -LiteralPath $item.FullName -Force -ErrorAction Stop
}

function Remove-StagedReleaseArtifacts {
  param([Parameter(Mandatory = $true)][string]$SidecarDir)

  if (-not (Test-Path -LiteralPath $SidecarDir)) { return }
  Assert-FixedLocalPathChain -Path $SidecarDir -Label "The staged release-artifact directory" -RequireExisting
  $directory = Get-Item -LiteralPath $SidecarDir -Force
  if (-not $directory.PSIsContainer -or (($directory.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "The staged release-artifact path must be a real directory: $SidecarDir"
  }
  foreach ($pattern in @(
      "code-hangar-elevated-*.exe",
      "code-hangar-mcp-*.exe",
      $script:CodeHangarReleaseManifestFileName
    )) {
    foreach ($item in @(Get-ChildItem -LiteralPath $SidecarDir -Filter $pattern -File -Force -ErrorAction Stop)) {
      if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to remove a reparse staged release artifact: $($item.FullName)"
      }
      Remove-Item -LiteralPath $item.FullName -Force -ErrorAction Stop
    }
  }
}

function Assert-ExactObjectPropertyNames {
  param(
    [Parameter(Mandatory = $true)]$Object,
    [Parameter(Mandatory = $true)][string[]]$Expected,
    [Parameter(Mandatory = $true)][string]$Label
  )
  if ($null -eq $Object) { throw "$Label is missing." }
  $actual = @($Object.PSObject.Properties.Name | Sort-Object)
  $wanted = @($Expected | Sort-Object)
  if (($actual -join "`n") -cne ($wanted -join "`n")) {
    throw "$Label has unexpected or missing fields (found: $($actual -join ', '))."
  }
}

function ConvertTo-SigningReceiptArtifact {
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [Parameter(Mandatory = $true)][string]$FileName,
    [switch]$IncludeAuthenticodeImageSha256
  )
  $artifact = [ordered]@{
    file_name = $FileName
    length = [long]$Evidence.Length
    sha256 = [string]$Evidence.Sha256
  }
  if ($IncludeAuthenticodeImageSha256) {
    $lockedEvidence = Get-CodeHangarAuthenticodeImageEvidence `
      -Path $Evidence.Path `
      -Label "Prepared $FileName"
    if ($lockedEvidence.Length -ne [long]$Evidence.Length -or
        $lockedEvidence.Sha256 -cne [string]$Evidence.Sha256) {
      throw "Prepared $FileName changed after its exact-copy evidence was recorded; refusing to write an ambiguous signing receipt."
    }
    $artifact.authenticode_image_sha256 = $lockedEvidence.ImageSha256
  }
  return $artifact
}

function New-CodeHangarFrontendSnapshot {
  param(
    [Parameter(Mandatory = $true)][string]$SigningDirectory,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$FrontendDistPath
  )

  Assert-FixedLocalPathChain -Path $SigningDirectory -Label "The signing-preparation directory" -RequireExisting
  $snapshotDirectory = Join-Path $SigningDirectory $script:CodeHangarFrontendSnapshotDirectoryName
  $manifestPath = Join-Path $SigningDirectory $script:CodeHangarFrontendSnapshotManifestFileName
  if (Test-Path -LiteralPath $snapshotDirectory) {
    throw "Prepared frontend snapshot directory already exists; refusing to overwrite it: $snapshotDirectory"
  }
  if (Test-Path -LiteralPath $manifestPath) {
    throw "Prepared frontend snapshot manifest already exists; refusing to overwrite it: $manifestPath"
  }
  $copy = Copy-CanonicalReleaseTree `
    -SourceRoot $FrontendDistPath `
    -DestinationRoot $snapshotDirectory `
    -Schema $script:CodeHangarFrontendSnapshotSchema `
    -Label "$Edition prepared frontend snapshot"
  $manifest = [ordered]@{
    schema = $script:CodeHangarFrontendSnapshotSchema
    edition = $Edition
    directory_name = $script:CodeHangarFrontendSnapshotDirectoryName
    file_count = [long]$copy.Destination.Count
    tree_sha256 = [string]$copy.Destination.Sha256
  }
  $manifestBytes = [System.Text.UTF8Encoding]::new($false).GetBytes(($manifest | ConvertTo-Json -Depth 5 -Compress))
  $stream = [System.IO.FileStream]::new($manifestPath, 'CreateNew', 'Write', 'None')
  try {
    $stream.Write($manifestBytes, 0, $manifestBytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
  $manifestEvidence = Get-StableReleaseArtifactEvidence -Path $manifestPath -Label "The prepared frontend snapshot manifest"
  return [pscustomobject]@{
    Directory = $snapshotDirectory
    Manifest = $manifestEvidence
    Tree = $copy.Destination
  }
}

function Assert-CodeHangarFrontendSnapshotReceipt {
  param(
    [Parameter(Mandatory = $true)]$Frontend,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$Label
  )

  Assert-ExactObjectPropertyNames -Object $Frontend -Expected @("directory_name", "manifest", "tree") -Label $Label
  if ([string]$Frontend.directory_name -cne $script:CodeHangarFrontendSnapshotDirectoryName) {
    throw "$Label directory name must be exactly $script:CodeHangarFrontendSnapshotDirectoryName."
  }
  Assert-SigningReceiptArtifact `
    -Artifact $Frontend.manifest `
    -ExpectedFileName $script:CodeHangarFrontendSnapshotManifestFileName `
    -Label "$Label manifest"
  Assert-ExactObjectPropertyNames -Object $Frontend.tree -Expected @("file_count", "sha256") -Label "$Label tree"
  if ([string]$Frontend.tree.file_count -cnotmatch '^[1-9][0-9]*$') {
    throw "$Label tree file count must be a positive decimal integer."
  }
  if ([string]$Frontend.tree.sha256 -cnotmatch '^[0-9a-f]{64}$') {
    throw "$Label tree SHA-256 must be 64 lowercase hexadecimal characters."
  }
}

function Read-AndValidateCodeHangarFrontendSnapshot {
  param(
    [Parameter(Mandatory = $true)][string]$SigningDirectory,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)]$FrontendReceipt
  )

  Assert-CodeHangarFrontendSnapshotReceipt -Frontend $FrontendReceipt -Edition $Edition -Label "The prepared frontend snapshot receipt"
  $directory = [System.IO.Path]::GetFullPath($SigningDirectory)
  Assert-FixedLocalPathChain -Path $directory -Label "The signing-preparation directory" -RequireExisting
  $snapshotDirectory = Join-Path $directory $script:CodeHangarFrontendSnapshotDirectoryName
  $manifestPath = Assert-RegularReleaseFile `
    -Path (Join-Path $directory $script:CodeHangarFrontendSnapshotManifestFileName) `
    -Label "The prepared frontend snapshot manifest"
  $manifestStream = $null
  $manifestReader = $null
  try {
    $manifestStream = [System.IO.FileStream]::new(
      $manifestPath,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    $manifestLength = $manifestStream.Length
    if ($manifestLength -le 0 -or $manifestLength -gt 64KB) {
      throw "The prepared frontend snapshot manifest must be non-empty and no larger than 64 KiB."
    }
    $manifestSha256 = (Get-LockedStreamSha256 -Stream $manifestStream).ToLowerInvariant()
    if ($manifestLength -ne [long]$FrontendReceipt.manifest.length -or
        $manifestSha256 -cne [string]$FrontendReceipt.manifest.sha256) {
      throw "The prepared frontend snapshot manifest no longer matches the signing receipt."
    }
    $manifestReader = [System.IO.StreamReader]::new(
      $manifestStream,
      [System.Text.UTF8Encoding]::new($false, $true),
      $false,
      4096,
      $true
    )
    try {
      $manifest = $manifestReader.ReadToEnd() | ConvertFrom-Json -DateKind String
    } catch {
      throw "The prepared frontend snapshot manifest is not strict UTF-8 JSON: $($_.Exception.Message)"
    }
    if ($manifestStream.Length -ne $manifestLength) {
      throw "The prepared frontend snapshot manifest changed while it was read."
    }
    $manifestEvidence = [pscustomobject]@{
      Path = $manifestPath
      FileName = [System.IO.Path]::GetFileName($manifestPath)
      Length = [long]$manifestLength
      Sha256 = $manifestSha256
    }
  } finally {
    if ($null -ne $manifestReader) { $manifestReader.Dispose() }
    if ($null -ne $manifestStream) { $manifestStream.Dispose() }
  }
  Assert-ExactObjectPropertyNames -Object $manifest -Expected @("schema", "edition", "directory_name", "file_count", "tree_sha256") -Label "The prepared frontend snapshot manifest"
  if ([string]$manifest.schema -cne $script:CodeHangarFrontendSnapshotSchema) { throw "The prepared frontend snapshot manifest schema is unsupported." }
  if ([string]$manifest.edition -cne $Edition) { throw "The prepared frontend snapshot manifest edition does not match $Edition." }
  if ([string]$manifest.directory_name -cne $script:CodeHangarFrontendSnapshotDirectoryName) {
    throw "The prepared frontend snapshot manifest directory name is invalid."
  }
  if ([string]$manifest.file_count -cnotmatch '^[1-9][0-9]*$' -or
      [long]$manifest.file_count -ne [long]$FrontendReceipt.tree.file_count) {
    throw "The prepared frontend snapshot manifest file count does not match the signing receipt."
  }
  if ([string]$manifest.tree_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
      [string]$manifest.tree_sha256 -cne [string]$FrontendReceipt.tree.sha256) {
    throw "The prepared frontend snapshot manifest tree SHA-256 does not match the signing receipt."
  }
  $tree = Assert-CanonicalReleaseTreeEvidence `
    -Root $snapshotDirectory `
    -Schema $script:CodeHangarFrontendSnapshotSchema `
    -Label "The prepared frontend snapshot" `
    -ExpectedCount ([long]$FrontendReceipt.tree.file_count) `
    -ExpectedSha256 ([string]$FrontendReceipt.tree.sha256)
  return [pscustomobject]@{
    Directory = $snapshotDirectory
    Manifest = $manifestEvidence
    Tree = $tree
  }
}

function Assert-CodeHangarFrontendSnapshotState {
  param(
    [Parameter(Mandatory = $true)]$FrontendSnapshot,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $manifest = Get-StableReleaseArtifactEvidence -Path $FrontendSnapshot.Manifest.Path -Label "$Label manifest"
  if ($manifest.Length -ne [long]$FrontendSnapshot.Manifest.Length -or
      $manifest.Sha256 -cne [string]$FrontendSnapshot.Manifest.Sha256) {
    throw "$Label manifest no longer matches the receipt-bound bytes."
  }
  return (Assert-CanonicalReleaseTreeEvidence `
    -Root $FrontendSnapshot.Directory `
    -Schema $script:CodeHangarFrontendSnapshotSchema `
    -Label $Label `
    -ExpectedCount ([long]$FrontendSnapshot.Tree.Count) `
    -ExpectedSha256 ([string]$FrontendSnapshot.Tree.Sha256))
}

function Open-CodeHangarFrontendSnapshotReadLocks {
  param(
    [Parameter(Mandatory = $true)]$FrontendSnapshot,
    [Parameter(Mandatory = $true)][string]$Label
  )

  [void](Assert-CodeHangarFrontendSnapshotState -FrontendSnapshot $FrontendSnapshot -Label $Label)
  $treeLocks = $null
  $manifestLock = $null
  try {
    $treeLocks = Open-CanonicalReleaseTreeReadLocks `
      -Root $FrontendSnapshot.Directory `
      -Schema $script:CodeHangarFrontendSnapshotSchema `
      -Label $Label `
      -ExpectedCount ([long]$FrontendSnapshot.Tree.Count) `
      -ExpectedSha256 ([string]$FrontendSnapshot.Tree.Sha256)
    $manifestLock = Open-ReleaseArtifactReadLock -Path $FrontendSnapshot.Manifest.Path -Label "$Label manifest"
    [void](Assert-CodeHangarFrontendSnapshotState -FrontendSnapshot $FrontendSnapshot -Label $Label)
    return [pscustomobject]@{
      TreeLocks = $treeLocks.Locks
      ManifestLock = $manifestLock
    }
  } catch {
    if ($null -ne $manifestLock) { $manifestLock.Dispose() }
    if ($null -ne $treeLocks) { Close-ReleaseArtifactLocks -Locks $treeLocks.Locks }
    throw
  }
}

function Write-CodeHangarSigningReceipt {
  param(
    [Parameter(Mandatory = $true)][string]$SigningDirectory,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$TargetTriple,
    [Parameter(Mandatory = $true)][string]$PublicBlobHex,
    [Parameter(Mandatory = $true)][string]$CargoLockSha256,
    [Parameter(Mandatory = $true)][string]$BundleContractSha256,
    [Parameter(Mandatory = $true)][string]$SourceCommit,
    [Parameter(Mandatory = $true)][string]$SourceTree,
    [Parameter(Mandatory = $true)]$ParentEvidence,
    [Parameter(Mandatory = $true)]$HelperEvidence,
    [Parameter(Mandatory = $true)]$VerifierEvidence,
    [Parameter(Mandatory = $true)]$FrontendSnapshot,
    $McpEvidence
  )

  Assert-FixedLocalPathChain -Path $SigningDirectory -Label "The signing-preparation directory" -RequireExisting
  if ($SourceCommit -cnotmatch '^[0-9a-f]{40,64}$' -or $SourceTree -cnotmatch '^[0-9a-f]{40,64}$') {
    throw "The signing receipt requires canonical lowercase Git commit/tree identities."
  }
  $receiptPath = Join-Path $SigningDirectory $script:CodeHangarSigningReceiptFileName
  if (Test-Path -LiteralPath $receiptPath) {
    throw "Signing receipt already exists; refusing to overwrite it: $receiptPath"
  }
  [void](Assert-CodeHangarFrontendSnapshotState -FrontendSnapshot $FrontendSnapshot -Label "The prepared frontend snapshot")
  $artifacts = [ordered]@{
    parent = ConvertTo-SigningReceiptArtifact -Evidence $ParentEvidence -FileName "code-hangar-desktop.exe" -IncludeAuthenticodeImageSha256
    helper = ConvertTo-SigningReceiptArtifact -Evidence $HelperEvidence -FileName "code-hangar-elevated.exe" -IncludeAuthenticodeImageSha256
    verifier = ConvertTo-SigningReceiptArtifact -Evidence $VerifierEvidence -FileName "code-hangar-release-verify.exe"
  }
  if ($Edition -eq "Connector") {
    if ($null -eq $McpEvidence) { throw "Connector signing preparation requires MCP evidence." }
    $artifacts.mcp = ConvertTo-SigningReceiptArtifact -Evidence $McpEvidence -FileName "code-hangar-mcp.exe"
  } elseif ($null -ne $McpEvidence) {
    throw "Local signing preparation must not contain MCP evidence."
  }
  $frontend = [ordered]@{
    directory_name = $script:CodeHangarFrontendSnapshotDirectoryName
    manifest = ConvertTo-SigningReceiptArtifact `
      -Evidence $FrontendSnapshot.Manifest `
      -FileName $script:CodeHangarFrontendSnapshotManifestFileName
    tree = [ordered]@{
      file_count = [long]$FrontendSnapshot.Tree.Count
      sha256 = [string]$FrontendSnapshot.Tree.Sha256
    }
  }
  $receipt = [ordered]@{
    schema = $script:CodeHangarSigningReceiptSchema
    edition = $Edition
    version = $Version
    target_triple = $TargetTriple
    release_root_public_blob_hex = $PublicBlobHex
    cargo_lock_sha256 = $CargoLockSha256.ToLowerInvariant()
    bundle_contract_sha256 = $BundleContractSha256.ToLowerInvariant()
    source = [ordered]@{
      git_commit = $SourceCommit
      git_tree = $SourceTree
      source_tree_dirty = $false
    }
    prepared_at_utc = [datetime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.fffffffZ", [System.Globalization.CultureInfo]::InvariantCulture)
    frontend = $frontend
    artifacts = $artifacts
  }
  $json = $receipt | ConvertTo-Json -Depth 8
  $stream = [System.IO.FileStream]::new($receiptPath, 'CreateNew', 'Write', 'None')
  try {
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
  return Get-StableReleaseArtifactEvidence -Path $receiptPath -Label "The signing receipt"
}

function Assert-SigningReceiptArtifact {
  param(
    [Parameter(Mandatory = $true)]$Artifact,
    [Parameter(Mandatory = $true)][string]$ExpectedFileName,
    [Parameter(Mandatory = $true)][string]$Label,
    [switch]$RequireAuthenticodeImageSha256
  )
  $expectedProperties = if ($RequireAuthenticodeImageSha256) {
    @("file_name", "length", "sha256", "authenticode_image_sha256")
  } else {
    @("file_name", "length", "sha256")
  }
  Assert-ExactObjectPropertyNames -Object $Artifact -Expected $expectedProperties -Label $Label
  if ([string]$Artifact.file_name -cne $ExpectedFileName) {
    throw "$Label file name must be exactly $ExpectedFileName."
  }
  if ([long]$Artifact.length -le 0) { throw "$Label length must be positive." }
  if ([string]$Artifact.sha256 -cnotmatch '^[0-9a-f]{64}$') {
    throw "$Label SHA-256 must be 64 lowercase hexadecimal characters."
  }
  if ($RequireAuthenticodeImageSha256 -and
      [string]$Artifact.authenticode_image_sha256 -cnotmatch '^[0-9a-f]{64}$') {
    throw "$Label Authenticode image SHA-256 must be 64 lowercase hexadecimal characters."
  }
}

function Read-AndValidateCodeHangarSigningReceipt {
  param(
    [Parameter(Mandatory = $true)][string]$SigningDirectory,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$ExpectedVersion,
    [Parameter(Mandatory = $true)][string]$ExpectedTargetTriple,
    [Parameter(Mandatory = $true)][string]$ExpectedPublicBlobHex,
    [Parameter(Mandatory = $true)][string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory = $true)][string]$ExpectedBundleContractSha256,
    [Parameter(Mandatory = $true)][string]$ExpectedSourceCommit,
    [Parameter(Mandatory = $true)][string]$ExpectedSourceTree,
    [Parameter(Mandatory = $true)][string]$ExpectedReceiptSha256
  )

  if (-not [System.IO.Path]::IsPathFullyQualified($SigningDirectory)) {
    throw "-SigningDirectory must be fully qualified."
  }
  $directory = [System.IO.Path]::GetFullPath($SigningDirectory)
  Assert-FixedLocalPathChain -Path $directory -Label "The signing-preparation directory" -RequireExisting
  $receiptPath = Assert-RegularReleaseFile `
    -Path (Join-Path $directory $script:CodeHangarSigningReceiptFileName) `
    -Label "The signing receipt"
  $receiptStream = $null
  $receiptReader = $null
  try {
    # Hash and parse the same write/delete-denying open file. Closing the hash
    # handle before Get-Content would leave a replacement window in the very
    # check that anchors the preparation directory to an externally recorded
    # -ExpectedSigningReceiptSha256.
    $receiptStream = [System.IO.FileStream]::new(
      $receiptPath,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    $receiptLength = $receiptStream.Length
    if ($receiptLength -le 0 -or $receiptLength -gt 64KB) {
      throw "The signing receipt must be non-empty and no larger than 64 KiB."
    }
    $receiptSha256 = (Get-LockedStreamSha256 -Stream $receiptStream).ToLowerInvariant()
    if ($ExpectedReceiptSha256 -cnotmatch '^[0-9A-Fa-f]{64}$' -or
        $receiptSha256 -cne $ExpectedReceiptSha256.ToLowerInvariant()) {
      throw "The signing receipt does not match -ExpectedSigningReceiptSha256."
    }
    $receiptReader = [System.IO.StreamReader]::new(
      $receiptStream,
      [System.Text.UTF8Encoding]::new($false, $true),
      $false,
      4096,
      $true
    )
    try {
      $receipt = $receiptReader.ReadToEnd() | ConvertFrom-Json -DateKind String
    } catch {
      throw "The signing receipt is not strict UTF-8 JSON: $($_.Exception.Message)"
    }
    if ($receiptStream.Length -ne $receiptLength) {
      throw "The signing receipt changed while it was read."
    }
    $receiptEvidence = [pscustomobject]@{
      Path = $receiptPath
      FileName = [System.IO.Path]::GetFileName($receiptPath)
      Length = [long]$receiptLength
      Sha256 = $receiptSha256
    }
  } finally {
    if ($null -ne $receiptReader) { $receiptReader.Dispose() }
    if ($null -ne $receiptStream) { $receiptStream.Dispose() }
  }
  Assert-ExactObjectPropertyNames -Object $receipt -Expected @(
    "schema", "edition", "version", "target_triple", "release_root_public_blob_hex",
    "cargo_lock_sha256", "bundle_contract_sha256", "source", "prepared_at_utc", "frontend", "artifacts"
  ) -Label "The signing receipt"
  if ([string]$receipt.schema -cne $script:CodeHangarSigningReceiptSchema) { throw "The signing receipt schema is unsupported." }
  if ([string]$receipt.edition -cne $Edition) { throw "The signing receipt edition does not match $Edition." }
  if ([string]$receipt.version -cne $ExpectedVersion) { throw "The signing receipt version does not match $ExpectedVersion." }
  if ([string]$receipt.target_triple -cne $ExpectedTargetTriple) { throw "The signing receipt target triple does not match $ExpectedTargetTriple." }
  if ([string]$receipt.release_root_public_blob_hex -cne $ExpectedPublicBlobHex) { throw "The signing receipt release root does not match the explicit public blob." }
  if ([string]$receipt.cargo_lock_sha256 -cne $ExpectedCargoLockSha256.ToLowerInvariant()) { throw "Cargo.lock changed after PrepareSigning; rerun preparation from the intended source state." }
  if ([string]$receipt.bundle_contract_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
      [string]$receipt.bundle_contract_sha256 -cne $ExpectedBundleContractSha256.ToLowerInvariant()) {
    throw "The canonical bundle contract changed after PrepareSigning; rerun preparation before bundling."
  }
  Assert-ExactObjectPropertyNames -Object $receipt.source -Expected @(
    "git_commit", "git_tree", "source_tree_dirty"
  ) -Label "The signing receipt source identity"
  if ([bool]$receipt.source.source_tree_dirty -or
      [string]$receipt.source.git_commit -cnotmatch '^[0-9a-f]{40,64}$' -or
      [string]$receipt.source.git_tree -cnotmatch '^[0-9a-f]{40,64}$' -or
      [string]$receipt.source.git_commit -cne $ExpectedSourceCommit -or
      [string]$receipt.source.git_tree -cne $ExpectedSourceTree) {
    throw "The signing receipt source commit/tree does not match the exact clean release source."
  }
  $parsedPreparedAt = [datetime]::MinValue
  if (-not [datetime]::TryParseExact(
      [string]$receipt.prepared_at_utc,
      "yyyy-MM-ddTHH:mm:ss.fffffffZ",
      [System.Globalization.CultureInfo]::InvariantCulture,
      [System.Globalization.DateTimeStyles]::AssumeUniversal,
      [ref]$parsedPreparedAt
    )) {
    throw "The signing receipt timestamp is invalid."
  }
  if ($parsedPreparedAt.ToUniversalTime() -gt [datetime]::UtcNow.AddMinutes(5)) {
    throw "The signing receipt timestamp is implausibly in the future."
  }

  $artifactNames = if ($Edition -eq "Connector") { @("parent", "helper", "verifier", "mcp") } else { @("parent", "helper", "verifier") }
  Assert-ExactObjectPropertyNames -Object $receipt.artifacts -Expected $artifactNames -Label "The signing receipt artifacts"
  Assert-SigningReceiptArtifact -Artifact $receipt.artifacts.parent -ExpectedFileName "code-hangar-desktop.exe" -Label "Prepared parent" -RequireAuthenticodeImageSha256
  Assert-SigningReceiptArtifact -Artifact $receipt.artifacts.helper -ExpectedFileName "code-hangar-elevated.exe" -Label "Prepared helper" -RequireAuthenticodeImageSha256
  Assert-SigningReceiptArtifact -Artifact $receipt.artifacts.verifier -ExpectedFileName "code-hangar-release-verify.exe" -Label "Prepared verifier"
  if ($Edition -eq "Connector") {
    Assert-SigningReceiptArtifact -Artifact $receipt.artifacts.mcp -ExpectedFileName "code-hangar-mcp.exe" -Label "Prepared MCP"
  }

  $verifierPath = Join-Path $directory "code-hangar-release-verify.exe"
  $verifierEvidence = Get-StableReleaseArtifactEvidence -Path $verifierPath -Label "The prepared release verifier" -RequirePe
  if ($verifierEvidence.Length -ne [long]$receipt.artifacts.verifier.length -or
      $verifierEvidence.Sha256 -cne [string]$receipt.artifacts.verifier.sha256) {
    throw "The prepared release verifier no longer matches the PrepareSigning receipt."
  }
  $mcpEvidence = $null
  if ($Edition -eq "Connector") {
    $mcpPath = Join-Path $directory "code-hangar-mcp.exe"
    $mcpEvidence = Get-StableReleaseArtifactEvidence -Path $mcpPath -Label "The prepared MCP sidecar" -RequirePe
    if ($mcpEvidence.Length -ne [long]$receipt.artifacts.mcp.length -or
        $mcpEvidence.Sha256 -cne [string]$receipt.artifacts.mcp.sha256) {
      throw "The prepared MCP sidecar no longer matches the PrepareSigning receipt."
    }
  }
  $frontend = Read-AndValidateCodeHangarFrontendSnapshot `
    -SigningDirectory $directory `
    -Edition $Edition `
    -FrontendReceipt $receipt.frontend
  return [pscustomobject]@{
    Directory = $directory
    Path = $receiptPath
    Evidence = $receiptEvidence
    Receipt = $receipt
    Frontend = $frontend
    Verifier = $verifierEvidence
    Mcp = $mcpEvidence
  }
}

function Assert-PostSigningArtifactMatchesReceipt {
  param(
    [Parameter(Mandatory = $true)]$SignedEvidence,
    [Parameter(Mandatory = $true)]$PreparedReceiptArtifact,
    [Parameter(Mandatory = $true)][string]$Label
  )

  Assert-SigningReceiptArtifact `
    -Artifact $PreparedReceiptArtifact `
    -ExpectedFileName $SignedEvidence.FileName `
    -Label "$Label prepared receipt artifact" `
    -RequireAuthenticodeImageSha256
  if ($SignedEvidence.Length -le [long]$PreparedReceiptArtifact.length -or
      $SignedEvidence.Sha256 -ceq [string]$PreparedReceiptArtifact.sha256) {
    throw "$Label does not show a byte-changing post-PrepareSigning Authenticode step."
  }
  $lockedSignedEvidence = Get-CodeHangarAuthenticodeImageEvidence -Path $SignedEvidence.Path -Label $Label
  if ($lockedSignedEvidence.Length -ne [long]$SignedEvidence.Length -or
      $lockedSignedEvidence.Sha256 -cne [string]$SignedEvidence.Sha256) {
    throw "$Label changed after its signed-byte evidence was recorded."
  }
  if ($lockedSignedEvidence.ImageSha256 -cne [string]$PreparedReceiptArtifact.authenticode_image_sha256) {
    throw "$Label Authenticode image SHA-256 no longer matches the prepared receipt; refusing a substituted post-sign binary."
  }
  return $lockedSignedEvidence.ImageSha256
}

function Invoke-CodeHangarReleaseVerifier {
  param(
    [Parameter(Mandatory = $true)][string]$VerifierPath,
    [Parameter(Mandatory = $true)][string]$InstallDirectory,
    [Parameter(Mandatory = $true)][string]$ExpectedParentSha256,
    [Parameter(Mandatory = $true)][string]$ExpectedHelperSha256
  )

  $verifier = Assert-RegularReleaseFile -Path $VerifierPath -Label "The receipt-bound release verifier" -RequirePe
  Assert-FixedLocalPathChain -Path $InstallDirectory -Label "The signed installation proof directory" -RequireExisting
  $start = [System.Diagnostics.ProcessStartInfo]::new()
  $start.FileName = $verifier
  $start.ArgumentList.Add([System.IO.Path]::GetFullPath($InstallDirectory))
  $start.UseShellExecute = $false
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  $start.CreateNoWindow = $true
  $process = [System.Diagnostics.Process]::Start($start)
  $stdout = $process.StandardOutput.ReadToEnd().Trim()
  $stderr = $process.StandardError.ReadToEnd().Trim()
  $process.WaitForExit()
  if ($process.ExitCode -ne 0) {
    throw "Receipt-bound release verification failed with exit code $($process.ExitCode): $stderr"
  }
  try {
    $proof = $stdout | ConvertFrom-Json
  } catch {
    throw "Receipt-bound release verifier returned invalid JSON: $stdout"
  }
  Assert-ExactObjectPropertyNames -Object $proof -Expected @(
    "release_id", "manifest_sha256", "parent_sha256", "helper_sha256"
  ) -Label "The release verification proof"
  foreach ($field in @("release_id", "manifest_sha256", "parent_sha256", "helper_sha256")) {
    if ([string]$proof.$field -cnotmatch '^[0-9a-f]{64}$') {
      throw "Release verification proof field $field is not a lowercase SHA-256/release identifier."
    }
  }
  if ([string]$proof.parent_sha256 -cne $ExpectedParentSha256.ToLowerInvariant() -or
      [string]$proof.helper_sha256 -cne $ExpectedHelperSha256.ToLowerInvariant()) {
    throw "Release verifier proof does not match the exact staged parent/helper bytes."
  }
  return $proof
}

function Open-ReleaseArtifactReadLock {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label
  )
  $fullPath = Assert-RegularReleaseFile -Path $Path -Label $Label
  return [System.IO.FileStream]::new(
    $fullPath,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read,
    [System.IO.FileShare]::Read
  )
}

function New-GeneratedReleaseDirectory {
  param(
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition
  )
  $generatedRoot = Join-Path $RepoRoot ".local\packaging-generated"
  if (-not (Test-Path -LiteralPath $generatedRoot)) {
    [void][System.IO.Directory]::CreateDirectory($generatedRoot)
  }
  Assert-FixedLocalPathChain -Path $generatedRoot -Label "The generated packaging root" -RequireExisting
  $runDirectory = Join-Path $generatedRoot ("release-signed-{0}-{1}" -f $Edition.ToLowerInvariant(), [guid]::NewGuid().ToString("N"))
  [void][System.IO.Directory]::CreateDirectory($runDirectory)
  Assert-FixedLocalPathChain -Path $runDirectory -Label "The generated signed-release run" -RequireExisting
  return $runDirectory
}

function Remove-GeneratedReleaseDirectory {
  param(
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][string]$RunDirectory
  )
  $generatedRoot = [System.IO.Path]::GetFullPath((Join-Path $RepoRoot ".local\packaging-generated"))
  $resolved = [System.IO.Path]::GetFullPath($RunDirectory)
  if (-not ([System.IO.Path]::GetDirectoryName($resolved)).Equals($generatedRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not ([System.IO.Path]::GetFileName($resolved)).StartsWith("release-signed-", [System.StringComparison]::Ordinal)) {
    throw "Refusing unsafe generated signed-release cleanup: $resolved"
  }
  if (Test-Path -LiteralPath $resolved) {
    foreach ($item in @(Get-ChildItem -LiteralPath $resolved -Recurse -Force -ErrorAction Stop)) {
      if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing signed-release cleanup containing a reparse point: $($item.FullName)"
      }
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
  }
}

function Assert-RealReleaseDirectory {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $fullPath = [System.IO.Path]::GetFullPath($Path)
  Assert-FixedLocalPathChain -Path $fullPath -Label $Label -RequireExisting
  $item = Get-Item -LiteralPath $fullPath -Force
  if (-not $item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "$Label must be a real non-reparse directory: $fullPath"
  }
  return $fullPath
}

function Enter-CodeHangarFrontendSnapshotBundleContext {
  param(
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)]$FrontendSnapshot,
    [Parameter(Mandatory = $true)][string]$RunDirectory
  )

  $desktopRoot = Assert-RealReleaseDirectory -Path (Join-Path $RepoRoot "apps\desktop") -Label "The desktop frontend worktree"
  $frontendDist = Assert-RealReleaseDirectory -Path (Join-Path $desktopRoot "dist") -Label "The mutable worktree frontend dist"
  $runDirectoryFull = Assert-RealReleaseDirectory -Path $RunDirectory -Label "The generated signed-release run"
  if (-not ([System.IO.Path]::GetPathRoot($frontendDist)).Equals([System.IO.Path]::GetPathRoot($runDirectoryFull), [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "The frontend snapshot restore run must stay on the worktree volume."
  }
  $priorDirectory = Join-Path $runDirectoryFull "frontend-dist-prior"
  $stagedDirectory = Join-Path $runDirectoryFull "frontend-dist-staged"
  $bundleOutputDirectory = Join-Path $runDirectoryFull "frontend-dist-bundle-output"
  foreach ($path in @($priorDirectory, $stagedDirectory, $bundleOutputDirectory)) {
    if (Test-Path -LiteralPath $path) { throw "The generated frontend bundle workspace is not empty: $path" }
  }
  $priorTree = Get-CanonicalReleaseTreeEvidence `
    -Root $frontendDist `
    -Schema $script:CodeHangarFrontendSnapshotSchema `
    -Label "The pre-bundle worktree frontend dist"
  $context = [pscustomobject]@{
    FrontendDist = $frontendDist
    PriorDirectory = $priorDirectory
    StagedDirectory = $stagedDirectory
    BundleOutputDirectory = $bundleOutputDirectory
    PriorTree = $priorTree
    FrontendSnapshot = $FrontendSnapshot
    SnapshotTreeLocks = $null
    SnapshotManifestLock = $null
    PriorTreeLocks = $null
    ActiveTreeLocks = $null
    PriorWasMoved = $false
    ActiveWasInstalled = $false
    Restored = $false
  }
  try {
    $snapshotLocks = Open-CodeHangarFrontendSnapshotReadLocks `
      -FrontendSnapshot $FrontendSnapshot `
      -Label "The receipt-bound frontend snapshot"
    $context.SnapshotTreeLocks = $snapshotLocks.TreeLocks
    $context.SnapshotManifestLock = $snapshotLocks.ManifestLock

    [System.IO.Directory]::Move($frontendDist, $priorDirectory)
    $context.PriorWasMoved = $true
    $priorLocks = Open-CanonicalReleaseTreeReadLocks `
      -Root $priorDirectory `
      -Schema $script:CodeHangarFrontendSnapshotSchema `
      -Label "The preserved pre-bundle worktree frontend dist" `
      -ExpectedCount $priorTree.Count `
      -ExpectedSha256 $priorTree.Sha256
    $context.PriorTreeLocks = $priorLocks.Locks

    [void](Copy-CanonicalReleaseTree `
      -SourceRoot $FrontendSnapshot.Directory `
      -DestinationRoot $stagedDirectory `
      -Schema $script:CodeHangarFrontendSnapshotSchema `
      -Label "The receipt-bound frontend snapshot restore")
    [void](Assert-CodeHangarFrontendSnapshotState -FrontendSnapshot $FrontendSnapshot -Label "The receipt-bound frontend snapshot")
    [System.IO.Directory]::Move($stagedDirectory, $frontendDist)
    $context.ActiveWasInstalled = $true
    $activeLocks = Open-CanonicalReleaseTreeReadLocks `
      -Root $frontendDist `
      -Schema $script:CodeHangarFrontendSnapshotSchema `
      -Label "The restored receipt-bound worktree frontend dist" `
      -ExpectedCount $FrontendSnapshot.Tree.Count `
      -ExpectedSha256 $FrontendSnapshot.Tree.Sha256
    $context.ActiveTreeLocks = $activeLocks.Locks
    return $context
  } catch {
    try {
      Restore-CodeHangarFrontendSnapshotBundleContext -Context $context
    } catch {
      throw "Frontend snapshot restore setup failed and its rollback also failed: $($_.Exception.Message)"
    }
    throw
  }
}

function Assert-CodeHangarFrontendSnapshotBundleContextState {
  param([Parameter(Mandatory = $true)]$Context)

  [void](Assert-CodeHangarFrontendSnapshotState `
    -FrontendSnapshot $Context.FrontendSnapshot `
    -Label "The receipt-bound frontend snapshot")
  [void](Assert-CanonicalReleaseTreeEvidence `
    -Root $Context.FrontendDist `
    -Schema $script:CodeHangarFrontendSnapshotSchema `
    -Label "The restored receipt-bound worktree frontend dist" `
    -ExpectedCount $Context.FrontendSnapshot.Tree.Count `
    -ExpectedSha256 $Context.FrontendSnapshot.Tree.Sha256)
  [void](Assert-CanonicalReleaseTreeEvidence `
    -Root $Context.PriorDirectory `
    -Schema $script:CodeHangarFrontendSnapshotSchema `
    -Label "The preserved pre-bundle worktree frontend dist" `
    -ExpectedCount $Context.PriorTree.Count `
    -ExpectedSha256 $Context.PriorTree.Sha256)
}

function Restore-CodeHangarFrontendSnapshotBundleContext {
  param([Parameter(Mandatory = $true)]$Context)

  if ($Context.Restored) { return }
  try {
    Close-ReleaseArtifactLocks -Locks $Context.ActiveTreeLocks
    $Context.ActiveTreeLocks = $null
    Close-ReleaseArtifactLocks -Locks $Context.PriorTreeLocks
    $Context.PriorTreeLocks = $null
    if (-not $Context.PriorWasMoved) {
      $Context.Restored = $true
      return
    }
    if (Test-Path -LiteralPath $Context.FrontendDist) {
      [void](Assert-RealReleaseDirectory -Path $Context.FrontendDist -Label "The temporary receipt-bound frontend dist")
      if (Test-Path -LiteralPath $Context.BundleOutputDirectory) {
        throw "The generated frontend bundle-output directory already exists: $($Context.BundleOutputDirectory)"
      }
      [System.IO.Directory]::Move($Context.FrontendDist, $Context.BundleOutputDirectory)
    }
    [void](Assert-RealReleaseDirectory -Path $Context.PriorDirectory -Label "The preserved pre-bundle frontend dist")
    [System.IO.Directory]::Move($Context.PriorDirectory, $Context.FrontendDist)
    $Context.Restored = $true
    [void](Assert-CanonicalReleaseTreeEvidence `
      -Root $Context.FrontendDist `
      -Schema $script:CodeHangarFrontendSnapshotSchema `
      -Label "The restored pre-bundle worktree frontend dist" `
      -ExpectedCount $Context.PriorTree.Count `
      -ExpectedSha256 $Context.PriorTree.Sha256)
  } finally {
    if ($null -ne $Context.SnapshotManifestLock) {
      $Context.SnapshotManifestLock.Dispose()
      $Context.SnapshotManifestLock = $null
    }
    Close-ReleaseArtifactLocks -Locks $Context.SnapshotTreeLocks
    $Context.SnapshotTreeLocks = $null
  }
}

function Initialize-CodeHangarOfflinePackagingEnvironment {
  param([Parameter(Mandatory = $true)][string]$RepoRoot)

  Assert-FixedLocalPathChain -Path $RepoRoot -Label "The packaging worktree" -RequireExisting
  $repoTarget = [System.IO.Path]::GetFullPath((Join-Path $RepoRoot "target")).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  if (-not [string]::IsNullOrWhiteSpace($env:CARGO_BUILD_TARGET)) {
    throw "CARGO_BUILD_TARGET changes Cargo's output layout and is refused for release packaging. Clear it and retry."
  }
  if (-not [string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    $requested = $env:CARGO_TARGET_DIR
    if (-not [System.IO.Path]::IsPathRooted($requested)) { $requested = Join-Path $RepoRoot $requested }
    $requested = [System.IO.Path]::GetFullPath($requested).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    if (-not $requested.Equals($repoTarget, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "CARGO_TARGET_DIR must be this worktree's target directory ($repoTarget), not $requested."
    }
  }
  Assert-FixedLocalPathChain -Path $repoTarget -Label "The worktree target path"
  if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { throw "LOCALAPPDATA is required to locate Tauri's user cache." }
  $tauriCache = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA "tauri"))
  Assert-FixedLocalPathChain -Path $tauriCache -Label "The Tauri user cache" -RequireExisting
  $env:CARGO_TARGET_DIR = $repoTarget
  $env:CARGO_NET_OFFLINE = "true"
  $env:npm_config_offline = "true"
  return $repoTarget
}

function Invoke-CheckedExternalCommand {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [Parameter(Mandatory = $true)][string[]]$Arguments,
    [Parameter(Mandatory = $true)][string]$FailureLabel
  )
  & $FilePath @Arguments
  if ($LASTEXITCODE -ne 0) { throw "$FailureLabel failed with exit code $LASTEXITCODE." }
}

function Invoke-CodeHangarFrontendEditionCheck {
  param(
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$FailureLabel
  )

  $desktopRoot = Join-Path $RepoRoot "apps\desktop"
  Assert-FixedLocalPathChain -Path $desktopRoot -Label "The desktop frontend worktree" -RequireExisting
  Push-Location $desktopRoot
  try {
    Invoke-CheckedExternalCommand `
      -FilePath "node" `
      -Arguments @("../../scripts/check-frontend-edition.mjs", $Edition.ToLowerInvariant()) `
      -FailureLabel $FailureLabel
  } finally {
    Pop-Location
  }
}

function Assert-CodeHangarPackagingMode {
  param(
    [switch]$PreflightOnly,
    [switch]$PrepareSigning,
    [switch]$BundleSigned,
    [switch]$SelfTest
  )
  $selected = 0
  foreach ($mode in @($PreflightOnly, $PrepareSigning, $BundleSigned, $SelfTest)) {
    if ([bool]$mode) { $selected++ }
  }
  if ($selected -ne 1) {
    throw "Choose exactly one explicit mode: -PreflightOnly, -PrepareSigning, -BundleSigned or -SelfTest. The default invocation never creates an installer."
  }
}

function Invoke-CodeHangarPrepareSigning {
  param(
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][string]$TargetDir,
    [Parameter(Mandatory = $true)]$WebViewContext,
    [Parameter(Mandatory = $true)][string]$ReleaseRootPublicBlobHex,
    [Parameter(Mandatory = $true)][string]$SigningDirectory
  )

  $sourceIdentity = Get-CodeHangarCleanGitIdentity -RepoRoot $RepoRoot
  if (-not [System.IO.Path]::IsPathFullyQualified($SigningDirectory)) {
    throw "-SigningDirectory must be a fully qualified path."
  }
  $signingFull = [System.IO.Path]::GetFullPath($SigningDirectory)
  if (Test-Path -LiteralPath $signingFull) {
    throw "PrepareSigning requires a new, non-existing signing directory: $signingFull"
  }
  $signingParent = Split-Path -Parent $signingFull
  Assert-FixedLocalPathChain -Path $signingParent -Label "The signing-directory parent" -RequireExisting
  [void][System.IO.Directory]::CreateDirectory($signingFull)
  Assert-FixedLocalPathChain -Path $signingFull -Label "The new signing-preparation directory" -RequireExisting

  $targetRelease = Join-Path $TargetDir "release"
  $desktopPath = Join-Path $targetRelease "code-hangar-desktop.exe"
  $helperPath = Join-Path $targetRelease "code-hangar-elevated.exe"
  $verifierPath = Join-Path $targetRelease "code-hangar-release-verify.exe"
  $mcpPath = Join-Path $targetRelease "code-hangar-mcp.exe"
  $sidecarDir = Join-Path $RepoRoot "apps\desktop\src-tauri\binaries"
  if (-not (Test-Path -LiteralPath $sidecarDir)) { [void][System.IO.Directory]::CreateDirectory($sidecarDir) }
  Assert-FixedLocalPathChain -Path $sidecarDir -Label "The staged-sidecar path" -RequireExisting
  Remove-StagedReleaseArtifacts -SidecarDir $sidecarDir

  foreach ($artifact in @(
      @{ Path = $desktopPath; Label = "prior desktop release output" },
      @{ Path = $helperPath; Label = "prior elevated-helper release output" },
      @{ Path = $verifierPath; Label = "prior release-verifier output" }
    )) {
    Remove-ExactReleaseArtifact -Path $artifact.Path -Label $artifact.Label
  }
  if ($Edition -eq "Connector") {
    Remove-ExactReleaseArtifact -Path $mcpPath -Label "prior MCP release output"
  }

  $cargoLockPath = Join-Path $RepoRoot "Cargo.lock"
  $cargoLockHash = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
  $bundleContractHash = Get-CodeHangarBundleContractSha256 -RepoRoot $RepoRoot -Edition $Edition
  $desktopFeature = if ($Edition -eq "Connector") { "agent_automation" } else { "mutation" }
  Set-Location $RepoRoot
  Invoke-CheckedExternalCommand -FilePath "cargo" -Arguments @(
    "build", "--locked", "--offline", "-p", "code-hangar-desktop", "--release", "--features", $desktopFeature
  ) -FailureLabel "$Edition locked/offline desktop preparation"
  Invoke-CheckedExternalCommand -FilePath "cargo" -Arguments @(
    "build", "--locked", "--offline", "-p", "hangar-mutation", "--release", "--features", "mutation",
    "--bin", "code-hangar-elevated", "--bin", "code-hangar-release-verify"
  ) -FailureLabel "$Edition locked/offline helper/verifier preparation"
  if ($Edition -eq "Connector") {
    Invoke-CheckedExternalCommand -FilePath "cargo" -Arguments @(
      "build", "--locked", "--offline", "-p", "code-hangar-mcp", "--release"
    ) -FailureLabel "Connector locked/offline MCP preparation"
  }

  $tauriCmd = Join-Path $RepoRoot "node_modules\.bin\tauri.cmd"
  $compileConfigs = @(Get-TauriCompileConfigArguments -Edition $Edition -RepoRoot $RepoRoot -OverridePath $WebViewContext.OverridePath)
  Set-Location (Join-Path $RepoRoot "apps\desktop")
  $tauriArguments = @("build", "--no-bundle", "--no-sign", "--ci", "--features", $desktopFeature) + $compileConfigs
  Invoke-CheckedExternalCommand -FilePath $tauriCmd -Arguments $tauriArguments -FailureLabel "$Edition Tauri compile-only preparation"
  Set-Location $RepoRoot
  Invoke-CodeHangarFrontendEditionCheck `
    -RepoRoot $RepoRoot `
    -Edition $Edition `
    -FailureLabel "$Edition prepared frontend isolation check"

  $cargoLockHashAfter = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($cargoLockHashAfter -cne $cargoLockHash) {
    throw "Cargo.lock changed during PrepareSigning; refusing all prepared artifacts."
  }
  $bundleContractHashAfter = Get-CodeHangarBundleContractSha256 -RepoRoot $RepoRoot -Edition $Edition
  if ($bundleContractHashAfter -cne $bundleContractHash) {
    throw "The canonical bundle contract changed during PrepareSigning; refusing all prepared artifacts."
  }
  $sourceIdentityAfter = Get-CodeHangarCleanGitIdentity -RepoRoot $RepoRoot
  if ($sourceIdentityAfter.Commit -cne $sourceIdentity.Commit -or
      $sourceIdentityAfter.Tree -cne $sourceIdentity.Tree) {
    throw "The release source identity changed during PrepareSigning; refusing all prepared artifacts."
  }

  $parentBuilt = Get-StableReleaseArtifactEvidence -Path $desktopPath -Label "The prepared desktop parent" -RequirePe
  $helperBuilt = Get-StableReleaseArtifactEvidence -Path $helperPath -Label "The prepared elevated helper" -RequirePe
  $verifierBuilt = Get-StableReleaseArtifactEvidence -Path $verifierPath -Label "The prepared release verifier" -RequirePe
  if ($parentBuilt.Sha256 -ceq $helperBuilt.Sha256) { throw "Prepared parent and helper unexpectedly have identical bytes." }
  $mcpBuilt = $null
  if ($Edition -eq "Connector") {
    $mcpBuilt = Get-StableReleaseArtifactEvidence -Path $mcpPath -Label "The prepared MCP sidecar" -RequirePe
  } elseif (Test-Path -LiteralPath $mcpPath) {
    # A previous Connector binary in target/release is not part of Local's
    # receipt or bundle. The staged binaries directory was already cleared.
    Write-Verbose "Ignoring an unstaged target/release MCP artifact in the Local lane."
  }

  $parentCopy = Set-ExactReleaseArtifact -SourcePath $desktopPath -DestinationPath (Join-Path $signingFull "code-hangar-desktop.exe") -Label "Prepared parent signing copy" -ExpectedSha256 $parentBuilt.Sha256 -RequirePe
  $helperCopy = Set-ExactReleaseArtifact -SourcePath $helperPath -DestinationPath (Join-Path $signingFull "code-hangar-elevated.exe") -Label "Prepared helper signing copy" -ExpectedSha256 $helperBuilt.Sha256 -RequirePe
  $verifierCopy = Set-ExactReleaseArtifact -SourcePath $verifierPath -DestinationPath (Join-Path $signingFull "code-hangar-release-verify.exe") -Label "Prepared verifier continuity copy" -ExpectedSha256 $verifierBuilt.Sha256 -RequirePe
  $mcpCopy = $null
  if ($Edition -eq "Connector") {
    $mcpCopy = Set-ExactReleaseArtifact -SourcePath $mcpPath -DestinationPath (Join-Path $signingFull "code-hangar-mcp.exe") -Label "Prepared MCP continuity copy" -ExpectedSha256 $mcpBuilt.Sha256 -RequirePe
  }
  $frontendSnapshot = New-CodeHangarFrontendSnapshot `
    -SigningDirectory $signingFull `
    -Edition $Edition `
    -FrontendDistPath (Join-Path $RepoRoot "apps\desktop\dist")
  $version = [string]((Get-Content -LiteralPath (Join-Path $RepoRoot "package.json") -Raw | ConvertFrom-Json).version)
  $triple = Get-CodeHangarHostTargetTriple
  $receipt = Write-CodeHangarSigningReceipt `
    -SigningDirectory $signingFull `
    -Edition $Edition `
    -Version $version `
    -TargetTriple $triple `
    -PublicBlobHex $ReleaseRootPublicBlobHex `
    -CargoLockSha256 $cargoLockHash `
    -BundleContractSha256 $bundleContractHash `
    -SourceCommit $sourceIdentity.Commit `
    -SourceTree $sourceIdentity.Tree `
    -ParentEvidence $parentCopy `
    -HelperEvidence $helperCopy `
    -VerifierEvidence $verifierCopy `
    -FrontendSnapshot $frontendSnapshot `
    -McpEvidence $mcpCopy

  Write-Host ""
  Write-Host "$Edition PrepareSigning completed; no installer was created." -ForegroundColor Green
  Write-Host "Signing directory: $signingFull" -ForegroundColor Green
  Write-Host "Receipt SHA256: $($receipt.Sha256)" -ForegroundColor Green
  Write-Host "Owner gate: Authenticode-sign code-hangar-desktop.exe and code-hangar-elevated.exe externally, then create $script:CodeHangarReleaseManifestFileName with scripts/new-release-identity-manifest.ps1 using the printed receipt SHA-256 and edition." -ForegroundColor Yellow
  Write-Host "The receipt also binds the verified immutable frontend-dist snapshot ($($frontendSnapshot.Tree.Count) files, $($frontendSnapshot.Tree.Sha256))." -ForegroundColor Yellow
  Write-Host "Do not modify code-hangar-release-verify.exe or, for Connector, code-hangar-mcp.exe; BundleSigned binds them to this receipt." -ForegroundColor Yellow
}

function Invoke-CodeHangarBundleSigned {
  param(
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [Parameter(Mandatory = $true)][string]$TargetDir,
    [Parameter(Mandatory = $true)]$WebViewContext,
    [Parameter(Mandatory = $true)][string]$ReleaseRootPublicBlobHex,
    [Parameter(Mandatory = $true)][string]$SigningDirectory,
    [Parameter(Mandatory = $true)][string]$ExpectedSigningReceiptSha256,
    [Parameter(Mandatory = $true)][string]$SignedParentPath,
    [Parameter(Mandatory = $true)][string]$SignedHelperPath,
    [Parameter(Mandatory = $true)][string]$ReleaseManifestPath
  )

  $sourceIdentity = Get-CodeHangarCleanGitIdentity -RepoRoot $RepoRoot
  $version = [string]((Get-Content -LiteralPath (Join-Path $RepoRoot "package.json") -Raw | ConvertFrom-Json).version)
  $triple = Get-CodeHangarHostTargetTriple
  $cargoLockPath = Join-Path $RepoRoot "Cargo.lock"
  $cargoLockHash = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
  $bundleContractHash = Get-CodeHangarBundleContractSha256 -RepoRoot $RepoRoot -Edition $Edition
  $preparation = Read-AndValidateCodeHangarSigningReceipt `
    -SigningDirectory $SigningDirectory `
    -Edition $Edition `
    -ExpectedVersion $version `
    -ExpectedTargetTriple $triple `
    -ExpectedPublicBlobHex $ReleaseRootPublicBlobHex `
    -ExpectedCargoLockSha256 $cargoLockHash `
    -ExpectedBundleContractSha256 $bundleContractHash `
    -ExpectedSourceCommit $sourceIdentity.Commit `
    -ExpectedSourceTree $sourceIdentity.Tree `
    -ExpectedReceiptSha256 $ExpectedSigningReceiptSha256
  $expectedReleaseId = Get-CodeHangarReceiptBoundReleaseId `
    -Edition $Edition `
    -SigningReceiptSha256 $preparation.Evidence.Sha256

  $signedParent = Get-StableReleaseArtifactEvidence -Path $SignedParentPath -Label "The externally Authenticode-signed parent" -RequirePe
  $signedHelper = Get-StableReleaseArtifactEvidence -Path $SignedHelperPath -Label "The externally Authenticode-signed helper" -RequirePe
  $expectedSignedParent = Join-Path $preparation.Directory "code-hangar-desktop.exe"
  $expectedSignedHelper = Join-Path $preparation.Directory "code-hangar-elevated.exe"
  if (-not $signedParent.Path.Equals($expectedSignedParent, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not $signedHelper.Path.Equals($expectedSignedHelper, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Signed parent/helper must be the canonical in-place files in the receipt-bound SigningDirectory."
  }
  [void](Assert-PostSigningArtifactMatchesReceipt `
    -SignedEvidence $signedParent `
    -PreparedReceiptArtifact $preparation.Receipt.artifacts.parent `
    -Label "The externally Authenticode-signed parent")
  [void](Assert-PostSigningArtifactMatchesReceipt `
    -SignedEvidence $signedHelper `
    -PreparedReceiptArtifact $preparation.Receipt.artifacts.helper `
    -Label "The externally Authenticode-signed helper")
  if ($signedParent.Sha256 -ceq $signedHelper.Sha256) { throw "Signed parent and helper unexpectedly have identical bytes." }
  $manifestFull = Assert-RegularReleaseFile -Path $ReleaseManifestPath -Label "The RSA-PSS release identity manifest"
  if ([System.IO.Path]::GetFileName($manifestFull) -cne $script:CodeHangarReleaseManifestFileName) {
    throw "The release identity manifest leaf name must be exactly $script:CodeHangarReleaseManifestFileName."
  }
  $expectedManifest = Join-Path $preparation.Directory $script:CodeHangarReleaseManifestFileName
  if (-not $manifestFull.Equals($expectedManifest, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "The release identity manifest must be the canonical in-place file in the receipt-bound SigningDirectory."
  }
  $manifestEvidence = Get-StableReleaseArtifactEvidence -Path $manifestFull -Label "The RSA-PSS release identity manifest"
  if ($manifestEvidence.Length -gt 64KB) { throw "The release identity manifest exceeds its 64 KiB bound." }

  $runDirectory = New-GeneratedReleaseDirectory -RepoRoot $RepoRoot -Edition $Edition
  $locks = [System.Collections.Generic.List[System.IDisposable]]::new()
  $frontendContext = $null
  try {
    $runVerifier = Set-ExactReleaseArtifact `
      -SourcePath $preparation.Verifier.Path `
      -DestinationPath (Join-Path $runDirectory "code-hangar-release-verify.exe") `
      -Label "The receipt-bound run verifier" `
      -ExpectedSha256 $preparation.Verifier.Sha256 `
      -RequirePe
    $locks.Add((Open-ReleaseArtifactReadLock -Path $runVerifier.Path -Label "The receipt-bound run verifier"))
    $preProofDir = Join-Path $runDirectory "pre-bundle-install"
    [void][System.IO.Directory]::CreateDirectory($preProofDir)
    $proofParent = Set-ExactReleaseArtifact -SourcePath $signedParent.Path -DestinationPath (Join-Path $preProofDir "code-hangar-desktop.exe") -Label "Pre-bundle signed parent" -ExpectedSha256 $signedParent.Sha256 -RequirePe
    $proofHelper = Set-ExactReleaseArtifact -SourcePath $signedHelper.Path -DestinationPath (Join-Path $preProofDir "code-hangar-elevated.exe") -Label "Pre-bundle signed helper" -ExpectedSha256 $signedHelper.Sha256 -RequirePe
    $proofManifest = Set-ExactReleaseArtifact -SourcePath $manifestFull -DestinationPath (Join-Path $preProofDir $script:CodeHangarReleaseManifestFileName) -Label "Pre-bundle release manifest" -ExpectedSha256 $manifestEvidence.Sha256
    foreach ($entry in @(
        @{ Path = $proofParent.Path; Label = "The pre-bundle signed parent proof" },
        @{ Path = $proofHelper.Path; Label = "The pre-bundle signed helper proof" },
        @{ Path = $proofManifest.Path; Label = "The pre-bundle signed manifest proof" }
      )) {
      $locks.Add((Open-ReleaseArtifactReadLock -Path $entry.Path -Label $entry.Label))
    }
    $preProof = Invoke-CodeHangarReleaseVerifier `
      -VerifierPath $runVerifier.Path `
      -InstallDirectory $preProofDir `
      -ExpectedParentSha256 $proofParent.Sha256 `
      -ExpectedHelperSha256 $proofHelper.Sha256
    if ([string]$preProof.manifest_sha256 -cne $proofManifest.Sha256 -or
        [string]$preProof.release_id -cne $expectedReleaseId) {
      throw "Release verifier proof does not match the receipt-bound owner manifest identity."
    }

    $targetRelease = Join-Path $TargetDir "release"
    if (-not (Test-Path -LiteralPath $targetRelease)) { [void][System.IO.Directory]::CreateDirectory($targetRelease) }
    Assert-FixedLocalPathChain -Path $targetRelease -Label "The release staging directory" -RequireExisting
    $targetParent = Join-Path $targetRelease "code-hangar-desktop.exe"
    $sidecarDir = Join-Path $RepoRoot "apps\desktop\src-tauri\binaries"
    if (-not (Test-Path -LiteralPath $sidecarDir)) { [void][System.IO.Directory]::CreateDirectory($sidecarDir) }
    Assert-FixedLocalPathChain -Path $sidecarDir -Label "The staged-sidecar path" -RequireExisting
    Remove-StagedReleaseArtifacts -SidecarDir $sidecarDir
    $stagedHelper = Join-Path $sidecarDir "code-hangar-elevated-$triple.exe"
    $stagedManifest = Join-Path $sidecarDir $script:CodeHangarReleaseManifestFileName
    $stagedParentEvidence = Set-ExactReleaseArtifact -SourcePath $proofParent.Path -DestinationPath $targetParent -Label "The exact signed desktop bundle input" -ExpectedSha256 $proofParent.Sha256 -RequirePe
    $stagedHelperEvidence = Set-ExactReleaseArtifact -SourcePath $proofHelper.Path -DestinationPath $stagedHelper -Label "The exact signed elevated-helper sidecar" -ExpectedSha256 $proofHelper.Sha256 -RequirePe
    $stagedManifestEvidence = Set-ExactReleaseArtifact -SourcePath $proofManifest.Path -DestinationPath $stagedManifest -Label "The exact signed release-manifest resource" -ExpectedSha256 $proofManifest.Sha256
    $stagedMcpEvidence = $null
    $stagedMcp = $null
    if ($Edition -eq "Connector") {
      $stagedMcp = Join-Path $sidecarDir "code-hangar-mcp-$triple.exe"
      $stagedMcpEvidence = Set-ExactReleaseArtifact -SourcePath $preparation.Mcp.Path -DestinationPath $stagedMcp -Label "The receipt-bound MCP sidecar" -ExpectedSha256 $preparation.Mcp.Sha256 -RequirePe
    }

    foreach ($entry in @(
        @{ Path = $targetParent; Label = "The signed desktop bundle input" },
        @{ Path = $stagedHelper; Label = "The signed helper bundle input" },
        @{ Path = $stagedManifest; Label = "The signed manifest bundle input" }
      )) {
      $locks.Add((Open-ReleaseArtifactReadLock -Path $entry.Path -Label $entry.Label))
    }
    if ($Edition -eq "Connector") {
      $locks.Add((Open-ReleaseArtifactReadLock -Path $stagedMcp -Label "The receipt-bound MCP bundle input"))
    }

    $releaseVersion = $version
    $expectedInstallerName = if ($Edition -eq "Connector") {
      "Code Hangar AI Connector_${releaseVersion}_x64-setup.exe"
    } else {
      "Code Hangar_${releaseVersion}_x64-setup.exe"
    }
    $nsisDir = Join-Path $targetRelease "bundle\nsis"
    Assert-FixedLocalPathChain -Path $nsisDir -Label "The NSIS output path"
    $frontendContext = Enter-CodeHangarFrontendSnapshotBundleContext `
      -RepoRoot $RepoRoot `
      -FrontendSnapshot $preparation.Frontend `
      -RunDirectory $runDirectory
    Invoke-CodeHangarFrontendEditionCheck `
      -RepoRoot $RepoRoot `
      -Edition $Edition `
      -FailureLabel "$Edition receipt-bound frontend isolation check"
    Assert-CodeHangarFrontendSnapshotBundleContextState -Context $frontendContext
    $runStartedAtUtc = [datetime]::UtcNow
    Remove-EditionRawInstallers -NsisDir $nsisDir -Edition $Edition
    $desktopFeature = if ($Edition -eq "Connector") { "agent_automation" } else { "mutation" }
    $tauriCmd = Join-Path $RepoRoot "node_modules\.bin\tauri.cmd"
    Set-Location (Join-Path $RepoRoot "apps\desktop")
    [void](Get-ValidatedNsisCompiler)
    $tauriArguments = @("bundle", "--no-sign", "--ci", "--bundles", "nsis", "--features", $desktopFeature) + @($WebViewContext.TauriConfigArguments)
    Invoke-CheckedExternalCommand -FilePath $tauriCmd -Arguments $tauriArguments -FailureLabel "$Edition signed-input NSIS bundling"
    Set-Location $RepoRoot
    [void](Get-ValidatedNsisCompiler)
    Assert-CodeHangarFrontendSnapshotBundleContextState -Context $frontendContext

    $cargoLockHashAfter = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($cargoLockHashAfter -cne $cargoLockHash) { throw "Cargo.lock changed during BundleSigned; refusing the bundle." }
    $bundleContractHashAfter = Get-CodeHangarBundleContractSha256 -RepoRoot $RepoRoot -Edition $Edition
    if ($bundleContractHashAfter -cne $bundleContractHash) { throw "The canonical bundle contract changed during BundleSigned; refusing the bundle." }
    foreach ($expected in @(
        @{ Path = $targetParent; Label = "Post-bundle signed desktop input"; Sha256 = $stagedParentEvidence.Sha256; Pe = $true },
        @{ Path = $stagedHelper; Label = "Post-bundle signed helper input"; Sha256 = $stagedHelperEvidence.Sha256; Pe = $true },
        @{ Path = $stagedManifest; Label = "Post-bundle release manifest"; Sha256 = $stagedManifestEvidence.Sha256; Pe = $false }
      )) {
      $actual = Get-StableReleaseArtifactEvidence -Path $expected.Path -Label $expected.Label -RequirePe:$expected.Pe
      if ($actual.Sha256 -cne $expected.Sha256) { throw "$($expected.Label) changed during tauri bundle --no-sign." }
    }
    if ($Edition -eq "Connector") {
      $mcpAfter = Get-StableReleaseArtifactEvidence -Path $stagedMcp -Label "Post-bundle MCP sidecar" -RequirePe
      if ($mcpAfter.Sha256 -cne $stagedMcpEvidence.Sha256) { throw "The MCP sidecar changed during tauri bundle --no-sign." }
    }

    $postProofDir = Join-Path $runDirectory "post-bundle-install"
    [void][System.IO.Directory]::CreateDirectory($postProofDir)
    $postParent = Set-ExactReleaseArtifact -SourcePath $targetParent -DestinationPath (Join-Path $postProofDir "code-hangar-desktop.exe") -Label "Post-bundle proof parent" -ExpectedSha256 $stagedParentEvidence.Sha256 -RequirePe
    $postHelper = Set-ExactReleaseArtifact -SourcePath $stagedHelper -DestinationPath (Join-Path $postProofDir "code-hangar-elevated.exe") -Label "Post-bundle proof helper" -ExpectedSha256 $stagedHelperEvidence.Sha256 -RequirePe
    $postManifest = Set-ExactReleaseArtifact -SourcePath $stagedManifest -DestinationPath (Join-Path $postProofDir $script:CodeHangarReleaseManifestFileName) -Label "Post-bundle proof manifest" -ExpectedSha256 $stagedManifestEvidence.Sha256
    $postProof = Invoke-CodeHangarReleaseVerifier `
      -VerifierPath $runVerifier.Path `
      -InstallDirectory $postProofDir `
      -ExpectedParentSha256 $postParent.Sha256 `
      -ExpectedHelperSha256 $postHelper.Sha256
    if ([string]$postProof.release_id -cne $expectedReleaseId -or
        [string]$postProof.release_id -cne [string]$preProof.release_id -or
        [string]$postProof.manifest_sha256 -cne $postManifest.Sha256 -or
        [string]$postProof.manifest_sha256 -cne [string]$preProof.manifest_sha256) {
      throw "Post-bundle release verification no longer matches the pre-bundle release identity."
    }

    Assert-FixedLocalPathChain -Path $nsisDir -Label "The final NSIS output path" -RequireExisting
    $installer = Get-ValidatedFreshInstaller `
      -NsisDir $nsisDir `
      -Edition $Edition `
      -ExpectedFileName $expectedInstallerName `
      -StartedAtUtc $runStartedAtUtc
    $installer = Assert-RawUnsignedHoldInstaller `
      -Path $installer.Path `
      -ExpectedSha256 $installer.Sha256
    Write-Host ""
    Write-Host "$Edition BundleSigned created a verified-inner-binaries raw UNSIGNED HOLD candidate: $($installer.Path)" -ForegroundColor Green
    Write-Host "Raw unsigned/HOLD candidate SHA256: $($installer.Sha256)" -ForegroundColor Green
    Write-Host "Release identity: $($postProof.release_id)" -ForegroundColor Green
    Write-Host "OWNER GATE: the outer setup is proven NotSigned; the embedded uninstaller is not claimed signed or release-ready. Do not publish; use an audited owner-certificate NSIS signCommand flow and verify both setup and installed uninstaller in a reset VM." -ForegroundColor Yellow
  } finally {
    $frontendRestoreError = $null
    if ($null -ne $frontendContext) {
      try {
        Restore-CodeHangarFrontendSnapshotBundleContext -Context $frontendContext
      } catch {
        $frontendRestoreError = $_
      }
    }
    foreach ($lock in $locks) { $lock.Dispose() }
    $generatedCleanupError = $null
    try {
      Remove-GeneratedReleaseDirectory -RepoRoot $RepoRoot -RunDirectory $runDirectory
    } catch {
      $generatedCleanupError = $_
    }
    if ($null -ne $frontendRestoreError) { throw $frontendRestoreError }
    if ($null -ne $generatedCleanupError) { throw $generatedCleanupError }
  }
}

function Invoke-CodeHangarReleasePackaging {
  param(
    [Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition,
    [Parameter(Mandatory = $true)][string]$RepoRoot,
    [string]$WebView2InstallerPath,
    [switch]$PreflightOnly,
    [switch]$PrepareSigning,
    [switch]$BundleSigned,
    [string]$ReleaseRootPublicBlobHex,
    [string]$SigningDirectory,
    [string]$ExpectedSigningReceiptSha256,
    [string]$SignedParentPath,
    [string]$SignedHelperPath,
    [string]$ReleaseManifestPath,
    [switch]$SelfTest
  )

  Assert-CodeHangarPackagingMode -PreflightOnly:$PreflightOnly -PrepareSigning:$PrepareSigning -BundleSigned:$BundleSigned -SelfTest:$SelfTest
  $preflightScript = Join-Path $RepoRoot "scripts\packaging-preflight.mjs"
  if ($SelfTest) {
    foreach ($value in @($WebView2InstallerPath, $ReleaseRootPublicBlobHex, $SigningDirectory, $ExpectedSigningReceiptSha256, $SignedParentPath, $SignedHelperPath, $ReleaseManifestPath)) {
      if (-not [string]::IsNullOrWhiteSpace($value)) { throw "-SelfTest does not accept release input parameters." }
    }
    Assert-PackagingEnvironmentOverrides
    & node $preflightScript --self-test
    if ($LASTEXITCODE -ne 0) { throw "Packaging preflight self-test failed with exit code $LASTEXITCODE." }
    Invoke-PackagingCommonSelfTest -Edition $Edition
    & (Join-Path $RepoRoot "scripts\new-release-identity-manifest.ps1") -SelfTest
    if ($LASTEXITCODE -ne 0) { throw "Release identity manifest self-test failed with exit code $LASTEXITCODE." }
    return
  }

  if ([string]::IsNullOrWhiteSpace($WebView2InstallerPath)) {
    throw "-WebView2InstallerPath is required. It must identify the explicitly pinned offline installer."
  }
  if ($PreflightOnly) {
    foreach ($value in @($ReleaseRootPublicBlobHex, $SigningDirectory, $ExpectedSigningReceiptSha256, $SignedParentPath, $SignedHelperPath, $ReleaseManifestPath)) {
      if (-not [string]::IsNullOrWhiteSpace($value)) { throw "-PreflightOnly does not accept signing or signed-artifact parameters." }
    }
  } elseif ($PrepareSigning) {
    if ([string]::IsNullOrWhiteSpace($ReleaseRootPublicBlobHex) -or [string]::IsNullOrWhiteSpace($SigningDirectory)) {
      throw "-PrepareSigning requires -ReleaseRootPublicBlobHex and a new -SigningDirectory."
    }
    foreach ($value in @($ExpectedSigningReceiptSha256, $SignedParentPath, $SignedHelperPath, $ReleaseManifestPath)) {
      if (-not [string]::IsNullOrWhiteSpace($value)) { throw "-PrepareSigning does not accept signed parent/helper/manifest inputs." }
    }
  } else {
    foreach ($required in @{
        ReleaseRootPublicBlobHex = $ReleaseRootPublicBlobHex
        SigningDirectory = $SigningDirectory
        ExpectedSigningReceiptSha256 = $ExpectedSigningReceiptSha256
        SignedParentPath = $SignedParentPath
        SignedHelperPath = $SignedHelperPath
        ReleaseManifestPath = $ReleaseManifestPath
      }.GetEnumerator()) {
      if ([string]::IsNullOrWhiteSpace([string]$required.Value)) { throw "-BundleSigned requires -$($required.Key)." }
    }
  }

  $normalizedPublicBlob = $null
  if (-not $PreflightOnly) {
    $normalizedPublicBlob = Assert-ReleaseRootPublicBlobHex -Value $ReleaseRootPublicBlobHex
  }
  $packagingLock = $null
  $webViewContext = $null
  $hadReleaseRoot = Test-Path Env:\CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX
  $priorReleaseRoot = $env:CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX
  try {
    $packagingLock = Enter-WorktreePackagingLock -RepoRoot $RepoRoot
    Assert-PackagingEnvironmentOverrides
    $targetDir = Initialize-CodeHangarOfflinePackagingEnvironment -RepoRoot $RepoRoot
    foreach ($cargoBin in @(
        (Join-Path $env:USERPROFILE ".cargo\bin"),
        (Join-Path $env:USERPROFILE ".local\cargo\bin")
      )) {
      if (Test-Path -LiteralPath $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
    }
    if (-not $PreflightOnly) {
      if ($hadReleaseRoot -and -not [string]::IsNullOrWhiteSpace($priorReleaseRoot) -and
          $priorReleaseRoot.ToUpperInvariant() -cne $normalizedPublicBlob) {
        throw "Existing CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX conflicts with the explicit release root."
      }
      $env:CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX = $normalizedPublicBlob
    }

    $manifestPath = Join-Path $RepoRoot "scripts\release-inputs\webview2-x64.json"
    $webViewContext = New-PinnedWebView2PackagingContext `
      -RepoRoot $RepoRoot `
      -Edition $Edition `
      -InstallerPath $WebView2InstallerPath `
      -ManifestPath $manifestPath
    Invoke-GeneratedNsisHookSyntaxCheck -Context $webViewContext
    $nodeArgs = @(
      $preflightScript,
      "--tauri",
      "--manifest", $webViewContext.ManifestPath,
      "--webview", $webViewContext.Staged.Evidence.Path,
      "--override", $webViewContext.OverridePath,
      "--hook", $webViewContext.HookPath,
      "--base-hook", $webViewContext.BaseHookPath,
      "--edition", $Edition.ToLowerInvariant()
    )
    for ($index = 1; $index -lt $webViewContext.TauriConfigArguments.Count; $index += 2) {
      $nodeArgs += @("--config-order", $webViewContext.TauriConfigArguments[$index])
    }
    & node @nodeArgs
    if ($LASTEXITCODE -ne 0) { throw "$Edition pinned-WebView2 packaging preflight failed with exit code $LASTEXITCODE." }
    if ($PreflightOnly) {
      Write-Host "$Edition packaging preflight passed. No build or packaging command was run." -ForegroundColor Green
      return
    }

    if ($PrepareSigning) {
      Invoke-CodeHangarPrepareSigning `
        -Edition $Edition `
        -RepoRoot $RepoRoot `
        -TargetDir $targetDir `
        -WebViewContext $webViewContext `
        -ReleaseRootPublicBlobHex $normalizedPublicBlob `
        -SigningDirectory $SigningDirectory
    } else {
      Invoke-CodeHangarBundleSigned `
        -Edition $Edition `
        -RepoRoot $RepoRoot `
        -TargetDir $targetDir `
        -WebViewContext $webViewContext `
        -ReleaseRootPublicBlobHex $normalizedPublicBlob `
        -SigningDirectory $SigningDirectory `
        -ExpectedSigningReceiptSha256 $ExpectedSigningReceiptSha256 `
        -SignedParentPath $SignedParentPath `
        -SignedHelperPath $SignedHelperPath `
        -ReleaseManifestPath $ReleaseManifestPath
    }
  } finally {
    Set-Location $RepoRoot
    try {
      if ($null -ne $webViewContext) { Remove-PinnedWebView2PackagingContext -Context $webViewContext -RepoRoot $RepoRoot }
    } finally {
      try {
        if ($null -ne $packagingLock) { $packagingLock.Dispose() }
      } finally {
        if ($hadReleaseRoot) {
          $env:CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX = $priorReleaseRoot
        } else {
          Remove-Item Env:\CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX -ErrorAction SilentlyContinue
        }
      }
    }
  }
}

function Write-PackagingSelfTestPe {
  param([Parameter(Mandatory = $true)][string]$Path)
  $bytes = [byte[]]::new(1024)
  $bytes[0] = 0x4D
  $bytes[1] = 0x5A
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint32]0x80), 0, $bytes, 0x3C, 4)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint32]0x00004550), 0, $bytes, 0x80, 4)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint16]0x8664), 0, $bytes, 0x84, 2)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint16]0x00F0), 0, $bytes, 0x94, 2)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint16]0x020B), 0, $bytes, 0x98, 2)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint32]16), 0, $bytes, 0x104, 4)
  [System.IO.File]::WriteAllBytes($Path, $bytes)
}

function Add-PackagingSelfTestAuthenticodeCertificate {
  param([Parameter(Mandatory = $true)][string]$Path)

  $before = [System.IO.File]::ReadAllBytes($Path)
  if ($before.Length -lt 1024) { throw "Self-test PE is unexpectedly short." }
  $peOffset = [System.BitConverter]::ToUInt32($before, 0x3C)
  if ($peOffset -ne 0x80 -or [System.BitConverter]::ToUInt32($before, [int]$peOffset) -ne 0x00004550) {
    throw "Self-test PE has an invalid PE header."
  }
  $optionalHeaderOffset = [int]$peOffset + 24
  if ([System.BitConverter]::ToUInt16($before, $optionalHeaderOffset) -ne 0x020B) {
    throw "Self-test PE has an invalid PE32+ optional header."
  }
  $certificateOffset = [uint32]$before.Length
  $certificateLength = [uint32]16
  $after = [byte[]]::new($before.Length + $certificateLength)
  [System.Buffer]::BlockCopy($before, 0, $after, 0, $before.Length)
  $syntheticChecksum = [System.UInt32]::Parse('AABBCCDD', [System.Globalization.NumberStyles]::HexNumber, [System.Globalization.CultureInfo]::InvariantCulture)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes($syntheticChecksum), 0, $after, $optionalHeaderOffset + 64, 4)
  $certificateDirectoryOffset = $optionalHeaderOffset + 112 + (8 * 4)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes($certificateOffset), 0, $after, $certificateDirectoryOffset, 4)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes($certificateLength), 0, $after, $certificateDirectoryOffset + 4, 4)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes($certificateLength), 0, $after, [int]$certificateOffset, 4)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint16]0x0200), 0, $after, [int]$certificateOffset + 4, 2)
  [System.Buffer]::BlockCopy([System.BitConverter]::GetBytes([uint16]0x0002), 0, $after, [int]$certificateOffset + 6, 2)
  [System.IO.File]::WriteAllBytes($Path, $after)
}

function Invoke-PackagingCommonSelfTest {
  param([Parameter(Mandatory = $true)][ValidateSet("Local", "Connector")][string]$Edition)

  $tempRoot = [System.IO.Path]::GetFullPath((Join-Path ([System.IO.Path]::GetTempPath()) "codehangar-packaging-selftest-$([guid]::NewGuid().ToString('N'))"))
  $tempParent = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  if (-not ([System.IO.Path]::GetDirectoryName($tempRoot)).Equals($tempParent, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing unsafe packaging self-test root: $tempRoot"
  }
  [void][System.IO.Directory]::CreateDirectory($tempRoot)
  $lock = $null
  try {
    $nodeOptionsRejected = $false
    try {
      Assert-PackagingEnvironmentOverrides -Environment @{ NODE_OPTIONS = "--require untrusted.js" }
    } catch {
      if ($_.Exception.Message -like "NODE_OPTIONS must be empty*") { $nodeOptionsRejected = $true } else { throw }
    }
    if (-not $nodeOptionsRejected) { throw "NODE_OPTIONS override self-test did not fail closed." }

    $tauriOverrideRejected = $false
    try {
      Assert-PackagingEnvironmentOverrides -Environment @{ TAURI_CONFIG = "untrusted.json" }
    } catch {
      if ($_.Exception.Message -like "TAURI_CONFIG is a build-affecting TAURI_* override*") { $tauriOverrideRejected = $true } else { throw }
    }
    if (-not $tauriOverrideRejected) { throw "TAURI_* override self-test did not fail closed." }

    $rustFlagsRejected = $false
    try {
      Assert-PackagingEnvironmentOverrides -Environment @{ RUSTFLAGS = "-C target-cpu=native" }
    } catch {
      if ($_.Exception.Message -like "RUSTFLAGS must be empty*") { $rustFlagsRejected = $true } else { throw }
    }
    if (-not $rustFlagsRejected) { throw "RUSTFLAGS override self-test did not fail closed." }

    $cargoProfileRejected = $false
    try {
      Assert-PackagingEnvironmentOverrides -Environment @{ CARGO_PROFILE_RELEASE_LTO = "off" }
    } catch {
      if ($_.Exception.Message -like "CARGO_PROFILE_RELEASE_LTO is a build-affecting Cargo override*") { $cargoProfileRejected = $true } else { throw }
    }
    if (-not $cargoProfileRejected) { throw "Cargo profile override self-test did not fail closed." }

    $unsafeNsisPathRejected = $false
    try {
      Assert-SafeNsisLiteralPath -Path 'C:\unsafe$path\installer.exe'
    } catch {
      if ($_.Exception.Message -like "NSIS source/include path contains unsafe*") { $unsafeNsisPathRejected = $true } else { throw }
    }
    if (-not $unsafeNsisPathRejected) { throw "Unsafe NSIS quoting self-test did not fail closed." }

    $localHookLeakRejected = $false
    try {
      Assert-LocalInstallerHookIsolation `
        -BaseHookContent 'ReadRegStr $0 HKCU "Software\JCOM Labs\Code Hangar\Installations\Code Hangar AI Connector" "Executable"' `
        -GeneratedHookTemplate '!include "neutral.nsh"'
    } catch {
      if ($_.Exception.Message -like 'The effective Local NSIS hook contains a Connector/AI*') { $localHookLeakRejected = $true } else { throw }
    }
    if (-not $localHookLeakRejected) { throw "Local NSIS Connector-path isolation self-test did not fail closed." }

    $relativeInputRejected = $false
    try {
      Open-PinnedWebView2Installer -Path ".\MicrosoftEdgeWebView2RuntimeInstallerX64.exe" -Manifest @{} -Label "Self-test WebView2 input"
    } catch {
      if ($_.Exception.Message -like "*path must be fully qualified*") { $relativeInputRejected = $true } else { throw }
    }
    if (-not $relativeInputRejected) { throw "Relative WebView2 input self-test did not fail closed." }

    $realDirectory = Join-Path $tempRoot "real-directory"
    $junctionPath = Join-Path $tempRoot "junction-directory"
    [void][System.IO.Directory]::CreateDirectory($realDirectory)
    [void](New-Item -ItemType Junction -Path $junctionPath -Target $realDirectory -ErrorAction Stop)
    $reparseRejected = $false
    try {
      Assert-FixedLocalPathChain -Path $junctionPath -Label "Self-test junction" -RequireExisting
    } catch {
      if ($_.Exception.Message -like "*contains a reparse point*") { $reparseRejected = $true } else { throw }
    }
    if (-not $reparseRejected) { throw "Reparse-path self-test did not fail closed." }
    Remove-Item -LiteralPath $junctionPath -Force -ErrorAction Stop

    $lockedInput = Join-Path $tempRoot "locked-input.bin"
    [System.IO.File]::WriteAllBytes($lockedInput, [byte[]](1, 2, 3, 4))
    $inputLock = [System.IO.FileStream]::new(
      $lockedInput,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::Read
    )
    try {
      $writeRejected = $false
      try {
        $unexpectedWriter = [System.IO.FileStream]::new(
          $lockedInput,
          [System.IO.FileMode]::Open,
          [System.IO.FileAccess]::Write,
          [System.IO.FileShare]::Read
        )
        $unexpectedWriter.Dispose()
      } catch [System.IO.IOException] {
        $writeRejected = $true
      }
      if (-not $writeRejected) { throw "Locked WebView2 input allowed a writer." }
      $deleteRejected = $false
      try {
        [System.IO.File]::Delete($lockedInput)
      } catch [System.IO.IOException] {
        $deleteRejected = $true
      }
      if (-not $deleteRejected -or -not (Test-Path -LiteralPath $lockedInput)) {
        throw "Locked WebView2 input allowed deletion."
      }
    } finally {
      $inputLock.Dispose()
    }

    $metadataManifest = [pscustomobject]@{
      length = 4
      sha256 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
      fileVersion = "1.2.3.4"
      peMachine = "014C"
      signerSubject = "CN=Microsoft Corporation"
      signerThumbprint = "1111111111111111111111111111111111111111"
      signerIssuer = "CN=Microsoft Code Signing PCA 2024"
      timestampThumbprint = "2222222222222222222222222222222222222222"
    }
    $metadataEvidence = [pscustomobject]@{
      Length = 4
      Sha256 = $metadataManifest.sha256
      FileVersion = $metadataManifest.fileVersion
      PeMachine = $metadataManifest.peMachine
      SignerSubject = $metadataManifest.signerSubject
      SignerThumbprint = $metadataManifest.signerThumbprint
      SignerIssuer = $metadataManifest.signerIssuer
      TimestampThumbprint = $metadataManifest.timestampThumbprint
    }
    Assert-PinnedWebView2EvidenceMatchesManifest -Evidence $metadataEvidence -Manifest $metadataManifest
    foreach ($field in @("Length", "Sha256", "FileVersion", "SignerSubject", "SignerThumbprint", "SignerIssuer", "TimestampThumbprint")) {
      $originalValue = $metadataEvidence.$field
      $metadataEvidence.$field = if ($field -eq "Length") { 5 } else { "wrong" }
      $mismatchRejected = $false
      try {
        Assert-PinnedWebView2EvidenceMatchesManifest -Evidence $metadataEvidence -Manifest $metadataManifest
      } catch {
        if ($_.Exception.Message -like "Pinned WebView2 $field does not match*") { $mismatchRejected = $true } else { throw }
      }
      if (-not $mismatchRejected) { throw "$field metadata mismatch self-test did not fail closed." }
      $metadataEvidence.$field = $originalValue
    }

    $lock = Enter-WorktreePackagingLock -RepoRoot $tempRoot
    $secondLockRejected = $false
    try {
      $unexpectedLock = Enter-WorktreePackagingLock -RepoRoot $tempRoot
      $unexpectedLock.Dispose()
    } catch {
      if ($_.Exception.Message -like "Another Code Hangar packaging/preflight process*") {
        $secondLockRejected = $true
      } else {
        throw
      }
    }
    if (-not $secondLockRejected) { throw "Concurrent packaging lock self-test did not fail closed." }

    $nsisDir = Join-Path $tempRoot "nsis"
    [void][System.IO.Directory]::CreateDirectory($nsisDir)
    $localName = "Code Hangar_0.1.3_x64-setup.exe"
    $connectorName = "Code Hangar AI Connector_0.1.3_x64-setup.exe"
    $expectedName = if ($Edition -eq "Local") { $localName } else { $connectorName }
    $otherName = if ($Edition -eq "Local") { $connectorName } else { $localName }
    $expectedPath = Join-Path $nsisDir $expectedName
    $otherPath = Join-Path $nsisDir $otherName
    Write-PackagingSelfTestPe -Path $expectedPath
    Write-PackagingSelfTestPe -Path $otherPath
    Remove-EditionRawInstallers -NsisDir $nsisDir -Edition $Edition
    if (Test-Path -LiteralPath $expectedPath) { throw "Same-edition stale installer was not removed." }
    if (-not (Test-Path -LiteralPath $otherPath -PathType Leaf)) { throw "Other-edition installer was not preserved." }

    Write-PackagingSelfTestPe -Path $expectedPath
    $started = [datetime]::UtcNow
    [System.IO.File]::SetCreationTimeUtc($expectedPath, $started.AddMinutes(-10))
    [System.IO.File]::SetLastWriteTimeUtc($expectedPath, $started.AddMinutes(1))
    $staleTouchedRejected = $false
    try {
      [void](Get-ValidatedFreshInstaller -NsisDir $nsisDir -Edition $Edition -ExpectedFileName $expectedName -StartedAtUtc $started)
    } catch {
      if ($_.Exception.Message -like "*was not newly created after this run started*") {
        $staleTouchedRejected = $true
      } else {
        throw
      }
    }
    if (-not $staleTouchedRejected) { throw "Touched stale installer self-test did not fail closed." }

    Remove-Item -LiteralPath $expectedPath -Force
    $wrongName = if ($Edition -eq "Local") { "Code Hangar_9.9.9_x64-setup.exe" } else { "Code Hangar AI Connector_9.9.9_x64-setup.exe" }
    $wrongPath = Join-Path $nsisDir $wrongName
    Write-PackagingSelfTestPe -Path $wrongPath
    $wrongNameRejected = $false
    try {
      [void](Get-ValidatedFreshInstaller -NsisDir $nsisDir -Edition $Edition -ExpectedFileName $expectedName -StartedAtUtc ([datetime]::UtcNow.AddSeconds(-5)))
    } catch {
      if ($_.Exception.Message -like "*filename is not the exact version-bound name*") {
        $wrongNameRejected = $true
      } else {
        throw
      }
    }
    if (-not $wrongNameRejected) { throw "Wrong-version installer filename self-test did not fail closed." }

    Remove-Item -LiteralPath $wrongPath -Force
    # Use a new output directory for the positive case. NTFS filename tunnelling
    # can deliberately preserve an old CreationTime when the same name is
    # deleted and recreated quickly; production treats that ambiguity as a
    # fail-closed result rather than weakening the stale-file gate.
    $positiveDir = Join-Path $tempRoot "nsis-positive"
    [void][System.IO.Directory]::CreateDirectory($positiveDir)
    $positivePath = Join-Path $positiveDir $expectedName
    $positiveStart = [datetime]::UtcNow.AddSeconds(-5)
    Write-PackagingSelfTestPe -Path $positivePath
    $validated = Get-ValidatedFreshInstaller -NsisDir $positiveDir -Edition $Edition -ExpectedFileName $expectedName -StartedAtUtc $positiveStart
    if ([string]::IsNullOrWhiteSpace($validated.Sha256)) { throw "Fresh installer self-test did not produce SHA-256." }
    $unsignedHold = Assert-RawUnsignedHoldInstaller -Path $validated.Path -ExpectedSha256 $validated.Sha256
    if ($unsignedHold.Sha256 -cne $validated.Sha256) { throw "Unsigned raw-HOLD self-test changed installer evidence." }

    $publicBlobBytes = [byte[]]::new(24 + 3 + 384)
    [System.BitConverter]::GetBytes([uint32]0x31415352).CopyTo($publicBlobBytes, 0)
    [System.BitConverter]::GetBytes([uint32]3072).CopyTo($publicBlobBytes, 4)
    [System.BitConverter]::GetBytes([uint32]3).CopyTo($publicBlobBytes, 8)
    [System.BitConverter]::GetBytes([uint32]384).CopyTo($publicBlobBytes, 12)
    $publicBlobBytes[24] = 1
    $publicBlobBytes[25] = 0
    $publicBlobBytes[26] = 1
    for ($index = 27; $index -lt $publicBlobBytes.Length; $index++) { $publicBlobBytes[$index] = 0xAB }
    $publicBlob = Assert-ReleaseRootPublicBlobHex -Value (([System.BitConverter]::ToString($publicBlobBytes)).Replace('-', ''))
    $invalidBlobRejected = $false
    try {
      [void](Assert-ReleaseRootPublicBlobHex -Value ('AB' * 32))
    } catch {
      if ($_.Exception.Message -like "*-ReleaseRootPublicBlobHex must be a bounded*") { $invalidBlobRejected = $true } else { throw }
    }
    if (-not $invalidBlobRejected) { throw "Undersized release-root public blob self-test did not fail closed." }

    $signingDir = Join-Path $tempRoot "signing-preparation"
    [void][System.IO.Directory]::CreateDirectory($signingDir)
    $preparedPaths = @{
      Parent = Join-Path $signingDir "code-hangar-desktop.exe"
      Helper = Join-Path $signingDir "code-hangar-elevated.exe"
      Verifier = Join-Path $signingDir "code-hangar-release-verify.exe"
      Mcp = Join-Path $signingDir "code-hangar-mcp.exe"
    }
    foreach ($path in $preparedPaths.Values) { Write-PackagingSelfTestPe -Path $path }
    # Make every synthetic PE byte-distinct while preserving its PE header.
    [System.IO.File]::AppendAllText($preparedPaths.Parent, "parent")
    [System.IO.File]::AppendAllText($preparedPaths.Helper, "helper")
    [System.IO.File]::AppendAllText($preparedPaths.Verifier, "verifier")
    [System.IO.File]::AppendAllText($preparedPaths.Mcp, "mcp")
    $preparedParent = Get-StableReleaseArtifactEvidence -Path $preparedPaths.Parent -Label "Self-test parent" -RequirePe
    $preparedHelper = Get-StableReleaseArtifactEvidence -Path $preparedPaths.Helper -Label "Self-test helper" -RequirePe
    $preparedVerifier = Get-StableReleaseArtifactEvidence -Path $preparedPaths.Verifier -Label "Self-test verifier" -RequirePe
    $preparedMcp = if ($Edition -eq "Connector") {
      Get-StableReleaseArtifactEvidence -Path $preparedPaths.Mcp -Label "Self-test MCP" -RequirePe
    } else {
      Remove-Item -LiteralPath $preparedPaths.Mcp -Force
      $null
    }
    $frontendSource = Join-Path $tempRoot "prepared-frontend-source"
    [void][System.IO.Directory]::CreateDirectory((Join-Path $frontendSource "assets"))
    [System.IO.File]::WriteAllText((Join-Path $frontendSource "index.html"), "<html>$Edition prepared frontend</html>", [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText((Join-Path $frontendSource "assets\edition-marker.txt"), "$Edition-only-marker", [System.Text.UTF8Encoding]::new($false))
    $frontendSnapshot = New-CodeHangarFrontendSnapshot `
      -SigningDirectory $signingDir `
      -Edition $Edition `
      -FrontendDistPath $frontendSource
    $cargoLockHash = ('12' * 32)
    $bundleContractHash = ('78' * 32)
    $selfTestReceipt = Write-CodeHangarSigningReceipt `
      -SigningDirectory $signingDir `
      -Edition $Edition `
      -Version "0.1.3" `
      -TargetTriple "x86_64-pc-windows-msvc" `
      -PublicBlobHex $publicBlob `
      -CargoLockSha256 $cargoLockHash `
      -BundleContractSha256 $bundleContractHash `
      -SourceCommit ('ab' * 20) `
      -SourceTree ('cd' * 20) `
      -ParentEvidence $preparedParent `
      -HelperEvidence $preparedHelper `
      -VerifierEvidence $preparedVerifier `
      -FrontendSnapshot $frontendSnapshot `
      -McpEvidence $preparedMcp
    $receiptValidationArgs = @{
      SigningDirectory = $signingDir
      Edition = $Edition
      ExpectedVersion = "0.1.3"
      ExpectedTargetTriple = "x86_64-pc-windows-msvc"
      ExpectedPublicBlobHex = $publicBlob
      ExpectedCargoLockSha256 = $cargoLockHash
      ExpectedBundleContractSha256 = $bundleContractHash
      ExpectedSourceCommit = ('ab' * 20)
      ExpectedSourceTree = ('cd' * 20)
      ExpectedReceiptSha256 = $selfTestReceipt.Sha256
    }
    $validatedReceipt = Read-AndValidateCodeHangarSigningReceipt @receiptValidationArgs
    foreach ($artifactCheck in @(
        @{ Name = "parent"; Evidence = $preparedParent },
        @{ Name = "helper"; Evidence = $preparedHelper },
        @{ Name = "verifier"; Evidence = $preparedVerifier }
      )) {
      $artifact = $validatedReceipt.Receipt.artifacts.($artifactCheck.Name)
      if ([long]$artifact.length -ne [long]$artifactCheck.Evidence.Length -or
          [string]$artifact.sha256 -cne [string]$artifactCheck.Evidence.Sha256) {
        throw "Signing receipt did not bind the exact prepared $($artifactCheck.Name) evidence."
      }
    }
    if ($Edition -eq "Connector" -and
        ([long]$validatedReceipt.Receipt.artifacts.mcp.length -ne [long]$preparedMcp.Length -or
         [string]$validatedReceipt.Receipt.artifacts.mcp.sha256 -cne [string]$preparedMcp.Sha256)) {
      throw "Connector signing receipt did not bind the exact prepared MCP evidence."
    }
    if ([string]$validatedReceipt.Receipt.artifacts.parent.authenticode_image_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
        [string]$validatedReceipt.Receipt.artifacts.helper.authenticode_image_sha256 -cnotmatch '^[0-9a-f]{64}$') {
      throw "Signing receipt did not bind prepared Authenticode image digests."
    }
    if ($validatedReceipt.Frontend.Tree.Count -ne $frontendSnapshot.Tree.Count -or
        $validatedReceipt.Frontend.Tree.Sha256 -cne $frontendSnapshot.Tree.Sha256) {
      throw "Signing receipt did not bind the exact prepared frontend snapshot tree."
    }

    $expectedReleaseId = Get-CodeHangarReceiptBoundReleaseId -Edition $Edition -SigningReceiptSha256 $selfTestReceipt.Sha256
    $caseNormalizedReleaseId = Get-CodeHangarReceiptBoundReleaseId -Edition $Edition.ToLowerInvariant() -SigningReceiptSha256 $selfTestReceipt.Sha256
    $otherEdition = if ($Edition -eq "Local") { "Connector" } else { "Local" }
    $otherReleaseId = Get-CodeHangarReceiptBoundReleaseId -Edition $otherEdition -SigningReceiptSha256 $selfTestReceipt.Sha256
    if ($expectedReleaseId -cnotmatch '^[0-9a-f]{64}$' -or
        $expectedReleaseId -cne $caseNormalizedReleaseId -or
        $expectedReleaseId -ceq $otherReleaseId) {
      throw "Receipt-bound release-id self-test did not separate Local and Connector."
    }

    $signedFixtureDirectory = Join-Path $tempRoot "signed-fixtures"
    [void][System.IO.Directory]::CreateDirectory($signedFixtureDirectory)
    $signedParentPath = Join-Path $signedFixtureDirectory "code-hangar-desktop.exe"
    $signedHelperPath = Join-Path $signedFixtureDirectory "code-hangar-elevated.exe"
    [void](Set-ExactReleaseArtifact -SourcePath $preparedParent.Path -DestinationPath $signedParentPath -Label "Self-test signed parent source" -ExpectedSha256 $preparedParent.Sha256 -RequirePe)
    [void](Set-ExactReleaseArtifact -SourcePath $preparedHelper.Path -DestinationPath $signedHelperPath -Label "Self-test signed helper source" -ExpectedSha256 $preparedHelper.Sha256 -RequirePe)
    Add-PackagingSelfTestAuthenticodeCertificate -Path $signedParentPath
    Add-PackagingSelfTestAuthenticodeCertificate -Path $signedHelperPath
    $signedParentFixture = Get-StableReleaseArtifactEvidence -Path $signedParentPath -Label "Self-test post-sign parent" -RequirePe
    $signedHelperFixture = Get-StableReleaseArtifactEvidence -Path $signedHelperPath -Label "Self-test post-sign helper" -RequirePe
    [void](Assert-PostSigningArtifactMatchesReceipt `
      -SignedEvidence $signedParentFixture `
      -PreparedReceiptArtifact $validatedReceipt.Receipt.artifacts.parent `
      -Label "Self-test post-sign parent")
    [void](Assert-PostSigningArtifactMatchesReceipt `
      -SignedEvidence $signedHelperFixture `
      -PreparedReceiptArtifact $validatedReceipt.Receipt.artifacts.helper `
      -Label "Self-test post-sign helper")
    $substitutedDirectory = Join-Path $tempRoot "substituted-signed-fixture"
    [void][System.IO.Directory]::CreateDirectory($substitutedDirectory)
    $substitutedParentPath = Join-Path $substitutedDirectory "code-hangar-desktop.exe"
    [void](Set-ExactReleaseArtifact -SourcePath $preparedParent.Path -DestinationPath $substitutedParentPath -Label "Self-test substituted parent source" -ExpectedSha256 $preparedParent.Sha256 -RequirePe)
    [System.IO.File]::AppendAllText($substitutedParentPath, "cross-edition-substitution")
    Add-PackagingSelfTestAuthenticodeCertificate -Path $substitutedParentPath
    $substitutedParent = Get-StableReleaseArtifactEvidence -Path $substitutedParentPath -Label "Self-test substituted post-sign parent" -RequirePe
    $substitutionRejected = $false
    try {
      [void](Assert-PostSigningArtifactMatchesReceipt `
        -SignedEvidence $substitutedParent `
        -PreparedReceiptArtifact $validatedReceipt.Receipt.artifacts.parent `
        -Label "Self-test substituted post-sign parent")
    } catch {
      if ($_.Exception.Message -like "*no longer matches the prepared receipt*") { $substitutionRejected = $true } else { throw }
    }
    if (-not $substitutionRejected) { throw "Cross-edition signed-parent substitution self-test did not fail closed." }

    $frontendMarker = Join-Path $validatedReceipt.Frontend.Directory "assets\edition-marker.txt"
    $frontendMarkerBytes = [System.IO.File]::ReadAllBytes($frontendMarker)
    $frontendTamperRejected = $false
    try {
      [System.IO.File]::AppendAllText($frontendMarker, "tampered")
      try {
        [void](Read-AndValidateCodeHangarSigningReceipt @receiptValidationArgs)
      } catch {
        if ($_.Exception.Message -like "*frontend snapshot*no longer matches*") { $frontendTamperRejected = $true } else { throw }
      }
    } finally {
      [System.IO.File]::WriteAllBytes($frontendMarker, $frontendMarkerBytes)
    }
    if (-not $frontendTamperRejected) { throw "Tampered frontend snapshot self-test did not fail closed." }

    $foreignFrontendSource = Join-Path $tempRoot "foreign-frontend-source"
    [void][System.IO.Directory]::CreateDirectory((Join-Path $foreignFrontendSource "assets"))
    [System.IO.File]::WriteAllText((Join-Path $foreignFrontendSource "index.html"), "<html>$otherEdition frontend</html>", [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText((Join-Path $foreignFrontendSource "assets\edition-marker.txt"), "$otherEdition-only-marker", [System.Text.UTF8Encoding]::new($false))
    $originalFrontendDirectory = "$($validatedReceipt.Frontend.Directory)-original"
    [System.IO.Directory]::Move($validatedReceipt.Frontend.Directory, $originalFrontendDirectory)
    try {
      [void](Copy-CanonicalReleaseTree `
        -SourceRoot $foreignFrontendSource `
        -DestinationRoot $validatedReceipt.Frontend.Directory `
        -Schema $script:CodeHangarFrontendSnapshotSchema `
        -Label "Self-test cross-edition frontend swap")
      $frontendSwapRejected = $false
      try {
        [void](Read-AndValidateCodeHangarSigningReceipt @receiptValidationArgs)
      } catch {
        if ($_.Exception.Message -like "*frontend snapshot*no longer matches*") { $frontendSwapRejected = $true } else { throw }
      }
      if (-not $frontendSwapRejected) { throw "Cross-edition frontend snapshot swap self-test did not fail closed." }
    } finally {
      if (Test-Path -LiteralPath $validatedReceipt.Frontend.Directory) {
        [System.IO.Directory]::Move($validatedReceipt.Frontend.Directory, (Join-Path $tempRoot "foreign-frontend-snapshot-output"))
      }
      [System.IO.Directory]::Move($originalFrontendDirectory, $validatedReceipt.Frontend.Directory)
    }

    $frontendManifestBytes = [System.IO.File]::ReadAllBytes($validatedReceipt.Frontend.Manifest.Path)
    $frontendManifestObject = ([System.Text.UTF8Encoding]::new($false, $true).GetString($frontendManifestBytes) | ConvertFrom-Json -DateKind String)
    $frontendManifestObject.edition = $otherEdition
    [System.IO.File]::WriteAllText(
      $validatedReceipt.Frontend.Manifest.Path,
      ($frontendManifestObject | ConvertTo-Json -Depth 5 -Compress),
      [System.Text.UTF8Encoding]::new($false)
    )
    try {
      $forgedFrontendReceipt = [pscustomobject]@{
        directory_name = $script:CodeHangarFrontendSnapshotDirectoryName
        manifest = [pscustomobject]@{
          file_name = $script:CodeHangarFrontendSnapshotManifestFileName
          length = (Get-Item -LiteralPath $validatedReceipt.Frontend.Manifest.Path).Length
          sha256 = (Get-FileHash -LiteralPath $validatedReceipt.Frontend.Manifest.Path -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        tree = [pscustomobject]@{
          file_count = $validatedReceipt.Frontend.Tree.Count
          sha256 = $validatedReceipt.Frontend.Tree.Sha256
        }
      }
      $frontendEditionRejected = $false
      try {
        [void](Read-AndValidateCodeHangarFrontendSnapshot `
          -SigningDirectory $signingDir `
          -Edition $Edition `
          -FrontendReceipt $forgedFrontendReceipt)
      } catch {
        if ($_.Exception.Message -like "*manifest edition does not match*") { $frontendEditionRejected = $true } else { throw }
      }
      if (-not $frontendEditionRejected) { throw "Frontend snapshot manifest edition-mismatch self-test did not fail closed." }
    } finally {
      [System.IO.File]::WriteAllBytes($validatedReceipt.Frontend.Manifest.Path, $frontendManifestBytes)
    }

    $frontendRestoreRepo = Join-Path $tempRoot "frontend-restore-repo"
    $frontendRestoreDist = Join-Path $frontendRestoreRepo "apps\desktop\dist"
    [void][System.IO.Directory]::CreateDirectory((Join-Path $frontendRestoreDist "assets"))
    [System.IO.File]::WriteAllText((Join-Path $frontendRestoreDist "index.html"), "<html>prior worktree frontend</html>", [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText((Join-Path $frontendRestoreDist "assets\edition-marker.txt"), "prior-worktree-marker", [System.Text.UTF8Encoding]::new($false))
    $priorFrontendTree = Get-CanonicalReleaseTreeEvidence -Root $frontendRestoreDist -Schema $script:CodeHangarFrontendSnapshotSchema -Label "Self-test prior worktree frontend"
    $frontendRestoreRun = Join-Path $tempRoot "frontend-restore-run"
    [void][System.IO.Directory]::CreateDirectory($frontendRestoreRun)
    $frontendRestoreContext = Enter-CodeHangarFrontendSnapshotBundleContext `
      -RepoRoot $frontendRestoreRepo `
      -FrontendSnapshot $validatedReceipt.Frontend `
      -RunDirectory $frontendRestoreRun
    try {
      Assert-CodeHangarFrontendSnapshotBundleContextState -Context $frontendRestoreContext
      $activeFrontendWriterRejected = $false
      try {
        $unexpectedWriter = [System.IO.FileStream]::new(
          (Join-Path $frontendRestoreDist "assets\edition-marker.txt"),
          [System.IO.FileMode]::Open,
          [System.IO.FileAccess]::Write,
          [System.IO.FileShare]::Read
        )
        $unexpectedWriter.Dispose()
      } catch [System.IO.IOException] {
        $activeFrontendWriterRejected = $true
      }
      if (-not $activeFrontendWriterRejected) { throw "Receipt-bound active frontend snapshot allowed a writer during bundle self-test." }
    } finally {
      Restore-CodeHangarFrontendSnapshotBundleContext -Context $frontendRestoreContext
    }
    $restoredFrontendTree = Get-CanonicalReleaseTreeEvidence -Root $frontendRestoreDist -Schema $script:CodeHangarFrontendSnapshotSchema -Label "Self-test restored worktree frontend"
    if ($restoredFrontendTree.Count -ne $priorFrontendTree.Count -or $restoredFrontendTree.Sha256 -cne $priorFrontendTree.Sha256) {
      throw "Frontend snapshot bundle context did not restore the prior worktree dist exactly."
    }

    foreach ($mismatch in @(
        @{ Parameter = "Edition"; Value = $otherEdition; Pattern = "*edition does not match*"; Label = "edition" },
        @{ Parameter = "ExpectedVersion"; Value = "9.9.9"; Pattern = "*version does not match*"; Label = "version" },
        @{ Parameter = "ExpectedTargetTriple"; Value = "aarch64-pc-windows-msvc"; Pattern = "*target triple does not match*"; Label = "target triple" },
        @{ Parameter = "ExpectedPublicBlobHex"; Value = ('56' * 411); Pattern = "*release root does not match*"; Label = "release root" },
        @{ Parameter = "ExpectedCargoLockSha256"; Value = ('34' * 32); Pattern = "*Cargo.lock changed after PrepareSigning*"; Label = "Cargo.lock" },
        @{ Parameter = "ExpectedBundleContractSha256"; Value = ('9a' * 32); Pattern = "*bundle contract changed after PrepareSigning*"; Label = "bundle contract" },
        @{ Parameter = "ExpectedSourceCommit"; Value = ('ef' * 20); Pattern = "*source commit/tree does not match*"; Label = "source commit" },
        @{ Parameter = "ExpectedSourceTree"; Value = ('01' * 20); Pattern = "*source commit/tree does not match*"; Label = "source tree" }
      )) {
      $mismatchArgs = @{}
      foreach ($key in $receiptValidationArgs.Keys) { $mismatchArgs[$key] = $receiptValidationArgs[$key] }
      $mismatchArgs[$mismatch.Parameter] = $mismatch.Value
      $mismatchRejected = $false
      try {
        [void](Read-AndValidateCodeHangarSigningReceipt @mismatchArgs)
      } catch {
        if ($_.Exception.Message -like $mismatch.Pattern) { $mismatchRejected = $true } else { throw }
      }
      if (-not $mismatchRejected) {
        throw "Signing-receipt $($mismatch.Label) mismatch self-test did not fail closed."
      }
    }

    $wrongReceiptHashRejected = $false
    try {
      $wrongHashArgs = @{}
      foreach ($key in $receiptValidationArgs.Keys) { $wrongHashArgs[$key] = $receiptValidationArgs[$key] }
      $wrongHashArgs.ExpectedReceiptSha256 = ('34' * 32)
      [void](Read-AndValidateCodeHangarSigningReceipt @wrongHashArgs)
    } catch {
      if ($_.Exception.Message -like "*does not match -ExpectedSigningReceiptSha256*") { $wrongReceiptHashRejected = $true } else { throw }
    }
    if (-not $wrongReceiptHashRejected) { throw "Wrong external signing-receipt hash self-test did not fail closed." }

    if ($Edition -eq "Connector") {
      $originalMcpBytes = [System.IO.File]::ReadAllBytes($preparedPaths.Mcp)
      $tamperedMcpRejected = $false
      try {
        [System.IO.File]::AppendAllText($preparedPaths.Mcp, "tampered")
        try {
          [void](Read-AndValidateCodeHangarSigningReceipt @receiptValidationArgs)
        } catch {
          if ($_.Exception.Message -like "*prepared MCP sidecar no longer matches*") { $tamperedMcpRejected = $true } else { throw }
        }
      } finally {
        [System.IO.File]::WriteAllBytes($preparedPaths.Mcp, $originalMcpBytes)
      }
      if (-not $tamperedMcpRejected) { throw "Tampered receipt-bound MCP self-test did not fail closed." }
    }

    [System.IO.File]::AppendAllText($preparedPaths.Verifier, "tampered")
    $tamperedVerifierRejected = $false
    try {
      [void](Read-AndValidateCodeHangarSigningReceipt @receiptValidationArgs)
    } catch {
      if ($_.Exception.Message -like "*prepared release verifier no longer matches*") { $tamperedVerifierRejected = $true } else { throw }
    }
    if (-not $tamperedVerifierRejected) { throw "Tampered receipt-bound verifier self-test did not fail closed." }

    $copySource = Join-Path $tempRoot "exact-copy-source.exe"
    $copyDestination = Join-Path $tempRoot "exact-copy-destination.exe"
    Write-PackagingSelfTestPe -Path $copySource
    [System.IO.File]::AppendAllText($copySource, "exact")
    Write-PackagingSelfTestPe -Path $copyDestination
    $copySourceEvidence = Get-StableReleaseArtifactEvidence -Path $copySource -Label "Self-test exact-copy source" -RequirePe
    $copyResult = Set-ExactReleaseArtifact -SourcePath $copySource -DestinationPath $copyDestination -Label "Self-test exact-copy destination" -ExpectedSha256 $copySourceEvidence.Sha256 -RequirePe
    if ($copyResult.Sha256 -cne $copySourceEvidence.Sha256) { throw "Exact artifact placement self-test changed bytes." }

    $contractRoot = Join-Path $tempRoot "bundle-contract"
    [void][System.IO.Directory]::CreateDirectory($contractRoot)
    $contractInputs = @(
      "package.json",
      "package-lock.json",
      "Cargo.toml",
      "apps/desktop/package.json",
      "apps/desktop/src-tauri/Cargo.toml",
      "apps/desktop/src-tauri/tauri.conf.json",
      "apps/desktop/src-tauri/tauri.connector.conf.json",
      "apps/desktop/src-tauri/tauri.release-local.conf.json",
       "apps/desktop/src-tauri/tauri.release-connector.conf.json",
       "apps/desktop/src-tauri/windows/shell-integration.nsh",
      "scripts/release-inputs/webview2-x64.json",
      "scripts/WebView2Authenticode.cs",
      "scripts/check-frontend-edition.mjs",
      "scripts/new-release-identity-manifest.ps1",
       "scripts/packaging-common.ps1",
      "scripts/packaging-preflight.mjs",
      "scripts/package-local.ps1",
      "scripts/package-connector.ps1"
    )
    foreach ($relative in $contractInputs) {
      $path = Join-Path $contractRoot $relative
      [void][System.IO.Directory]::CreateDirectory((Split-Path -Parent $path))
      [System.IO.File]::WriteAllText($path, "fixture:$relative", [System.Text.UTF8Encoding]::new($false))
    }
    $contractBefore = Get-CodeHangarBundleContractSha256 -RepoRoot $contractRoot -Edition $Edition
    $editionWrapper = if ($Edition -eq "Connector") { "scripts/package-connector.ps1" } else { "scripts/package-local.ps1" }
    [System.IO.File]::AppendAllText((Join-Path $contractRoot $editionWrapper), "changed")
    $contractAfter = Get-CodeHangarBundleContractSha256 -RepoRoot $contractRoot -Edition $Edition
    if ($contractAfter -ceq $contractBefore) { throw "Bundle-contract drift self-test did not change the digest." }

    $treeRoot = Join-Path $tempRoot "canonical-tree"
    [void][System.IO.Directory]::CreateDirectory((Join-Path $treeRoot "nested"))
    [System.IO.File]::WriteAllText((Join-Path $treeRoot "a.txt"), "a")
    [System.IO.File]::WriteAllText((Join-Path $treeRoot "nested\b.txt"), "b")
    $treeBefore = Get-CanonicalReleaseTreeEvidence -Root $treeRoot -Schema "codehangar/self-test-tree/1" -Label "Self-test canonical tree"
    if ($treeBefore.Count -ne 2 -or $treeBefore.Sha256 -cnotmatch '^[0-9a-f]{64}$') { throw "Canonical tree self-test returned invalid evidence." }
    [System.IO.File]::AppendAllText((Join-Path $treeRoot "nested\b.txt"), "changed")
    $treeAfter = Get-CanonicalReleaseTreeEvidence -Root $treeRoot -Schema "codehangar/self-test-tree/1" -Label "Self-test canonical tree"
    if ($treeAfter.Sha256 -ceq $treeBefore.Sha256) { throw "Canonical tree drift self-test did not change the digest." }
  } finally {
    if ($null -ne $lock) { $lock.Dispose() }
    $resolvedRoot = [System.IO.Path]::GetFullPath($tempRoot)
    if (([System.IO.Path]::GetDirectoryName($resolvedRoot)).Equals($tempParent, [System.StringComparison]::OrdinalIgnoreCase) -and
        ([System.IO.Path]::GetFileName($resolvedRoot)).StartsWith("codehangar-packaging-selftest-", [System.StringComparison]::Ordinal)) {
      Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    } else {
      throw "Refusing unsafe packaging self-test cleanup: $resolvedRoot"
    }
  }
  Write-Host "Packaging common self-test passed for $Edition."
}
