$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$cargoBins = @(
  (Join-Path $env:USERPROFILE ".cargo\bin"),
  (Join-Path $env:USERPROFILE ".local\cargo\bin")
)

foreach ($cargoBin in $cargoBins) {
  if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
  }
}

cargo build -p code-hangar-mcp --release
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

$serverExe = Join-Path $repoRoot "target\release\code-hangar-mcp.exe"
if (-not (Test-Path $serverExe)) {
  throw "Connected-app server release artifact was not produced: $serverExe"
}
