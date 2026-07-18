[CmdletBinding()]
param(
  [string]$SharedRoot = "C:\CodeHangarAcceptance"
)

$ErrorActionPreference = "Stop"

# FAIL-CLOSED SANDBOX GUARD. This agent installs/uninstalls Code Hangar (running installers with
# /S) and reads, hashes, and reasons over the REAL %APPDATA%\local.codehangar.desktop catalog. It
# must ONLY ever run inside a disposable Windows Sandbox. Two independent sandbox signals are
# accepted: Windows Sandbox always auto-logs on as WDAGUtilityAccount, and the host driver
# (sandbox-lifecycle.ps1) sets CODEHANGAR_SANDBOX_AGENT=1 in the .wsb LogonCommand. If NEITHER is
# present we are not in the Sandbox — abort before any install/uninstall/DB action below.
if ($env:USERNAME -ne 'WDAGUtilityAccount' -and $env:CODEHANGAR_SANDBOX_AGENT -ne '1') {
  throw "Refusing to run outside Windows Sandbox: sandbox-lifecycle-agent.ps1 installs/uninstalls apps and reads the real Code Hangar catalog under %APPDATA%. It may run only inside the disposable acceptance Sandbox (detected user '$env:USERNAME')."
}

$SharedRoot = [System.IO.Path]::GetFullPath($SharedRoot)
$commandsDir = Join-Path $SharedRoot "commands"
$resultsDir = Join-Path $SharedRoot "results"
New-Item -ItemType Directory -Path $commandsDir, $resultsDir -Force | Out-Null

function Write-JsonAtomic {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][object]$Value
  )
  $temporary = "$Path.tmp-$([guid]::NewGuid().ToString('N'))"
  $json = $Value | ConvertTo-Json -Depth 10
  [System.IO.File]::WriteAllText($temporary, $json, [System.Text.UTF8Encoding]::new($false))
  [System.IO.File]::Move($temporary, $Path)
}

function Resolve-ExecutablePath {
  param([object]$RegistryEntry)

  $displayIcon = [string]$RegistryEntry.DisplayIcon
  if (-not [string]::IsNullOrWhiteSpace($displayIcon)) {
    $candidate = ($displayIcon -replace ',\s*\d+$', '').Trim().Trim('"')
    if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
  }
  $installLocation = [string]$RegistryEntry.InstallLocation
  if (-not [string]::IsNullOrWhiteSpace($installLocation)) {
    $candidate = Join-Path $installLocation "code-hangar-desktop.exe"
    if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
  }
  return $null
}

function Resolve-UninstallerPath {
  param([object]$RegistryEntry)

  $command = [string]$RegistryEntry.UninstallString
  if ($command -match '^"([^"]+)"') { return $Matches[1] }
  if ($command -match '^([^\s]+\.exe)') { return $Matches[1] }
  return $null
}

function Get-CatalogState {
  $dataDir = Join-Path $env:APPDATA "local.codehangar.desktop"
  $keyPath = Join-Path $dataDir "codehangar.sqlite3.key.dpapi"
  $dbPaths = @(
    (Join-Path $dataDir "codehangar.sqlite3"),
    (Join-Path $dataDir "codehangar.sqlite3-wal"),
    (Join-Path $dataDir "codehangar.sqlite3-shm")
  )
  $dbBytes = [long]0
  foreach ($path in $dbPaths) {
    if (Test-Path -LiteralPath $path -PathType Leaf) {
      $dbBytes += (Get-Item -LiteralPath $path).Length
    }
  }
  [pscustomobject]@{
    dataDir = $dataDir
    exists = Test-Path -LiteralPath $dataDir -PathType Container
    databaseBytes = $dbBytes
    keyExists = Test-Path -LiteralPath $keyPath -PathType Leaf
    keySha256 = if (Test-Path -LiteralPath $keyPath -PathType Leaf) {
      (Get-FileHash -LiteralPath $keyPath -Algorithm SHA256).Hash.ToLowerInvariant()
    } else { $null }
    files = @(
      Get-ChildItem -LiteralPath $dataDir -File -ErrorAction SilentlyContinue |
        Sort-Object Name |
        ForEach-Object { [pscustomobject]@{ name = $_.Name; bytes = $_.Length } }
    )
  }
}

