[CmdletBinding()]
param(
  [string]$EvidenceDir
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance\v0.1.1\mcp-claude-real"))
if ([string]::IsNullOrWhiteSpace($EvidenceDir)) {
  $EvidenceDir = Join-Path $acceptanceRoot (Get-Date -Format "yyyyMMdd-HHmmss")
}
$EvidenceDir = [System.IO.Path]::GetFullPath($EvidenceDir)
$allowedPrefix = $acceptanceRoot.TrimEnd("\") + "\"
if (-not $EvidenceDir.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "EvidenceDir must stay under $acceptanceRoot"
}
New-Item -ItemType Directory -Path $EvidenceDir -Force | Out-Null

function Write-JsonFile {
  param([string]$Path, [object]$Value)
  $Value | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $Path -Encoding utf8
}

$reportPath = Join-Path $EvidenceDir "claude-real-mcp.json"
$authText = (& claude auth status 2>&1 | Out-String).Trim()
$authExit = $LASTEXITCODE
$auth = try { $authText | ConvertFrom-Json } catch { $null }
if ($authExit -ne 0 -or $null -eq $auth -or -not [bool]$auth.loggedIn) {
  $blocked = [pscustomobject]@{
    schemaVersion = 1
    status = "BLOCKED"
    checkedAt = (Get-Date).ToString("o")
    reason = "Claude Code is not logged in. Run 'claude auth login' manually, then rerun this script."
    loggedIn = $false
  }
  Write-JsonFile -Path $reportPath -Value $blocked
  Write-Host "Claude real MCP test is blocked until manual login. Evidence: $reportPath" -ForegroundColor Yellow
  exit 2
}

$server = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "target\release\code-hangar-mcp.exe"))
if (-not (Test-Path -LiteralPath $server -PathType Leaf)) {
  throw "Connected-app sidecar not found: $server"
}
$fixtureRoot = Join-Path $EvidenceDir "fixture"
$fixturePath = Join-Path $fixtureRoot "fixture.json"
$strictConfigPath = Join-Path $EvidenceDir "claude-strict-mcp.json"
$clientOutputPath = Join-Path $EvidenceDir "claude-client.json"
$clientErrorPath = Join-Path $EvidenceDir "claude-client.stderr.log"
$failure = $null
$disconnectFailure = $null
$prepared = $false
$clientExitCode = $null
$clientResultText = $null
$auditPath = $null
$disconnectPath = $null

try {
  cargo run -q -p code-hangar-mcp --example acceptance_fixture -- prepare $fixtureRoot $server
  if ($LASTEXITCODE -ne 0) {
    throw "MCP fixture preparation failed with exit code $LASTEXITCODE"
  }
  $prepared = $true
  $fixture = Get-Content -LiteralPath $fixturePath -Raw | ConvertFrom-Json
  $claudeConfig = Get-Content -LiteralPath $fixture.clients.claude.configPath -Raw | ConvertFrom-Json
  $serverSpec = $claudeConfig.mcpServers.'code-hangar'
  if ($null -eq $serverSpec) {
    throw "Prepared Claude config has no code-hangar server."
  }
  Write-JsonFile -Path $strictConfigPath -Value ([pscustomobject]@{
    mcpServers = [ordered]@{ "code-hangar" = $serverSpec }
  })

  $prompt = "Use only the Code Hangar MCP server. Call list_catalog, find the project named exactly 'Fixture Git-like Project', then call get_project_context with its numeric projectId. Reply with the exact project name and its context file names."
  $claudeArgs = @(
    "-p", $prompt,
    "--output-format", "json",
    "--no-session-persistence",
    "--strict-mcp-config",
    "--mcp-config", $strictConfigPath,
    "--permission-mode", "dontAsk",
    "--allowedTools", "mcp__code-hangar__list_catalog,mcp__code-hangar__get_project_context",
    "--setting-sources", "project",
    "--disable-slash-commands",
    "--no-chrome",
    "--max-budget-usd", "0.50"
  )
  Push-Location $fixture.root
  try {
    $clientLines = @(& claude @claudeArgs 2> $clientErrorPath)
    $clientExitCode = $LASTEXITCODE
  } finally {
    Pop-Location
  }
  $clientJson = ($clientLines -join "`n").Trim()
  Set-Content -LiteralPath $clientOutputPath -Value $clientJson -Encoding utf8
  if ($clientExitCode -ne 0) {
    throw "Claude client exited with code $clientExitCode."
  }
  $client = $clientJson | ConvertFrom-Json
  $clientResultText = [string]$client.result
  if ([bool]$client.is_error) {
    throw "Claude returned an error result: $clientResultText"
  }
  if ($clientResultText -notmatch 'Fixture Git-like Project') {
    throw "Claude result did not identify the fixture project."
  }

  cargo run -q -p code-hangar-mcp --example acceptance_fixture -- audit-host $fixturePath claude list_catalog project_context
  if ($LASTEXITCODE -ne 0) {
    throw "Claude MCP activity audit failed with exit code $LASTEXITCODE"
  }
  $auditPath = Join-Path $fixtureRoot "mcp-audit-claude.json"
  $audit = Get-Content -LiteralPath $auditPath -Raw | ConvertFrom-Json
  if ($audit.status -ne "PASS") {
    throw "Claude MCP activity audit did not pass."
  }
} catch {
  $failure = $_
} finally {
  if ($prepared -and (Test-Path -LiteralPath $fixturePath -PathType Leaf)) {
    try {
      cargo run -q -p code-hangar-mcp --example acceptance_fixture -- disconnect $fixturePath
      if ($LASTEXITCODE -ne 0) {
        throw "MCP fixture disconnect failed with exit code $LASTEXITCODE"
      }
      $disconnectPath = Join-Path $fixtureRoot "mcp-disconnect.json"
    } catch {
      $disconnectFailure = $_
    }
  }
}

$report = [pscustomobject]@{
  schemaVersion = 1
  status = if ($null -eq $failure -and $null -eq $disconnectFailure) { "PASS" } else { "FAIL" }
  completedAt = (Get-Date).ToString("o")
  gitCommit = (git -C $repoRoot rev-parse HEAD).Trim()
  claudeVersion = (& claude --version | Out-String).Trim()
  authMethod = [string]$auth.authMethod
  serverSha256 = (Get-FileHash -LiteralPath $server -Algorithm SHA256).Hash.ToLowerInvariant()
  clientExitCode = $clientExitCode
  projectObserved = $clientResultText -match 'Fixture Git-like Project'
  requiredMethods = @("list_catalog", "project_context")
  audit = if ($null -ne $auditPath) { [System.IO.Path]::GetRelativePath($repoRoot, $auditPath) } else { $null }
  disconnect = if ($null -ne $disconnectPath) { [System.IO.Path]::GetRelativePath($repoRoot, $disconnectPath) } else { $null }
  failure = if ($null -ne $failure) { $failure.Exception.Message } else { $null }
  disconnectFailure = if ($null -ne $disconnectFailure) { $disconnectFailure.Exception.Message } else { $null }
}
Write-JsonFile -Path $reportPath -Value $report

if ($null -ne $disconnectFailure) {
  throw "Claude MCP cleanup failed: $($disconnectFailure.Exception.Message). Evidence: $reportPath"
}
if ($null -ne $failure) {
  throw "Claude real MCP test failed: $($failure.Exception.Message). Evidence: $reportPath"
}

Write-Host "Claude real MCP lifecycle passed. Evidence: $reportPath" -ForegroundColor Green
