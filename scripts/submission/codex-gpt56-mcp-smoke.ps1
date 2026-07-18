[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$EvidenceDir,
  [string]$ServerPath = ""
)

$ErrorActionPreference = "Stop"

function Get-CodexSessionFiles([string]$Root) {
  if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
    return @()
  }
  return @(
    Get-ChildItem -LiteralPath $Root -Filter "rollout-*.jsonl" -File -Recurse |
      ForEach-Object { $_.FullName } |
      Sort-Object
  )
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance"))
$evidenceRoot = [System.IO.Path]::GetFullPath($EvidenceDir)
$allowedPrefix = $acceptanceRoot.TrimEnd("\") + "\"
if (-not $evidenceRoot.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "EvidenceDir must stay under $acceptanceRoot"
}
if (Test-Path -LiteralPath $evidenceRoot) {
  throw "Refusing to overwrite an existing evidence directory: $evidenceRoot"
}

$explicitServer = -not [string]::IsNullOrWhiteSpace($ServerPath)
$server = if ($explicitServer) {
  [System.IO.Path]::GetFullPath($ServerPath)
} else {
  [System.IO.Path]::GetFullPath(
    (Join-Path $repoRoot "target\release\code-hangar-mcp.exe")
  )
}
if (-not (Test-Path -LiteralPath $server -PathType Leaf)) {
  throw "Connected-app sidecar not found: $server"
}
if ([System.IO.Path]::GetFileName($server) -ne "code-hangar-mcp.exe") {
  throw "Connected-app sidecar must be named code-hangar-mcp.exe"
}
$serverSha256 = (Get-FileHash -LiteralPath $server -Algorithm SHA256).Hash.ToLowerInvariant()

$strictErrorPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$loginStatus = (& codex login status 2>&1 | Out-String).Trim()
$loginExitCode = $LASTEXITCODE
$codexVersion = (& codex --version 2>&1 | Out-String).Trim()
$versionExitCode = $LASTEXITCODE
$ErrorActionPreference = $strictErrorPreference
if ($loginExitCode -ne 0 -or $loginStatus -notmatch "Logged in using ChatGPT") {
  throw "Codex must already be signed in with ChatGPT for the subscription-backed proof."
}
if ($versionExitCode -ne 0) {
  throw "Could not read the Codex CLI version."
}

New-Item -ItemType Directory -Path $evidenceRoot -Force | Out-Null
$fixtureRoot = Join-Path $evidenceRoot "fixture"
$fixturePath = Join-Path $fixtureRoot "fixture.json"
$runtimeBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$runtimeCwd = [System.IO.Path]::GetFullPath(
  (Join-Path $runtimeBase ("codehangar-gpt56-mcp-" + [Guid]::NewGuid().ToString("N")))
)
$runtimePrefix = $runtimeBase.TrimEnd("\") + "\"
if (-not $runtimeCwd.StartsWith($runtimePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Temporary Codex working directory escaped the system temp root."
}
New-Item -ItemType Directory -Path $runtimeCwd -Force | Out-Null
$codexSessionRoot = Join-Path ([Environment]::GetFolderPath("UserProfile")) ".codex\sessions"
$sessionFilesBefore = @(Get-CodexSessionFiles $codexSessionRoot)
$prepared = $false
$disconnected = $false
$failure = $null
$proof = $null

try {
  cargo run -q -p code-hangar-mcp --example acceptance_fixture -- prepare $fixtureRoot $server
  if ($LASTEXITCODE -ne 0) {
    throw "MCP fixture preparation failed with exit code $LASTEXITCODE"
  }
  $prepared = $true
  $fixture = Get-Content -LiteralPath $fixturePath -Raw | ConvertFrom-Json

  $env:CODEHANGAR_MCP_TOKEN = [string]$fixture.clients.codex.token
  $env:CODEHANGAR_DB_PATH = [string]$fixture.dbPath
  $serverToml = ([string]$fixture.serverPath).Replace("\", "/")
  $serverConfig = "mcp_servers.code-hangar.command='$serverToml'"
  $envConfig = "mcp_servers.code-hangar.env_vars=['CODEHANGAR_MCP_TOKEN','CODEHANGAR_DB_PATH']"
  $prompt = @"
Use only the code-hangar MCP tools. Call list_catalog, then get_project_context
for the only project. Do not run shell commands or read workspace files. Reply
in exactly three lines:
MODEL=gpt-5.6-sol
PROJECT=<the exact project name>
SOURCE=code-hangar MCP
"@

  $ErrorActionPreference = "Continue"
  $codexOutput = @(
    & codex exec `
      --ephemeral `
      --ignore-user-config `
      --skip-git-repo-check `
      -C $runtimeCwd `
      -m gpt-5.6-sol `
      -s read-only `
      -c "approval_policy='never'" `
      -c $serverConfig `
      -c $envConfig `
      -c 'mcp_servers.code-hangar.required=true' `
      --json `
      $prompt 2>&1
  )
  $codexExitCode = $LASTEXITCODE
  $ErrorActionPreference = $strictErrorPreference
  if ($codexExitCode -ne 0) {
    $codexOutput | Select-Object -Last 30 | Write-Host
    throw "Codex MCP proof failed with exit code $codexExitCode"
  }

  $sessionFilesAfter = @(Get-CodexSessionFiles $codexSessionRoot)
  $newSessionFiles = @(
    Compare-Object -ReferenceObject $sessionFilesBefore -DifferenceObject $sessionFilesAfter |
      Where-Object { $_.SideIndicator -eq "=>" } |
      ForEach-Object { [string]$_.InputObject }
  )
  if ($newSessionFiles.Count -ne 0) {
    throw "Ephemeral Codex proof unexpectedly created a persisted session file."
  }

  $events = @(
    $codexOutput | ForEach-Object {
      try {
        $_ | ConvertFrom-Json -ErrorAction Stop
      } catch {
        # Diagnostics are intentionally ignored. Only structured Codex events
        # contribute to the redacted proof.
      }
    }
  )
  $finalText = @(
    $events |
      Where-Object {
        $_.type -eq "item.completed" -and
        $_.item.type -eq "agent_message"
      } |
      ForEach-Object { [string]$_.item.text }
  )[-1]

  if (
    $finalText -notmatch "MODEL=gpt-5\.6-sol" -or
    $finalText -notmatch "PROJECT=Fixture Git-like Project" -or
    $finalText -notmatch "SOURCE=code-hangar MCP"
  ) {
    throw "GPT-5.6 returned an unexpected final proof response."
  }

  cargo run -q -p code-hangar-mcp --example acceptance_fixture -- `
    audit-host $fixturePath codex list_catalog project_context
  if ($LASTEXITCODE -ne 0) {
    throw "Code Hangar did not audit both required Codex MCP reads."
  }

  $proof = [ordered]@{
    schemaVersion = 1
    status = "PASS"
    executedAtUtc = [DateTimeOffset]::UtcNow.ToString("o")
    codexVersion = $codexVersion
    authentication = "ChatGPT subscription"
    requestedModel = "gpt-5.6-sol"
    resolvedModel = "gpt-5.6-sol"
    transport = "Codex local client plus Code Hangar MCP over stdio"
    serverSource = if ($explicitServer) { "explicit installed candidate sidecar" } else { "repository release sidecar" }
    serverSha256 = $serverSha256
    ephemeralSession = $true
    persistedSessionFilesCreated = 0
    workingDirectory = "disposable system temporary directory"
    project = "Fixture Git-like Project"
    requiredMcpReads = @("list_catalog", "project_context")
    finalResponse = $finalText
    containsCredential = $false
    containsPersonalProjectData = $false
  }
} catch {
  $failure = $_
} finally {
  Remove-Item Env:CODEHANGAR_MCP_TOKEN -ErrorAction SilentlyContinue
  Remove-Item Env:CODEHANGAR_DB_PATH -ErrorAction SilentlyContinue

  if ($prepared -and (Test-Path -LiteralPath $fixturePath)) {
    cargo run -q -p code-hangar-mcp --example acceptance_fixture -- disconnect $fixturePath
    $disconnected = $LASTEXITCODE -eq 0
  }

  if (Test-Path -LiteralPath $fixtureRoot) {
    $resolvedFixture = [System.IO.Path]::GetFullPath($fixtureRoot)
    if (-not $resolvedFixture.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Fixture cleanup target escaped the acceptance root."
    }
    Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
  }

  if (Test-Path -LiteralPath $runtimeCwd) {
    $resolvedRuntime = [System.IO.Path]::GetFullPath($runtimeCwd)
    if (-not $resolvedRuntime.StartsWith($runtimePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Temporary Codex cleanup target escaped the system temp root."
    }
    Remove-Item -LiteralPath $resolvedRuntime -Recurse -Force
  }
}

if (-not $disconnected) {
  throw "The temporary Codex MCP credential was not successfully revoked."
}
if ($null -ne $failure) {
  throw $failure
}

$reportPath = Join-Path $evidenceRoot "codex-gpt56-mcp-proof.json"
$proof | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding utf8
Write-Host "GPT-5.6 subscription + Code Hangar MCP proof passed: $reportPath" -ForegroundColor Green