function Get-InstalledState {
  $entries = [System.Collections.Generic.List[object]]::new()
  foreach ($registryPath in @(
    "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
    "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
    "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*"
  )) {
    foreach ($entry in @(Get-ItemProperty -Path $registryPath -ErrorAction SilentlyContinue)) {
      if ([string]$entry.DisplayName -notlike "Code Hangar*") { continue }
      $executable = Resolve-ExecutablePath $entry
      $installLocation = if ($executable) {
        Split-Path -Parent $executable
      } else {
        [string]$entry.InstallLocation
      }
      $entries.Add([pscustomobject]@{
        displayName = [string]$entry.DisplayName
        displayVersion = [string]$entry.DisplayVersion
        installLocation = $installLocation
        executable = $executable
        executableExists = $null -ne $executable
        sidecarExists = if ($installLocation) {
          Test-Path -LiteralPath (Join-Path $installLocation "code-hangar-mcp.exe") -PathType Leaf
        } else { $false }
        uninstaller = Resolve-UninstallerPath $entry
        registryPath = [string]$entry.PSPath
      })
    }
  }
  [pscustomobject]@{
    applications = @($entries | Sort-Object displayName, installLocation -Unique)
    catalog = Get-CatalogState
    runningPids = @(
      Get-Process -Name "code-hangar-desktop" -ErrorAction SilentlyContinue |
        Sort-Object Id |
        ForEach-Object { $_.Id }
    )
  }
}

function Get-AppEntry {
  param(
    [Parameter(Mandatory = $true)][object]$State,
    [Parameter(Mandatory = $true)][string]$DisplayName
  )
  $matches = @($State.applications | Where-Object displayName -eq $DisplayName)
  if ($matches.Count -ne 1) {
    throw "Expected one installed '$DisplayName' entry, found $($matches.Count)."
  }
  return $matches[0]
}

function Assert-ExpectedApp {
  param(
    [Parameter(Mandatory = $true)][object]$Command,
    [Parameter(Mandatory = $true)][object]$State
  )
  $app = Get-AppEntry -State $State -DisplayName ([string]$Command.displayName)
  if ($Command.expectedVersion -and $app.displayVersion -ne [string]$Command.expectedVersion) {
    throw "Expected version $($Command.expectedVersion), found $($app.displayVersion)."
  }
  if ($null -ne $Command.expectedSidecar -and $app.sidecarExists -ne [bool]$Command.expectedSidecar) {
    throw "Sidecar expectation failed for $($Command.displayName): expected $($Command.expectedSidecar), found $($app.sidecarExists)."
  }
  return $app
}

function Resolve-SharedInstaller {
  param([string]$RelativePath)
  $path = [System.IO.Path]::GetFullPath((Join-Path $SharedRoot $RelativePath))
  $allowedPrefix = $SharedRoot.TrimEnd("\") + "\"
  if (-not $path.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Installer must stay under $SharedRoot"
  }
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Installer not found: $path"
  }
  return $path
}

function Invoke-Install {
  param([object]$Command)
  $installer = Resolve-SharedInstaller ([string]$Command.installer)
  # Windows Sandbox mapped folders are redirected shares. NSIS can return exit 2
  # before setup when executed directly from that boundary, so copy the exact,
  # already-hashed artifact into the disposable guest and run it locally.
  $installerSha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
  $localInstaller = Join-Path $env:TEMP ("codehangar-acceptance-$installerSha256.exe")
  if (-not (Test-Path -LiteralPath $localInstaller -PathType Leaf)) {
    Copy-Item -LiteralPath $installer -Destination $localInstaller
  }
  if ((Get-FileHash -LiteralPath $localInstaller -Algorithm SHA256).Hash.ToLowerInvariant() -ne $installerSha256) {
    throw "Guest-local installer copy failed hash verification."
  }
  $before = Get-InstalledState
  $started = Get-Date
  $process = Start-Process -FilePath $localInstaller -ArgumentList "/S" -PassThru -Wait
  $elapsedMs = [int]((Get-Date) - $started).TotalMilliseconds
  if ($process.ExitCode -ne 0) {
    throw "Installer exited with code $($process.ExitCode)."
  }
  $after = Get-InstalledState
  $app = Assert-ExpectedApp -Command $Command -State $after
  [pscustomobject]@{
    action = "install"
    installer = [System.IO.Path]::GetFileName($installer)
    installerSha256 = $installerSha256
    exitCode = $process.ExitCode
    elapsedMs = $elapsedMs
    before = $before
    after = $after
    selectedApp = $app
  }
}

