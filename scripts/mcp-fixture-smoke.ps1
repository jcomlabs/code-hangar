[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$EvidenceDir
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$acceptanceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".local\acceptance"))
$EvidenceDir = [System.IO.Path]::GetFullPath($EvidenceDir)
$allowedPrefix = $acceptanceRoot.TrimEnd("\") + "\"
if (-not $EvidenceDir.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "EvidenceDir must stay under $acceptanceRoot"
}
New-Item -ItemType Directory -Path $EvidenceDir -Force | Out-Null

$server = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "target\release\code-hangar-mcp.exe"))
if (-not (Test-Path -LiteralPath $server -PathType Leaf)) {
  throw "Connected-app sidecar not found: $server"
}
$fixtureRoot = Join-Path $EvidenceDir "mcp-fixture"
$fixturePath = Join-Path $fixtureRoot "fixture.json"
if (Test-Path -LiteralPath $fixturePath) {
  throw "Refusing to overwrite an existing MCP fixture: $fixturePath"
}

$failure = $null
$disconnectFailure = $null
$clientResults = [System.Collections.Generic.List[object]]::new()
try {
  cargo run -q -p code-hangar-mcp --example acceptance_fixture -- prepare $fixtureRoot $server
  if ($LASTEXITCODE -ne 0) { throw "MCP fixture preparation failed with exit code $LASTEXITCODE" }
  $fixture = Get-Content -LiteralPath $fixturePath -Raw | ConvertFrom-Json

  foreach ($clientProperty in $fixture.clients.PSObject.Properties) {
    $hostId = $clientProperty.Name
    $client = $clientProperty.Value
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $fixture.serverPath
    $startInfo.WorkingDirectory = $fixture.root
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.Environment["CODEHANGAR_MCP_TOKEN"] = $client.token
    $startInfo.Environment["CODEHANGAR_DB_PATH"] = $fixture.dbPath

    $process = [System.Diagnostics.Process]::Start($startInfo)
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}')
    $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":2,"method":"tools/list"}')
    $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_catalog","arguments":{}}}')
    $contextRequest = [ordered]@{
      jsonrpc = "2.0"
      id = 4
      method = "tools/call"
      params = [ordered]@{
        name = "get_project_context"
        arguments = [ordered]@{ projectId = [int64]$fixture.project.id }
      }
    } | ConvertTo-Json -Depth 5 -Compress
    $process.StandardInput.WriteLine($contextRequest)
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(20000)) {
      Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
      throw "$hostId MCP sidecar timed out"
    }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    if ($process.ExitCode -ne 0) {
      throw "$hostId MCP sidecar failed with exit code $($process.ExitCode): $stderr"
    }
    $responses = @(
      $stdout -split "\r?\n" |
        Where-Object { $_ } |
        ForEach-Object { $_ | ConvertFrom-Json }
    )
    $call = @($responses | Where-Object id -eq 3)[0]
    $catalogText = [string]$call.result.content[0].text
    if ($call.result.isError -ne $false -or $catalogText -notmatch 'projects') {
      throw "$hostId list_catalog did not return the scoped project catalog"
    }
    $contextCall = @($responses | Where-Object id -eq 4)[0]
    $contextText = [string]$contextCall.result.content[0].text
    if ($contextCall.result.isError -ne $false -or $contextText -notmatch 'Fixture Git-like Project') {
      throw "$hostId get_project_context did not return the scoped fixture project"
    }
    $clientResults.Add([pscustomobject]@{
      host = $hostId
      sidecarExitCode = $process.ExitCode
      responseCount = $responses.Count
      listCatalogAllowed = $true
      projectContextAllowed = $true
    })
  }

  cargo run -q -p code-hangar-mcp --example acceptance_fixture -- audit $fixturePath
  if ($LASTEXITCODE -ne 0) { throw "MCP fixture audit failed with exit code $LASTEXITCODE" }
  foreach ($clientProperty in $fixture.clients.PSObject.Properties) {
    cargo run -q -p code-hangar-mcp --example acceptance_fixture -- audit-host $fixturePath $clientProperty.Name list_catalog project_context
    if ($LASTEXITCODE -ne 0) {
      throw "$($clientProperty.Name) MCP method audit failed with exit code $LASTEXITCODE"
    }
  }
} catch {
  $failure = $_
} finally {
  if (Test-Path -LiteralPath $fixturePath) {
    try {
      cargo run -q -p code-hangar-mcp --example acceptance_fixture -- disconnect $fixturePath
      if ($LASTEXITCODE -ne 0) {
        throw "MCP fixture disconnect failed with exit code $LASTEXITCODE"
      }
    } catch {
      $disconnectFailure = $_
    }
  }
}

$report = [pscustomobject]@{
  schemaVersion = 1
  status = if ($null -eq $failure -and $null -eq $disconnectFailure) { "PASS" } else { "FAIL" }
  serverSha256 = (Get-FileHash -LiteralPath $server -Algorithm SHA256).Hash.ToLowerInvariant()
  clients = @($clientResults)
  audit = if (Test-Path -LiteralPath (Join-Path $fixtureRoot "mcp-audit.json")) {
    [System.IO.Path]::GetRelativePath($repoRoot, (Join-Path $fixtureRoot "mcp-audit.json"))
  } else { $null }
  disconnect = if (Test-Path -LiteralPath (Join-Path $fixtureRoot "mcp-disconnect.json")) {
    [System.IO.Path]::GetRelativePath($repoRoot, (Join-Path $fixtureRoot "mcp-disconnect.json"))
  } else { $null }
  failure = if ($null -ne $failure) { $failure.Exception.Message } else { $null }
  disconnectFailure = if ($null -ne $disconnectFailure) { $disconnectFailure.Exception.Message } else { $null }
}
$reportPath = Join-Path $EvidenceDir "mcp-fixture-smoke.json"
$report | ConvertTo-Json -Depth 7 | Set-Content -LiteralPath $reportPath -Encoding utf8

if ($null -ne $disconnectFailure) {
  throw "MCP fixture cleanup failed: $($disconnectFailure.Exception.Message). Evidence: $reportPath"
}
if ($null -ne $failure) {
  throw "MCP fixture smoke failed: $($failure.Exception.Message). Evidence: $reportPath"
}

Write-Host "MCP fixture lifecycle passed. Evidence: $reportPath" -ForegroundColor Green
