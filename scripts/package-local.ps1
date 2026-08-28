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

# Canonical LOCAL release wrapper. It has no implicit packaging mode: an owner
# must explicitly prepare inner binaries for signing or bundle already signed,
# release-manifest-bound bytes. Publication remains outside this script.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot "packaging-common.ps1")

$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
Set-Location $repoRoot

Invoke-CodeHangarReleasePackaging `
  -Edition Local `
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