function Invoke-Launch {
  param([object]$Command)
  $before = Get-InstalledState
  $app = Assert-ExpectedApp -Command $Command -State $before
  if ($before.runningPids.Count -ne 0) {
    throw "Close the running Code Hangar process before launch."
  }
  $process = Start-Process -FilePath $app.executable -PassThru
  $deadline = (Get-Date).AddSeconds(30)
  do {
    Start-Sleep -Milliseconds 100
    $process.Refresh()
  } while (-not $process.HasExited -and $process.MainWindowHandle -eq 0 -and (Get-Date) -lt $deadline)
  if ($process.HasExited) { throw "Application exited during launch with code $($process.ExitCode)." }
  if ($process.MainWindowHandle -eq 0) { throw "Application did not show a window within 30 seconds." }
  [pscustomobject]@{
    action = "launch"
    pid = $process.Id
    windowHandle = $process.MainWindowHandle.ToInt64()
    selectedApp = $app
    state = Get-InstalledState
  }
}

function Invoke-Close {
  $processes = @(Get-Process -Name "code-hangar-desktop" -ErrorAction SilentlyContinue)
  foreach ($process in $processes) {
    [void]$process.CloseMainWindow()
  }
  $deadline = (Get-Date).AddSeconds(15)
  do {
    Start-Sleep -Milliseconds 100
    $remaining = @(Get-Process -Name "code-hangar-desktop" -ErrorAction SilentlyContinue)
  } while ($remaining.Count -gt 0 -and (Get-Date) -lt $deadline)
  if ($remaining.Count -gt 0) {
    throw "Code Hangar did not close gracefully; remaining PID(s): $($remaining.Id -join ', ')."
  }
  [pscustomobject]@{ action = "close"; closedPids = @($processes.Id); state = Get-InstalledState }
}

function Invoke-Uninstall {
  param([object]$Command)
  $before = Get-InstalledState
  if ($before.runningPids.Count -ne 0) { throw "Close Code Hangar before uninstalling." }
  $app = Get-AppEntry -State $before -DisplayName ([string]$Command.displayName)
  if (-not $app.uninstaller -or -not (Test-Path -LiteralPath $app.uninstaller -PathType Leaf)) {
    throw "Uninstaller not found for $($Command.displayName)."
  }
  $keyBefore = $before.catalog.keySha256
  $started = Get-Date
  $process = Start-Process -FilePath $app.uninstaller -ArgumentList "/S" -PassThru -Wait
  $elapsedMs = [int]((Get-Date) - $started).TotalMilliseconds
  if ($process.ExitCode -ne 0) { throw "Uninstaller exited with code $($process.ExitCode)." }
  $after = Get-InstalledState
  if (@($after.applications | Where-Object displayName -eq [string]$Command.displayName).Count -ne 0) {
    throw "$($Command.displayName) is still registered after uninstall."
  }
  if ($Command.expectCatalogPreserved -and (
      -not $after.catalog.keyExists -or $after.catalog.keySha256 -ne $keyBefore
    )) {
    throw "The encrypted catalog key was not preserved by uninstall."
  }
  [pscustomobject]@{
    action = "uninstall"
    displayName = [string]$Command.displayName
    exitCode = $process.ExitCode
    elapsedMs = $elapsedMs
    before = $before
    after = $after
  }
}

