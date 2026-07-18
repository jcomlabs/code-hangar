# Builds the CONNECTOR edition installer: the agent_automation desktop build that
# also ships code-hangar-mcp.exe next to it (as a Tauri sidecar), so a user can
# connect their AI apps. The Local edition (npm run package:local) links none of this.
#
# Output: target/release/bundle/nsis/Code Hangar AI Connector_*-setup.exe
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

# 1. Build the connected-app server (the MCP stdio binary).
cargo build -p code-hangar-mcp --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$serverExe = Join-Path $repoRoot "target\release\code-hangar-mcp.exe"
if (-not (Test-Path $serverExe)) {
  throw "Connected-app server artifact was not produced: $serverExe"
}

# 2. Stage it as a Tauri sidecar. The triple-suffixed name is the sidecar naming
#    convention; Tauri strips the triple and ships "code-hangar-mcp.exe" right next
#    to the desktop exe — exactly where connected_app_server_path() looks for it.
$triple = ((rustc -Vv) | Where-Object { $_ -like 'host:*' }) -replace 'host:\s*', ''
if ([string]::IsNullOrWhiteSpace($triple)) {
  throw "Could not determine the host target triple from rustc."
}
$sidecarDir = Join-Path $repoRoot "apps\desktop\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $sidecarDir | Out-Null
$sidecar = Join-Path $sidecarDir "code-hangar-mcp-$triple.exe"
Copy-Item -Path $serverExe -Destination $sidecar -Force
Write-Host "Staged connector sidecar: $sidecar"

# 3. Bundle the connector edition (agent_automation build + the sidecar resource).
Set-Location (Join-Path $repoRoot "apps\desktop")
& npx tauri build --features agent_automation --config src-tauri/tauri.connector.conf.json
exit $LASTEXITCODE
