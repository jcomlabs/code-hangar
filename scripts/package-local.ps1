# Builds the LOCAL edition installer: mutation-enabled, no AI connector, no
# outbound-network dependencies. This is the edition most users should install.
#
# Output: target/release/bundle/nsis/Code Hangar_*-setup.exe
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

# Make cargo reachable regardless of how the shell was launched.
$cargoBins = @(
  (Join-Path $env:USERPROFILE ".cargo\bin"),
  (Join-Path $env:USERPROFILE ".local\cargo\bin")
)
foreach ($cargoBin in $cargoBins) {
  if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
}

# A Connector build stages the MCP sidecar under src-tauri/binaries. The Local
# config does not declare externalBin, but remove stale sidecars before bundling
# so the workspace and release artifacts are unambiguous.
$sidecarDir = Join-Path $repoRoot "apps\desktop\src-tauri\binaries"
if (Test-Path $sidecarDir) {
  Get-ChildItem -Path $sidecarDir -Filter "code-hangar-mcp-*.exe" -File -ErrorAction SilentlyContinue |
    Remove-Item -Force
}

Set-Location (Join-Path $repoRoot "apps\desktop")
& npx tauri build --features mutation
exit $LASTEXITCODE
