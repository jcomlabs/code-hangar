[CmdletBinding()]
param(
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

# Canonical CONNECTOR release wrapper. MCP is receipt-bound at PrepareSigning;
# only parent/helper bytes may change through the external Authenticode gate.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot "packaging-common.ps1")

$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
Set-Location $repoRoot

Invoke-CodeHangarReleasePackaging `
  -Edition Connector `
  -RepoRoot $repoRoot `
  -WebView2InstallerPath $WebView2InstallerPath `
  -PreflightOnly:$PreflightOnly `
  -PrepareSigning:$PrepareSigning `
  -BundleSigned:$BundleSigned `
  -ReleaseRootPublicBlobHex $ReleaseRootPublicBlobHex `
  -SigningDirectory $SigningDirectory `
  -ExpectedSigningReceiptSha256 $ExpectedSigningReceiptSha256 `
  -SignedParentPath $SignedParentPath `
  -SignedHelperPath $SignedHelperPath `
  -ReleaseManifestPath $ReleaseManifestPath `
  -SelfTest:$SelfTest
