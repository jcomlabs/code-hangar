# Generate SHA256SUMS for the built installers. Run this AFTER signing and right
# before publishing, so the checksums match exactly what you upload.
#
# Output: target/release/bundle/nsis/release-assets/ with portable installer
# names plus SHA256SUMS (one "<hash>  <filename>" per line). GitHub normalizes
# spaces in uploaded asset names, so the release copies deliberately use only
# stable URL/file-safe characters.
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$nsisDir = Join-Path $repoRoot "target\release\bundle\nsis"
$tauriConfigPath = Join-Path $repoRoot "apps\desktop\src-tauri\tauri.conf.json"
if (-not (Test-Path $nsisDir)) {
  throw "No NSIS bundle directory at $nsisDir. Build the installers first (package:local / package:connector)."
}
if (-not (Test-Path $tauriConfigPath)) {
  throw "No Tauri config at $tauriConfigPath."
}

$version = [string]((Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json).version)
if ([string]::IsNullOrWhiteSpace($version)) {
  throw "Tauri config does not contain a release version."
}

$assetNames = [ordered]@{
  "Code Hangar AI Connector_$($version)_x64-setup.exe" = "Code-Hangar-AI-Connector_$($version)_x64-setup.exe"
  "Code Hangar_$($version)_x64-setup.exe" = "Code-Hangar_$($version)_x64-setup.exe"
}
$assetsDir = Join-Path $nsisDir "release-assets"
New-Item -ItemType Directory -Path $assetsDir -Force | Out-Null

$releaseAssets = foreach ($entry in $assetNames.GetEnumerator()) {
  $sourcePath = Join-Path $nsisDir $entry.Key
  if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
    throw "Missing current-version installer: $sourcePath. Build both editions sequentially before checksumming."
  }
  $destinationPath = Join-Path $assetsDir $entry.Value
  Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
  $sourceHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
  $destinationHash = (Get-FileHash -LiteralPath $destinationPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($sourceHash -ne $destinationHash) {
    throw "Release-asset copy hash mismatch: $($entry.Value)"
  }
  [pscustomobject]@{
    File = Get-Item -LiteralPath $destinationPath
    Hash = $destinationHash
  }
}
$releaseAssets = $releaseAssets | Sort-Object { $_.File.Name }

$lines = foreach ($asset in $releaseAssets) {
  "$($asset.Hash)  $($asset.File.Name)"
}

$out = Join-Path $assetsDir "SHA256SUMS"
Set-Content -Path $out -Value $lines -Encoding ascii
Write-Host "Wrote $out"
$lines | ForEach-Object { Write-Host $_ }