function Invoke-Catalog {
  param([object]$Command)
  $helper = Resolve-SharedInstaller ([string]$Command.helper)
  $project = [System.IO.Path]::GetFullPath((Join-Path $SharedRoot ([string]$Command.project)))
  $allowedPrefix = $SharedRoot.TrimEnd("\") + "\"
  if (-not $project.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Catalog fixture project must stay under $SharedRoot"
  }
  if (-not (Test-Path -LiteralPath $project -PathType Container)) {
    throw "Catalog fixture project not found: $project"
  }
  if (@(Get-Process -Name "code-hangar-desktop" -ErrorAction SilentlyContinue).Count -ne 0) {
    throw "Close Code Hangar before opening its catalog with the acceptance helper."
  }
  $dbPath = Join-Path $env:APPDATA "local.codehangar.desktop\codehangar.sqlite3"
  if (-not (Test-Path -LiteralPath $dbPath -PathType Leaf)) {
    throw "Catalog database not found: $dbPath"
  }
  $reportPath = Join-Path $resultsDir ("$($Command.id)-catalog.json")
  $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $helper
  $startInfo.WorkingDirectory = $SharedRoot
  $startInfo.UseShellExecute = $false
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true
  $quotedArguments = @([string]$Command.mode, $dbPath, $project, $reportPath) |
    ForEach-Object { '"' + ([string]$_).Replace('"', '\"') + '"' }
  $startInfo.Arguments = $quotedArguments -join ' '
  $process = [System.Diagnostics.Process]::Start($startInfo)
  $stdoutTask = $process.StandardOutput.ReadToEndAsync()
  $stderrTask = $process.StandardError.ReadToEndAsync()
  if (-not $process.WaitForExit(150000)) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    throw "Catalog acceptance helper timed out."
  }
  $stdout = $stdoutTask.GetAwaiter().GetResult()
  $stderr = $stderrTask.GetAwaiter().GetResult()
  if ($process.ExitCode -ne 0) {
    throw "Catalog acceptance helper failed with exit code $($process.ExitCode): $stderr"
  }
  $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
  if ($report.status -ne "PASS") { throw "Catalog acceptance report did not pass." }
  [pscustomobject]@{
    action = "catalog"
    mode = [string]$Command.mode
    helperSha256 = (Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash.ToLowerInvariant()
    report = [System.IO.Path]::GetFileName($reportPath)
    stdout = $stdout.Trim()
    catalog = $report
    state = Get-InstalledState
  }
}

function Invoke-CommandFile {
  param([object]$Command)
  switch ([string]$Command.action) {
    "install" { return Invoke-Install $Command }
    "launch" { return Invoke-Launch $Command }
    "close" { return Invoke-Close }
    "inspect" { return [pscustomobject]@{ action = "inspect"; state = Get-InstalledState } }
    "uninstall" { return Invoke-Uninstall $Command }
    "catalog" { return Invoke-Catalog $Command }
    default { throw "Unknown lifecycle action: $($Command.action)" }
  }
}

Write-JsonAtomic -Path (Join-Path $resultsDir "agent-ready.json") -Value ([pscustomobject]@{
  status = "PASS"
  startedAt = (Get-Date).ToString("o")
  machine = $env:COMPUTERNAME
  user = $env:USERNAME
  sharedRoot = $SharedRoot
  state = Get-InstalledState
})

while (-not (Test-Path -LiteralPath (Join-Path $SharedRoot "stop.flag"))) {
  foreach ($commandFile in @(Get-ChildItem -LiteralPath $commandsDir -Filter "*.json" -File | Sort-Object Name)) {
    $resultPath = Join-Path $resultsDir $commandFile.Name
    if (Test-Path -LiteralPath $resultPath) { continue }
    $startedAt = Get-Date
    try {
      $command = Get-Content -LiteralPath $commandFile.FullName -Raw | ConvertFrom-Json
      $detail = Invoke-CommandFile $command
      $result = [pscustomobject]@{
        id = [string]$command.id
        status = "PASS"
        startedAt = $startedAt.ToString("o")
        completedAt = (Get-Date).ToString("o")
        detail = $detail
        error = $null
      }
    } catch {
      $result = [pscustomobject]@{
        id = $commandFile.BaseName
        status = "FAIL"
        startedAt = $startedAt.ToString("o")
        completedAt = (Get-Date).ToString("o")
        detail = $null
        error = $_.Exception.Message
      }
    }
    Write-JsonAtomic -Path $resultPath -Value $result
  }
  Start-Sleep -Milliseconds 250
}

Write-JsonAtomic -Path (Join-Path $resultsDir "agent-stopped.json") -Value ([pscustomobject]@{
  status = "PASS"
  stoppedAt = (Get-Date).ToString("o")
  state = Get-InstalledState
})
