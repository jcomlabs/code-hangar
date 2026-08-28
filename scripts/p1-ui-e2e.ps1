[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$desktopRoot = Join-Path $repoRoot "apps\desktop"
$vite = Join-Path $repoRoot "node_modules\.bin\vite.cmd"
$tempParent = [System.IO.Path]::GetTempPath()
$workspaceLeaf = "code-hangar-p1-ui-e2e-run-$PID-$([Guid]::NewGuid().ToString('N'))"
$workspaceRoot = Join-Path $tempParent $workspaceLeaf

function Remove-VerifiedP1E2EWorkspace {
  if (-not (Test-Path -LiteralPath $workspaceRoot)) { return }

  $canonicalTemp = (Resolve-Path -LiteralPath $tempParent -ErrorAction Stop).Path.TrimEnd('\')
  $item = Get-Item -LiteralPath $workspaceRoot -Force -ErrorAction Stop
  if ($item.Name -cne $workspaceLeaf) {
    throw "P1-UI-E2E-01 cleanup refused because the exact temp leaf drifted."
  }
  if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "P1-UI-E2E-01 cleanup refused because the workspace became a reparse point."
  }

  $canonicalWorkspace = (Resolve-Path -LiteralPath $workspaceRoot -ErrorAction Stop).Path
  $canonicalParent = [System.IO.Path]::GetDirectoryName($canonicalWorkspace).TrimEnd('\')
  if ($canonicalParent -cne $canonicalTemp -or
      [System.IO.Path]::GetFileName($canonicalWorkspace) -cne $workspaceLeaf) {
    throw "P1-UI-E2E-01 cleanup refused because the canonical temp boundary drifted."
  }

  Remove-Item -LiteralPath $canonicalWorkspace -Recurse -Force -ErrorAction Stop
  if (Test-Path -LiteralPath $workspaceRoot) {
    throw "P1-UI-E2E-01 cleanup did not remove the verified temp workspace."
  }
}

if (-not $IsWindows) {
  throw "P1-UI-E2E-01 requires Windows WebView2 so it can cross the real Tauri/Wry IPC boundary."
}
if (-not (Test-Path -LiteralPath $vite -PathType Leaf)) {
  throw "P1-UI-E2E-01 requires the repository-local desktop dependencies; run the normal offline bootstrap first."
}

Push-Location $desktopRoot
try {
  & $vite build --config vite.p1-e2e.config.ts --mode offline
  if ($LASTEXITCODE -ne 0) { throw "P1-UI-E2E-01 frontend build failed." }
} finally {
  Pop-Location
}

Push-Location $repoRoot
try {
  & cargo run --locked --offline --package code-hangar-desktop --bin p1_ui_e2e --features p1_ui_e2e -- $workspaceLeaf
  $nativeExitCode = $LASTEXITCODE
} finally {
  Pop-Location
  Remove-VerifiedP1E2EWorkspace
  if (Test-Path -LiteralPath $workspaceRoot) {
    throw "P1-UI-E2E-01 cleanup failed to remove the exact temp workspace after process exit."
  }
  Write-Host "P1-UI-E2E-01 cleanup PASS: exact temp workspace absent after process exit."
}

if ($nativeExitCode -ne 0) { throw "P1-UI-E2E-01 native journey failed." }
