[CmdletBinding()]
param(
  # Optional immutable private evidence directory. It must be a new direct child
  # of .local/acceptance/v0.1.3/release-stress. Canonical local-CI supplies it;
  # ordinary developer runs may omit it and still execute the exact lane.
  [string]$EvidenceDir,
  [string]$ExpectedGitCommit,
  [string]$ExpectedGitTree,
  [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot 'packaging-common.ps1')

$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$evidenceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local\acceptance\v0.1.3\release-stress')).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$reportName = 'RELEASE-STRESS-EVIDENCE.private.json'
$testDefinitions = @(
  [ordered]@{
    id = 'AUTO-03/adversarial-inventory-stress'
    label = 'Large adversarial inventory stays bounded and cancellable'
    log = 'adversarial-inventory.log'
    requiredOutputMarkers = @(
      'test large_adversarial_inventory_stays_bounded_and_cancellable',
      'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;'
    )
    arguments = @(
      'test', '--locked', '--offline', '-p', 'hangar-fs', '--test', 'adversarial_inventory', '--',
      '--ignored', '--exact', '--nocapture', '--test-threads=1'
    )
  },
  [ordered]@{
    id = 'AUTO-03/progressive-session-stress'
    label = 'Huge generated session progressively loads and opens fully'
    log = 'progressive-session.log'
    requiredOutputMarkers = @(
      'test tests::huge_generated_session_progressively_loads_and_opens_fully',
      'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;'
    )
    arguments = @(
      'test', '--locked', '--offline', '-p', 'hangar-api', '--no-default-features', '--features', 'core',
      'tests::huge_generated_session_progressively_loads_and_opens_fully', '--', '--ignored', '--exact',
      '--nocapture', '--test-threads=1'
    )
  }
)

function Assert-CanonicalObjectId {
  param([string]$Value, [string]$Label)
  if ($Value -cnotmatch '^[0-9a-f]{40,64}$') {
    throw "$Label must be a canonical lowercase Git object ID."
  }
}

function Get-CleanSourceIdentity {
  $commit = ([string](& git -C $repoRoot rev-parse HEAD)).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to read the release-stress source commit.' }
  $tree = ([string](& git -C $repoRoot rev-parse 'HEAD^{tree}')).Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to read the release-stress source tree.' }
  Assert-CanonicalObjectId -Value $commit -Label 'Release-stress source commit'
  Assert-CanonicalObjectId -Value $tree -Label 'Release-stress source tree'
  $status = @(& git -C $repoRoot status --porcelain --untracked-files=all)
  if ($LASTEXITCODE -ne 0) { throw 'Unable to prove the release-stress source status.' }
  if ($status.Count -ne 0) {
    throw 'Release-stress evidence requires the exact clean release source tree.'
  }
  return [pscustomobject]@{ Commit = $commit; Tree = $tree }
}

function Assert-ExpectedSourceIdentity {
  param([object]$Identity)
  if ([string]::IsNullOrWhiteSpace($ExpectedGitCommit) -xor [string]::IsNullOrWhiteSpace($ExpectedGitTree)) {
    throw '-ExpectedGitCommit and -ExpectedGitTree must be supplied together.'
  }
  if (-not [string]::IsNullOrWhiteSpace($ExpectedGitCommit)) {
    Assert-CanonicalObjectId -Value $ExpectedGitCommit -Label 'Expected release-stress commit'
    Assert-CanonicalObjectId -Value $ExpectedGitTree -Label 'Expected release-stress tree'
    if ($Identity.Commit -cne $ExpectedGitCommit -or $Identity.Tree -cne $ExpectedGitTree) {
      throw 'Release-stress source does not match the exact local-CI commit/tree.'
    }
  }
}

function Resolve-NewEvidenceDirectory {
  param([string]$RequestedPath)
  if ([string]::IsNullOrWhiteSpace($RequestedPath)) { return $null }
  if (-not [System.IO.Path]::IsPathFullyQualified($RequestedPath)) {
    $RequestedPath = Join-Path $repoRoot $RequestedPath
  }
  $full = [System.IO.Path]::GetFullPath($RequestedPath).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $prefix = $evidenceRoot + [System.IO.Path]::DirectorySeparatorChar
  $parent = [System.IO.Path]::GetDirectoryName($full)
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      [string]::IsNullOrWhiteSpace($parent) -or
      -not $parent.Equals($evidenceRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
      (Test-Path -LiteralPath $full)) {
    throw "EvidenceDir must be a new direct child below $evidenceRoot"
  }

  $localRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local'))
  Assert-FixedLocalPathChain -Path $localRoot -Label 'The worktree private-local root'
  [void][System.IO.Directory]::CreateDirectory($localRoot)
  Assert-FixedLocalPathChain -Path $localRoot -Label 'The worktree private-local root' -RequireExisting
  Assert-FixedLocalPathChain -Path $evidenceRoot -Label 'The release-stress evidence root'
  [void][System.IO.Directory]::CreateDirectory($evidenceRoot)
  Assert-FixedLocalPathChain -Path $evidenceRoot -Label 'The release-stress evidence root' -RequireExisting
  [void][System.IO.Directory]::CreateDirectory($full)
  Assert-FixedLocalPathChain -Path $full -Label 'The release-stress evidence attempt' -RequireExisting
  return $full
}

function Write-NewUtf8File {
  param([string]$Path, [string]$Text)
  $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($Text)
  $stream = [System.IO.FileStream]::new($Path, 'CreateNew', 'Write', 'None')
  try {
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
}

function Add-Utf8TextFile {
  param([string]$Path, [string]$Text)
  $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($Text)
  $stream = [System.IO.FileStream]::new($Path, 'Open', 'Write', 'None')
  try {
    $stream.Position = $stream.Length
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
}

function Get-FileEvidence {
  param([string]$Path, [string]$RelativePath)
  $item = Get-Item -LiteralPath $Path -Force
  if ($item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
    throw "Release-stress log is not a regular non-reparse file: $Path"
  }
  return [ordered]@{
    path = $RelativePath
    bytes = [long]$item.Length
    sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
  }
}

function Assert-TestDefinitions {
  $ids = @($testDefinitions | ForEach-Object { [string]$_.id })
  if ($ids.Count -ne 2 -or @($ids | Sort-Object -Unique).Count -ne 2 -or
      $ids[0] -cne 'AUTO-03/adversarial-inventory-stress' -or
      $ids[1] -cne 'AUTO-03/progressive-session-stress') {
    throw 'Release-stress Test IDs changed or are not unique.'
  }
  foreach ($test in $testDefinitions) {
    $args = @($test.arguments | ForEach-Object { [string]$_ })
    if ($args -cnotcontains '--locked' -or $args -cnotcontains '--offline' -or
        $args -cnotcontains '--ignored' -or $args -cnotcontains '--exact' -or
        $args -cnotcontains '--test-threads=1') {
      throw "Release-stress command $($test.id) lost a required offline/sequential guard."
    }
    if (@($test.requiredOutputMarkers).Count -ne 2 -or
        @($test.requiredOutputMarkers | Where-Object { [string]::IsNullOrWhiteSpace([string]$_) }).Count -ne 0) {
      throw "Release-stress command $($test.id) does not have its two exact output markers."
    }
  }
}

Assert-TestDefinitions

if ($SelfTest) {
  if (-not [string]::IsNullOrWhiteSpace($EvidenceDir) -or
      -not [string]::IsNullOrWhiteSpace($ExpectedGitCommit) -or
      -not [string]::IsNullOrWhiteSpace($ExpectedGitTree)) {
    throw '-SelfTest does not accept evidence or source-identity arguments.'
  }
  $malformedRejected = $false
  try { Assert-CanonicalObjectId -Value ('A' * 40) -Label 'Synthetic object' } catch { $malformedRejected = $true }
  if (-not $malformedRejected) { throw 'Release-stress self-test accepted a non-canonical object ID.' }
  $scopeRejected = $false
  try {
    Resolve-NewEvidenceDirectory -RequestedPath (Join-Path $repoRoot '.local\release-stress-outside-scope') | Out-Null
  } catch { $scopeRejected = $true }
  if (-not $scopeRejected) { throw 'Release-stress self-test accepted an out-of-scope evidence directory.' }
  $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $logSelfTestPath = Join-Path $tempRoot ("codehangar-release-stress-log-$([guid]::NewGuid().ToString('N')).txt")
  try {
    Write-NewUtf8File -Path $logSelfTestPath -Text "prefix`n"
    Add-Utf8TextFile -Path $logSelfTestPath -Text "suffix`n"
    if ([System.IO.File]::ReadAllText($logSelfTestPath) -cne "prefix`nsuffix`n") {
      throw 'Release-stress UTF-8 append self-test produced different bytes.'
    }
  } finally {
    if (Test-Path -LiteralPath $logSelfTestPath -PathType Leaf) {
      Remove-Item -LiteralPath $logSelfTestPath -Force -ErrorAction Stop
    }
  }
  Write-Host 'v0.1.3 release-stress evidence self-test passed.' -ForegroundColor Green
  exit 0
}

if ([string]::IsNullOrWhiteSpace($EvidenceDir) -and
    (-not [string]::IsNullOrWhiteSpace($ExpectedGitCommit) -or -not [string]::IsNullOrWhiteSpace($ExpectedGitTree))) {
  throw 'Expected source identity is valid only with -EvidenceDir.'
}

Set-Location $repoRoot
Assert-PackagingEnvironmentOverrides
Assert-FixedLocalPathChain -Path $repoRoot -Label 'The release-stress worktree' -RequireExisting

$env:CARGO_NET_OFFLINE = 'true'
$env:CARGO_BUILD_JOBS = '1'
$env:RUST_TEST_THREADS = '1'
$env:RUSTUP_AUTO_INSTALL = '0'
$env:CARGO_TERM_COLOR = 'never'

$localRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local'))
[void][System.IO.Directory]::CreateDirectory($localRoot)
Assert-FixedLocalPathChain -Path $localRoot -Label 'The release-stress lock root' -RequireExisting
$lockPath = Join-Path $localRoot 'release-stress-v013.lock'
$lock = $null
$formalEvidence = $null
$sourceIdentity = $null
$startedAt = [datetime]::UtcNow
$results = [System.Collections.Generic.List[object]]::new()
$failure = $null

try {
  try {
    $lock = [System.IO.FileStream]::new($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
  } catch [System.IO.IOException] {
    throw 'Another v0.1.3 release-stress lane already holds the worktree lock.'
  }

  if (-not [string]::IsNullOrWhiteSpace($EvidenceDir)) {
    $sourceIdentity = Get-CleanSourceIdentity
    Assert-ExpectedSourceIdentity -Identity $sourceIdentity
    $formalEvidence = Resolve-NewEvidenceDirectory -RequestedPath $EvidenceDir
  }

  foreach ($test in $testDefinitions) {
    $stepStarted = [datetime]::UtcNow
    $displayCommand = 'cargo ' + (@($test.arguments) -join ' ')
    $logPath = if ($null -ne $formalEvidence) { Join-Path $formalEvidence ([string]$test.log) } else { $null }
    Write-Host ''
    Write-Host "==> [$($test.id)] $($test.label)" -ForegroundColor Cyan
    if ($null -ne $logPath) {
      Write-NewUtf8File -Path $logPath -Text ("`$ $displayCommand`n")
    }

    $exitCode = 0
    $capturedOutput = $null
    & cargo @($test.arguments) 2>&1 | Tee-Object -Variable capturedOutput
    $exitCode = $LASTEXITCODE
    $capturedText = (@($capturedOutput) | ForEach-Object { [string]$_ }) -join "`n"
    if ($null -ne $logPath) {
      Add-Utf8TextFile -Path $logPath -Text ($capturedText + "`n")
    }
    if ($exitCode -eq 0) {
      foreach ($marker in @($test.requiredOutputMarkers)) {
        if (-not $capturedText.Contains([string]$marker, [System.StringComparison]::Ordinal)) {
          $exitCode = 86
          Write-Host "Release-stress test $($test.id) exited zero without required output marker: $marker" -ForegroundColor Red
          break
        }
      }
    }
    $elapsedMs = [long]([datetime]::UtcNow - $stepStarted).TotalMilliseconds
    $result = [ordered]@{
      id = [string]$test.id
      status = if ($exitCode -eq 0) { 'PASS' } else { 'FAIL' }
      exitCode = [int]$exitCode
      elapsedMs = $elapsedMs
      command = $displayCommand
      log = if ($null -ne $logPath) { Get-FileEvidence -Path $logPath -RelativePath ([string]$test.log) } else { $null }
    }
    $results.Add([pscustomobject]$result)
    if ($exitCode -ne 0) {
      throw "Release-stress test $($test.id) failed with exit code $exitCode."
    }
  }
} catch {
  $failure = $_
} finally {
  if ($null -ne $formalEvidence) {
    try {
      $after = Get-CleanSourceIdentity
      Assert-ExpectedSourceIdentity -Identity $after
      if ($after.Commit -cne $sourceIdentity.Commit -or $after.Tree -cne $sourceIdentity.Tree) {
        throw 'Release-stress source commit/tree changed during the lane.'
      }
    } catch {
      if ($null -eq $failure) { $failure = $_ }
    }

    $report = [ordered]@{
      schemaVersion = 1
      documentType = 'codehangar/release-stress-evidence/1'
      version = '0.1.3'
      status = if ($null -eq $failure -and $results.Count -eq $testDefinitions.Count) { 'PASS' } else { 'FAIL' }
      startedAtUtc = $startedAt.ToString('yyyy-MM-ddTHH:mm:ss.fffffffZ', [System.Globalization.CultureInfo]::InvariantCulture)
      completedAtUtc = [datetime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffffffZ', [System.Globalization.CultureInfo]::InvariantCulture)
      source = [ordered]@{
        gitCommit = [string]$sourceIdentity.Commit
        gitTree = [string]$sourceIdentity.Tree
        sourceTreeDirty = $false
      }
      isolation = [ordered]@{
        sequential = $true
        cargoOffline = $true
        cargoBuildJobs = 1
        rustTestThreads = 1
      }
      testIds = @($testDefinitions | ForEach-Object { [string]$_.id })
      results = @($results)
    }
    $reportPath = Join-Path $formalEvidence $reportName
    Write-NewUtf8File -Path $reportPath -Text (($report | ConvertTo-Json -Depth 8) + "`n")
    $reportHash = (Get-FileHash -LiteralPath $reportPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "Private release-stress evidence: $reportPath" -ForegroundColor Green
    Write-Host "RELEASE-STRESS-EVIDENCE SHA-256: $reportHash" -ForegroundColor Green
  }
  if ($null -ne $lock) { $lock.Dispose() }
}

if ($null -ne $failure) { throw $failure }
Write-Host 'v0.1.3 sequential release-stress lane passed.' -ForegroundColor Green
