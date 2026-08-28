param(
  [string]$WebView2InstallerPath,
  [switch]$SkipTauriBuild,
  [switch]$CoreOnly,
  [switch]$AgentAutomation,
  # Optional immutable, private evidence output for the release acceptance
  # validator. The directory must be new and live below
  # .local/acceptance/v0.1.3/local-ci; it is never a public artifact.
  [string]$EvidenceDir,
  # Perf gate, STAGE 1 (non-blocking): time each step and, on a green run, write a machine-local
  # baseline to .local/perf-baseline.json (gitignored - never committed). Default off, so normal
  # CI behavior is unchanged.
  [switch]$Measure,
  # Perf gate, STAGE 2 (blocking): time each step and compare against the machine-local baseline,
  # failing the run on a GROSS regression only (generous tolerance, so normal build-cache / machine-
  # load variance never trips it - it catches a catastrophic slowdown, not noise). Bootstraps a
  # baseline on the first run when none exists. Refresh the baseline deliberately with -Measure.
  [switch]$PerfGate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$script:StepTimings = @()
$script:CompletedSteps = [System.Collections.Generic.List[object]]::new()
$script:CanonicalSourceIdentity = $null
$script:ReleaseStressEvidenceDirectory = $null

. (Join-Path $PSScriptRoot "packaging-common.ps1")

$repoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$supportedWindowsTarget = "x86_64-pc-windows-msvc"
Set-Location $repoRoot

if ($CoreOnly -and $AgentAutomation) {
  throw "-CoreOnly and -AgentAutomation are mutually exclusive."
}
if (-not [string]::IsNullOrWhiteSpace($EvidenceDir) -and
    ($CoreOnly -or -not $AgentAutomation -or -not $SkipTauriBuild -or $Measure -or $PerfGate)) {
  throw "-EvidenceDir requires the canonical non-packaging release lane: -AgentAutomation -SkipTauriBuild, without CoreOnly/Measure/PerfGate."
}
if (-not $SkipTauriBuild -and -not $CoreOnly -and [string]::IsNullOrWhiteSpace($WebView2InstallerPath)) {
  throw "-WebView2InstallerPath is required whenever local CI will package a Tauri installer."
}

Assert-PackagingEnvironmentOverrides

function Initialize-OfflineCiEnvironment {
  Assert-FixedLocalPathChain -Path $repoRoot -Label "The CI worktree" -RequireExisting
  $repoTarget = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "target")).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  if (-not [string]::IsNullOrWhiteSpace($env:CARGO_BUILD_TARGET)) {
    throw "CARGO_BUILD_TARGET changes Cargo's output layout and is refused by local CI. Clear it and retry."
  }
  if (-not [string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    $requested = $env:CARGO_TARGET_DIR
    if (-not [System.IO.Path]::IsPathRooted($requested)) {
      $requested = Join-Path $repoRoot $requested
    }
    $requested = [System.IO.Path]::GetFullPath($requested).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    if (-not $requested.Equals($repoTarget, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "CARGO_TARGET_DIR must be this worktree's target directory ($repoTarget), not $requested."
    }
  }
  Assert-FixedLocalPathChain -Path $repoTarget -Label "The worktree target path"
  $env:CARGO_TARGET_DIR = $repoTarget
  $env:CARGO_NET_OFFLINE = "true"
  $env:CARGO_BUILD_JOBS = "2"
  $env:RUST_TEST_THREADS = "2"
  $env:npm_config_offline = "true"
}

Initialize-OfflineCiEnvironment

$cargoBins = @(
  (Join-Path $env:USERPROFILE ".cargo\bin"),
  (Join-Path $env:USERPROFILE ".local\cargo\bin")
)

foreach ($cargoBin in $cargoBins) {
  if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
  }
}

function Run-Step {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Label,
    [Parameter(Mandatory = $true)]
    [scriptblock]$Command
  )

  Write-Host ""
  Write-Host "==> $Label" -ForegroundColor Cyan
  $stepStart = Get-Date
  & $Command
  $exit = $LASTEXITCODE
  if ($Measure -or $PerfGate) {
    $elapsedMs = [int]((Get-Date) - $stepStart).TotalMilliseconds
    $script:StepTimings += [pscustomobject]@{ name = $Label; elapsedMs = $elapsedMs }
    Write-Host ("    {0} ms" -f $elapsedMs) -ForegroundColor DarkGray
  }
  if ($exit -ne 0) {
    throw "$Label failed with exit code $exit"
  }
  $stepId = (($Label.ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-'))
  if (@($script:CompletedSteps | Where-Object id -eq $stepId).Count -ne 0) {
    throw "Local CI generated a duplicate canonical step id: $stepId"
  }
  $script:CompletedSteps.Add([pscustomobject]@{ id = $stepId; label = $Label })
}

function Assert-LocalReleaseBinaryIsolation {
  param([Parameter(Mandatory = $true)][string]$Path)

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "The compile-only Local release binary is missing: $Path"
  }
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  if ($bytes.Length -lt 1024 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
    throw "The compile-only Local release output is not a valid bounded PE candidate: $Path"
  }
  $utf8 = [System.Text.Encoding]::UTF8.GetString($bytes)
  $utf16 = [System.Text.Encoding]::Unicode.GetString($bytes)
  foreach ($forbiddenMarker in @(
      'Connector',
      'AI Assist',
      'ai_provider_',
      'ai_explain_',
      'mcp_appconfig_',
      'agent_request_',
      'automation_register',
      'agent_automation',
      'hangar-agent',
      'hangar-ai',
      'code-hangar-mcp',
      'api.openai.com',
      'api.anthropic.com',
      'openrouter.ai'
    )) {
    if ($utf8.IndexOf($forbiddenMarker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $utf16.IndexOf($forbiddenMarker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
      throw "The compile-only Local release PE contains a Connector-only capability/name/path marker: $forbiddenMarker"
    }
  }
  Write-Host "Local release PE isolation passed: no Connector name/capability/IPC/provider endpoint markers in exact release bytes." -ForegroundColor Green
}

function Get-CanonicalLocalCiTestClaims {
  # Each semantic acceptance id below is issued only after the full relevant
  # suite passed and its named regression marker still exists in the exact
  # source tree. These are coverage identifiers, not a reproducible-build or
  # formal-verification claim.
  $claims = @(
    @{ id = 'SRC-02/workspace-version-contract'; path = 'scripts/release-pipeline-self-test.ps1'; markers = @('0.1.3') },
    @{ id = 'SRC-03/sequential-edition-build-contract'; path = 'scripts/local-ci.ps1'; markers = @('compile-only Windows Local release (nonpublishable)', 'compile-only Windows Connector desktop release (nonpublishable)') },
    @{ id = 'SRC-04/local-edition-isolation'; checks = @(
        @{ path = 'scripts/check-frontend-edition.mjs'; markers = @('Local', 'assertLocalNativeInstallerIsolation', 'Connector installation path in Local native hook') },
        @{ path = 'scripts/local-ci.ps1'; markers = @('Assert-LocalReleaseBinaryIsolation', 'Local release PE isolation passed') }
      ) },
    @{ id = 'SRC-05/connector-edition-isolation'; path = 'scripts/check-frontend-edition.mjs'; markers = @('Connector') },
    @{ id = 'AUTO-01/format-clippy'; path = 'scripts/local-ci.ps1'; markers = @('cargo fmt', 'cargo clippy agent automation') },
    @{ id = 'AUTO-02/frontend-test-typescript-eslint'; path = 'package.json'; markers = @('npm run lint && npm run test') },
    @{ id = 'P1-UI-E2E-01'; checks = @(
        @{ path = 'scripts/local-ci.ps1'; markers = @('P1-UI-E2E-01 native edit and restore journey') },
        @{ path = 'scripts/p1-ui-e2e.ps1'; markers = @('--bin p1_ui_e2e', '--features p1_ui_e2e') },
        @{ path = 'apps/desktop/src-tauri/src/bin/p1_ui_e2e.rs'; markers = @('real UI + Tauri IPC restored exact bytes', 'verify_completed_journey', 'verify_ledger_entry') }
      ) },
    @{ id = 'AUTO-02/safe-manage-first-run-choice-orchestration'; checks = @(
        @{ path = 'apps/desktop/src/safeManageFirstRun.ts'; markers = @('applySafeManageFirstRunChoice', 'const analysisJobId = await backend.startAnalysis()') },
        @{ path = 'apps/desktop/src/__tests__/safe-manage-portfolio.test.ts'; markers = @('first-run choices persist distinct behavior and only Analyze now starts work') }
      ) },
    @{ id = 'AUTO-02/safe-manage-bounded-comparison-ui'; checks = @(
        @{ path = 'apps/desktop/src/views/SafeManagePortfolioView.tsx'; markers = @('Bounded portfolio comparison', 'Low-confidence, metadata-only hints') },
        @{ path = 'apps/desktop/src/__tests__/safe-manage-portfolio.test.ts'; markers = @('renders bounded comparison evidence without exposing paths or treating unavailable as zero') }
      ) },
    @{ id = 'AUTO-03/core-mutation-rust'; path = 'scripts/local-ci.ps1'; markers = @('cargo test core', 'cargo test mutation') },
    @{ id = 'AUTO-03/adversarial-inventory-stress'; checks = @(
        @{ path = 'scripts/release-stress-v013.ps1'; markers = @('AUTO-03/adversarial-inventory-stress', "'--test', 'adversarial_inventory'") },
        @{ path = 'crates/hangar-fs/tests/adversarial_inventory.rs'; markers = @('fn large_adversarial_inventory_stays_bounded_and_cancellable') }
      ) },
    @{ id = 'AUTO-03/progressive-session-stress'; checks = @(
        @{ path = 'scripts/release-stress-v013.ps1'; markers = @('AUTO-03/progressive-session-stress', 'tests::huge_generated_session_progressively_loads_and_opens_fully') },
        @{ path = 'crates/hangar-api/src/lib.rs'; markers = @('fn huge_generated_session_progressively_loads_and_opens_fully') }
      ) },
    @{ id = 'AUTO-03/safe-manage-large-portfolio'; path = 'crates/hangar-api/src/safe_manage.rs'; markers = @(
        'fn large_portfolio_analysis_persists_every_synthetic_project_without_mutation',
        'const LARGE_PORTFOLIO_PROJECTS: usize = 512'
      ) },
    @{ id = 'AUTO-03/safe-manage-cancellation-preserves-complete'; path = 'crates/hangar-api/src/safe_manage.rs'; markers = @(
        'fn mid_run_cancellation_preserves_last_complete_and_never_promotes_partial',
        'the previously complete portfolio must remain byte-for-byte unchanged'
      ) },
    @{ id = 'AUTO-03/safe-manage-first-run-persistence'; path = 'crates/hangar-api/src/safe_manage.rs'; markers = @(
        'fn first_run_analyze_later_and_suppress_have_distinct_persisted_outcomes',
        'first-run-analyze-now'
      ) },
    @{ id = 'AUTO-03/safe-manage-bounded-comparison-evidence'; checks = @(
        @{ path = 'crates/hangar-core/src/safe_manage.rs'; markers = @(
            'fn partial_comparison_blocks_archive_without_turning_unknown_into_zero',
            'fn partial_comparison_does_not_hide_exact_regenerable_cleanup_review'
          ) },
        @{ path = 'crates/hangar-db/src/safe_manage.rs'; markers = @(
            'fn bounded_profiles_report_positive_kinds_and_conservative_copy_evidence',
            'fn exact_material_groups_keep_exact_counts_but_cap_related_ids'
          ) },
        @{ path = 'crates/hangar-api/src/safe_manage.rs'; markers = @(
            'comparison_evidence_revision: &''a str',
            'comparison_changed.comparison_evidence_revision = "comparison-2"'
          ) }
      ) },
    @{ id = 'AUTO-04/agent-automation-connected-app'; checks = @(
        @{ path = 'scripts/local-ci.ps1'; markers = @('cargo test agent automation', 'cargo test connected-app surface') },
        @{ path = 'crates/hangar-api/src/lib.rs'; markers = @(
            'fn held_project_is_visible_to_primary_final_remove_batch',
            'require_final_remove_enabled(state)?',
            'FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT'
          ) }
      ) },
    @{ id = 'AUTO-05/zero-outbound-guards'; path = 'package.json'; markers = @('check-no-outbound-deps.mjs', 'check-no-forbidden-code.mjs') },
    @{ id = 'AUTO-06/secret-scan'; path = 'package.json'; markers = @('node scripts/secret-scan.mjs') },
    @{ id = 'AUTO-07/packaging-contract-selftests'; path = 'scripts/local-ci.ps1'; markers = @('Local packaging deterministic self-tests', 'Connector packaging deterministic self-tests') },
    @{ id = 'AUTO-08/release-script-selftests'; path = 'scripts/local-ci.ps1'; markers = @('release artifact proof self-test', 'v0.1.3 acceptance evidence self-test', 'v0.1.3 release stress evidence self-test', 'checksum staging self-test') },
    @{ id = 'SAFE-01/bound-source-replacement'; path = 'crates/hangar-mutation/src/bound_fs.rs'; markers = @('fn bound_source_denies_path_replacement_after_hash') },
    @{ id = 'SAFE-02/cross-volume-copy-recovery'; path = 'crates/hangar-mutation/src/recover.rs'; markers = @('fn exposes_a_cross_volume_copy_left_beside_the_original') },
    @{ id = 'SAFE-03/held-object-substitution'; path = 'crates/hangar-mutation/src/recover.rs'; markers = @('fn batch_recovery_never_accepts_same_path_replacement_as_deleted_or_original') },
    @{ id = 'SAFE-04/post-arm-topology-race'; path = 'crates/hangar-mutation/src/bound_fs.rs'; markers = @('fn final_disposition_post_arm_proof_catches_a_hardlink_won_after_precheck') },
    @{ id = 'SAFE-05/guardian-parent-loss-preservation'; checks = @(
        @{ path = 'crates/hangar-mutation/src/recover.rs'; markers = @(
            'fn guardian_receipt_flushed_before_lost_ack_settles_after_exact_guardian_death',
            'fn guardian_receipt_not_flushed_before_parent_death_stays_unknown',
            'fn authenticated_guardian_receipt_waits_while_exact_guardian_is_alive',
            'fn legacy_live_duplicate_absence_never_settles_and_cancel_restores_same_object',
            'fn tampered_guardian_receipt_never_authorizes_absence',
            'fn substituted_guardian_receipt_path_never_authorizes_absence'
          ) },
        @{ path = 'crates/hangar-mutation/src/elevated_transport.rs'; markers = @(
            'fn synthetic_guardian_cancels_after_parent_handle_and_pipe_die',
            'fn durable_guardian_receipt_rejects_object_mode_nonce_and_operation_replay',
            'fn guardian_reply_failure_after_prove_armed_parent_crash_preserves_bytes',
            'fn guardian_disconnect_after_arm_ready_retains_through_late_arm_and_parent_crash',
            'fn parent_proved_preprove_cancel_notifies_guardian_and_allows_same_session_retry'
          ) },
        @{ path = 'crates/hangar-mutation/src/bound_fs.rs'; markers = @(
            'fn guardian_duplicate_cancels_after_parent_drop_and_parent_cancel_fault'
          ) },
        @{ path = 'crates/hangar-mutation/src/final_remove.rs'; markers = @(
            'let guardian_outcome = guardian.cancel(mode).ok();',
            'if cancelled_safe || parent_cancelled'
          ) }
      ) },
    @{ id = 'SAFE-05/prove-armed-reply-loss-preservation'; path = 'crates/hangar-mutation/src/elevated_transport.rs'; markers = @(
        'fn guardian_reply_failure_after_prove_armed_parent_crash_preserves_bytes'
      ) },
    @{ id = 'SAFE-05/arm-ready-disconnect-late-arm-preservation'; path = 'crates/hangar-mutation/src/elevated_transport.rs'; markers = @(
        'fn guardian_disconnect_after_arm_ready_retains_through_late_arm_and_parent_crash'
      ) },
    @{ id = 'SAFE-07/restore-no-overwrite'; path = 'crates/hangar-mutation/src/restore.rs'; markers = @('fn restore_never_overwrites_an_occupied_original') },
    @{ id = 'SAFE-08/cloud-files-project-admission-no-io'; path = 'crates/hangar-api/src/lib.rs'; markers = @(
        'fn validate_plan_candidate_source',
        'fn both_cloud_states_block_every_project_mutation_entrypoint_before_source_io'
      ) },
    @{ id = 'SAFE-11/same-metadata-replacement'; path = 'crates/hangar-mutation/src/backup.rs'; markers = @('fn backup_rejects_same_bytes_with_a_different_reviewed_identity') },
    @{ id = 'SAFE-13/crash-ordering-recovery'; path = 'crates/hangar-mutation/src/bound_fs.rs'; markers = @('fn copy_failure_after_create_removes_the_exact_new_object', 'fn abandoned_archive_partial_is_removed_by_its_create_handle_close') },
    @{ id = 'UX-02/deep-scan-terminal-truth'; path = 'apps/desktop/src/__tests__/deep-scan-state.test.ts'; markers = @('Deep Scan terminal truth') },
    @{ id = 'UX-03/wsl-opt-in-off'; path = 'crates/hangar-api/src/lib.rs'; markers = @('fn project_discovery_entrypoints_never_select_wsl_source_when_opted_out') },
    @{ id = 'UX-03/wsl-opt-in-on'; path = 'crates/hangar-api/src/lib.rs'; markers = @('fn project_discovery_entrypoints_select_injected_wsl_source_when_opted_in') },
    @{ id = 'UX-04/undated-review-identity'; path = 'crates/hangar-api/src/project_review.rs'; markers = @('fn undated_session_fingerprint_accepts_only_the_bounded_wire_format') },
    @{ id = 'UX-05/cold-shell-preview-toggle'; path = 'apps/desktop/src/__tests__/preview-refresh.test.ts'; markers = @('keeps a cold shell-open preview', 'wires Rendered/Source mode changes') },
    @{ id = 'UX-06/safe-manage-first-run-three-choice-contract'; checks = @(
        @{ path = 'apps/desktop/src/__tests__/safe-manage-portfolio.test.ts'; markers = @('first-run choices persist distinct behavior and only Analyze now starts work') },
        @{ path = 'crates/hangar-api/src/safe_manage.rs'; markers = @('fn first_run_analyze_later_and_suppress_have_distinct_persisted_outcomes') }
      ) },
    @{ id = 'UX-07/safe-manage-ai-enriched-recommendation'; checks = @(
        @{ path = 'apps/desktop/src/__tests__/connector-ai-security-contract.test.ts'; markers = @(
            'keeps AI-enriched Safe Manage recommendations exact, redacted, one-shot and non-authoritative'
          ) },
        @{ path = 'crates/hangar-api/src/ai_assist.rs'; markers = @(
            'fn safe_manage_ai_recommendation_parser_is_exact_and_fail_closed'
          ) },
        @{ path = 'crates/hangar-api/src/connector_advisory.rs'; markers = @(
            'fn connector_enriches_confident_results_but_never_a_do_not_touch_floor'
          ) }
      ) },
    @{ id = 'UX-09/theme-motion-static'; path = 'apps/desktop/src/__tests__/phase8-polish.test.ts'; markers = @('honours both OS and in-app reduced-motion settings') },
    @{ id = 'UX-11/edition-loading-truth'; path = 'apps/desktop/src/__tests__/edition-identity.test.ts'; markers = @('compile-time edition identity', 'integration availability truth') }
  )
  foreach ($claim in $claims) {
    $checks = if ($claim.ContainsKey('checks')) {
      @($claim.checks)
    } else {
      @([pscustomobject]@{ path = [string]$claim.path; markers = @($claim.markers) })
    }
    foreach ($check in $checks) {
      $full = Join-Path $repoRoot ([string]$check.path).Replace('/', '\')
      if (-not (Test-Path -LiteralPath $full -PathType Leaf)) {
        throw "Canonical local-CI test-claim source is missing: $($check.path)"
      }
      $source = [System.IO.File]::ReadAllText($full)
      foreach ($marker in @($check.markers)) {
        if (-not $source.Contains([string]$marker, [System.StringComparison]::Ordinal)) {
          throw "Canonical local-CI test claim $($claim.id) is missing its exact marker '$marker' in $($check.path)."
        }
      }
    }
  }
  return @($claims | ForEach-Object { [string]$_.id })
}

function Get-LocalCiEvidenceRoot {
  return [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local\acceptance\v0.1.3\local-ci')).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
}

function Resolve-NewLocalCiEvidenceDirectory {
  param([Parameter(Mandatory = $true)][string]$RequestedDirectory)
  if (-not [System.IO.Path]::IsPathFullyQualified($RequestedDirectory)) {
    $RequestedDirectory = Join-Path $repoRoot $RequestedDirectory
  }
  $full = [System.IO.Path]::GetFullPath($RequestedDirectory).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $allowedRoot = Get-LocalCiEvidenceRoot
  $prefix = $allowedRoot + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not ([System.IO.Path]::GetDirectoryName($full)).Equals($allowedRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
      (Test-Path -LiteralPath $full)) {
    throw "EvidenceDir must be a new direct child below $allowedRoot"
  }
  return $full
}

function Initialize-CanonicalLocalCiEvidence {
  param([Parameter(Mandatory = $true)][string]$RequestedDirectory)
  $full = Resolve-NewLocalCiEvidenceDirectory -RequestedDirectory $RequestedDirectory
  $releaseStressRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local\acceptance\v0.1.3\release-stress')).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
  $script:ReleaseStressEvidenceDirectory = Join-Path $releaseStressRoot ([System.IO.Path]::GetFileName($full))
  if (Test-Path -LiteralPath $script:ReleaseStressEvidenceDirectory) {
    throw "The matching release-stress evidence attempt already exists: $script:ReleaseStressEvidenceDirectory"
  }
  $script:CanonicalSourceIdentity = Get-CodeHangarCleanGitIdentity -RepoRoot $repoRoot
}

function Write-LocalCiEvidence {
  param([Parameter(Mandatory = $true)][string]$RequestedDirectory, [string[]]$TestIds)
  $full = Resolve-NewLocalCiEvidenceDirectory -RequestedDirectory $RequestedDirectory
  $allowedRoot = Get-LocalCiEvidenceRoot
  $localRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.local'))
  Assert-FixedLocalPathChain -Path $localRoot -Label 'The worktree private-local root'
  [void][System.IO.Directory]::CreateDirectory($localRoot)
  Assert-FixedLocalPathChain -Path $localRoot -Label 'The worktree private-local root' -RequireExisting
  Assert-FixedLocalPathChain -Path $allowedRoot -Label 'The local-CI evidence root'
  [void][System.IO.Directory]::CreateDirectory($allowedRoot)
  Assert-FixedLocalPathChain -Path $allowedRoot -Label 'The local-CI evidence root' -RequireExisting
  $identity = Get-CodeHangarCleanGitIdentity -RepoRoot $repoRoot
  if ($null -eq $script:CanonicalSourceIdentity -or
      $identity.Commit -cne $script:CanonicalSourceIdentity.Commit -or
      $identity.Tree -cne $script:CanonicalSourceIdentity.Tree) {
    throw 'The canonical local-CI commit/tree changed during the release lane.'
  }
  [void][System.IO.Directory]::CreateDirectory($full)
  $record = [ordered]@{
    schemaVersion = 1
    documentType = 'codehangar/local-ci-evidence/1'
    version = '0.1.3'
    status = 'PASS'
    completedAtUtc = [datetime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffffffZ', [System.Globalization.CultureInfo]::InvariantCulture)
    source = [ordered]@{ gitCommit = $identity.Commit; gitTree = $identity.Tree; sourceTreeDirty = $false }
    invocation = [ordered]@{ agentAutomation = $true; skipTauriBuild = $true; coreOnly = $false }
    isolation = [ordered]@{ targetTriple = $supportedWindowsTarget; cargoOffline = $true; npmOffline = $true; cargoBuildJobs = 2; rustTestThreads = 2 }
    completedStepIds = @($script:CompletedSteps | ForEach-Object { [string]$_.id })
    testIds = @($TestIds | Sort-Object -Unique)
  }
  $output = Join-Path $full 'LOCAL-CI-EVIDENCE.private.json'
  $json = ($record | ConvertTo-Json -Depth 8) + "`n"
  $stream = [System.IO.FileStream]::new($output, 'CreateNew', 'Write', 'None')
  try {
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally { $stream.Dispose() }
  $hash = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant()
  Write-Host "Private local-CI evidence: $output" -ForegroundColor Green
  Write-Host "LOCAL-CI-EVIDENCE SHA-256: $hash" -ForegroundColor Green
}

function Invoke-LocalCiEvidenceWriterSelfTest {
  $allowedRoot = Get-LocalCiEvidenceRoot
  $probeName = 'writer-self-test-' + [guid]::NewGuid().ToString('N')
  $probeDirectory = Join-Path $allowedRoot $probeName
  try {
    Write-LocalCiEvidence -RequestedDirectory $probeDirectory -TestIds @('SELFTEST/local-ci-evidence-writer')
    $output = Join-Path $probeDirectory 'LOCAL-CI-EVIDENCE.private.json'
    $record = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json
    if ($record.status -cne 'PASS' -or
        $record.documentType -cne 'codehangar/local-ci-evidence/1' -or
        $record.source.gitCommit -cne $script:CanonicalSourceIdentity.Commit -or
        $record.source.gitTree -cne $script:CanonicalSourceIdentity.Tree -or
        @($record.testIds).Count -ne 1 -or
        [string]$record.testIds[0] -cne 'SELFTEST/local-ci-evidence-writer') {
      throw 'The local-CI evidence writer self-test produced an invalid record.'
    }
  } finally {
    if (Test-Path -LiteralPath $probeDirectory) {
      $fullProbe = [System.IO.Path]::GetFullPath($probeDirectory).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
      $expectedParent = [System.IO.Path]::GetDirectoryName($fullProbe)
      if (-not $expectedParent.Equals($allowedRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
          [System.IO.Path]::GetFileName($fullProbe) -cne $probeName -or
          ([System.IO.File]::GetAttributes($fullProbe) -band [System.IO.FileAttributes]::ReparsePoint)) {
        throw 'The local-CI evidence writer self-test cleanup target drifted.'
      }
      [System.IO.Directory]::Delete($fullProbe, $true)
    }
  }
  Write-Host 'Local-CI evidence writer self-test passed.' -ForegroundColor Green
}

# Pure regression check (no I/O), so the gate logic stays testable. A step is a regression only when
# it is BOTH more than $Tolerance x its baseline AND grew by more than $FloorMs ms - the floor keeps
# noise on fast steps (50 ms -> 150 ms) from ever tripping the gate; only slow steps that blow up do.
function Get-PerfRegressions {
  param(
    [Parameter(Mandatory = $true)] $BaselineSteps,
    [Parameter(Mandatory = $true)] $CurrentSteps,
    [double]$Tolerance = 2.0,
    [int]$FloorMs = 5000
  )
  $regressions = @()
  foreach ($step in $CurrentSteps) {
    $base = $BaselineSteps | Where-Object { $_.name -eq $step.name } | Select-Object -First 1
    if ($null -eq $base) { continue }
    if ($step.elapsedMs -gt ($base.elapsedMs * $Tolerance) -and ($step.elapsedMs - $base.elapsedMs) -gt $FloorMs) {
      $regressions += [pscustomobject]@{ name = $step.name; baselineMs = [int]$base.elapsedMs; nowMs = [int]$step.elapsedMs }
    }
  }
  return $regressions
}

# Write the machine-local baseline (hostname + git sha stamped, never compared across machines and
# never committed). Returns the baseline object. Shared by -Measure and the -PerfGate bootstrap.
function Write-PerfBaseline {
  param([Parameter(Mandatory = $true)][string]$Path)
  $gitSha = ""
  try { $gitSha = (git rev-parse --short HEAD 2>$null) } catch { $gitSha = "" }
  $baseline = [pscustomobject]@{
    recordedAt = (Get-Date).ToString("o")
    machine    = $env:COMPUTERNAME
    gitSha     = $gitSha
    totalMs    = ($script:StepTimings | Measure-Object -Property elapsedMs -Sum).Sum
    steps      = $script:StepTimings
  }
  $baseline | ConvertTo-Json -Depth 5 | Set-Content -Path $Path -Encoding UTF8
  return $baseline
}

# The connected-AI-app (MCP) crates always link hangar-api with agent_automation
# (the server cannot function without authenticated dispatch). They must NOT be
# pulled into the core/mutation feature lanes, or those lanes would stop proving
# core/Local isolation. They are excluded here and covered by their own lane
# below; Local/core isolation is separately guaranteed by
# check-no-outbound-deps.mjs (a targeted cargo-tree over code-hangar-desktop).
$connectedAppCrates = @(
  "--exclude", "hangar-mcp",
  "--exclude", "code-hangar-mcp",
  "--exclude", "hangar-appconfig"
)

if (-not [string]::IsNullOrWhiteSpace($EvidenceDir)) {
  Initialize-CanonicalLocalCiEvidence -RequestedDirectory $EvidenceDir
  Invoke-LocalCiEvidenceWriterSelfTest
}

Run-Step "Local packaging deterministic self-tests" { pwsh -NoProfile -File scripts/package-local.ps1 -SelfTest }
Run-Step "Connector packaging deterministic self-tests" { pwsh -NoProfile -File scripts/package-connector.ps1 -SelfTest }
Run-Step "release pipeline parser and contract self-test" { pwsh -NoProfile -File scripts/release-pipeline-self-test.ps1 }
Run-Step "v0.1.3 acceptance evidence self-test" { pwsh -NoProfile -File scripts/acceptance-v013.ps1 -SelfTest }
Run-Step "v0.1.3 Claude live-client MCP self-test" { pwsh -NoProfile -File scripts/mcp-claude-real.ps1 -SelfTest }
Run-Step "release artifact proof self-test" { pwsh -NoProfile -File scripts/release-artifact-proof.ps1 -SelfTest }
Run-Step "checksum staging self-test" { pwsh -NoProfile -File scripts/checksums.ps1 -SelfTest }
Run-Step "worktree JavaScript toolchain preflight" { node scripts/packaging-preflight.mjs --toolchain }
Run-Step "npm run check" { npm.cmd run check }
Run-Step "secret scan and worktree publication audit" {
  node scripts/publication-audit.mjs --self-test
  if ($LASTEXITCODE -ne 0) { throw 'Publication audit self-test failed.' }
  npm.cmd run audit:publication:worktree
}
Run-Step "frontend Local edition isolation" { npm.cmd --workspace apps/desktop run build:local }
Run-Step "cargo fmt" { cargo fmt --all --check }
Run-Step "sandbox lifecycle validator self-test" { pwsh -NoProfile -File scripts/sandbox-lifecycle.ps1 -SelfTest }
Run-Step "v0.1.3 release stress evidence self-test" { pwsh -NoProfile -File scripts/release-stress-v013.ps1 -SelfTest }
Run-Step "v0.1.3 sequential release stress lane" {
  if ($null -ne $script:ReleaseStressEvidenceDirectory) {
    pwsh -NoProfile -File scripts/release-stress-v013.ps1 `
      -EvidenceDir $script:ReleaseStressEvidenceDirectory `
      -ExpectedGitCommit $script:CanonicalSourceIdentity.Commit `
      -ExpectedGitTree $script:CanonicalSourceIdentity.Tree
  } else {
    pwsh -NoProfile -File scripts/release-stress-v013.ps1
  }
}
Run-Step "cargo test core" { cargo test --locked --offline --workspace $connectedAppCrates --no-default-features --features core }
Run-Step "cargo clippy core" { cargo clippy --locked --offline --workspace $connectedAppCrates --all-targets --no-default-features --features core -- -D warnings }

if (-not $CoreOnly) {
  Run-Step "P1-UI-E2E-01 native edit and restore journey" { pwsh -NoProfile -File scripts/p1-ui-e2e.ps1 }
  Run-Step "cargo test mutation" { cargo test --locked --offline --workspace $connectedAppCrates --no-default-features --features mutation }
  Run-Step "cargo clippy mutation" { cargo clippy --locked --offline --workspace $connectedAppCrates --all-targets --no-default-features --features mutation -- -D warnings }
  Run-Step "compile-only Windows Local release (nonpublishable)" {
    cargo build --locked --offline --target $supportedWindowsTarget -p code-hangar-desktop --release --no-default-features --features mutation
    if ($LASTEXITCODE -ne 0) { throw 'The compile-only Windows Local release build failed.' }
    Assert-LocalReleaseBinaryIsolation -Path (Join-Path $repoRoot "target\$supportedWindowsTarget\release\code-hangar-desktop.exe")
  }
}

if ($AgentAutomation) {
  Run-Step "frontend Connector edition isolation" { npm.cmd --workspace apps/desktop run build:connector }
  Run-Step "cargo test agent automation" { cargo test --locked --offline --workspace $connectedAppCrates --no-default-features --features agent_automation }
  Run-Step "cargo clippy agent automation" { cargo clippy --locked --offline --workspace $connectedAppCrates --all-targets --no-default-features --features agent_automation -- -D warnings }
  Run-Step "cargo clippy Windows Connector desktop backend" {
    cargo clippy --locked --offline --target $supportedWindowsTarget -p code-hangar-desktop --all-targets --no-default-features --features agent_automation -- -D warnings
  }
  Run-Step "compile-only Windows Connector desktop release (nonpublishable)" {
    cargo build --locked --offline --target $supportedWindowsTarget -p code-hangar-desktop --release --no-default-features --features agent_automation
  }
  Run-Step "compile-only Windows connected-app server release (nonpublishable)" {
    cargo build --locked --offline --target $supportedWindowsTarget -p code-hangar-mcp --release
  }
}

# Dedicated lane for the feature-gated connected-AI-app surface. These crates carry
# their own feature wiring (they pull hangar-api/agent_automation themselves), so
# they build without the workspace feature flags.
Run-Step "cargo test connected-app surface" { cargo test --locked --offline -p hangar-mcp -p code-hangar-mcp -p hangar-appconfig }
Run-Step "cargo clippy connected-app surface" { cargo clippy --locked --offline -p hangar-mcp -p code-hangar-mcp -p hangar-appconfig --all-targets -- -D warnings }

if (-not $SkipTauriBuild) {
  if ($AgentAutomation) {
    # Local CI has no owner signing inputs, so it must never enter PrepareSigning
    # or BundleSigned implicitly. Prove only the explicit, deterministic input
    # preflight here; the receipt-bound two-phase package remains an owner gate.
    Run-Step "Connector packaging preflight (pinned offline WebView2)" {
      pwsh -NoProfile -File scripts/package-connector.ps1 -PreflightOnly -WebView2InstallerPath $WebView2InstallerPath
    }
  } elseif ($CoreOnly) {
    # A core-only binary is an isolation/CI artifact, not the shipped Local edition.
    # Never wrap it in the Local Tauri config/product name.
    Run-Step "cargo build core release (no Tauri bundle)" {
      cargo build --locked --offline -p code-hangar-desktop --release --no-default-features --features core
    }
    Write-Host "CoreOnly produced no installer; any existing NSIS files are stale and were not refreshed or accepted." -ForegroundColor Yellow
  } else {
    Run-Step "Local packaging preflight (pinned offline WebView2)" {
      pwsh -NoProfile -File scripts/package-local.ps1 -PreflightOnly -WebView2InstallerPath $WebView2InstallerPath
    }
  }
}

if ($Measure -or $PerfGate) {
  $localDir = Join-Path $repoRoot ".local"
  if (-not (Test-Path $localDir)) {
    New-Item -ItemType Directory -Path $localDir | Out-Null
  }
  $baselinePath = Join-Path $localDir "perf-baseline.json"
  $totalNow = ($script:StepTimings | Measure-Object -Property elapsedMs -Sum).Sum

  if ($PerfGate) {
    # STAGE 2 (blocking): compare against the existing baseline and fail on a gross regression.
    if (-not (Test-Path $baselinePath)) {
      Write-Host ""
      Write-Host "Perf gate: no baseline yet - bootstrapping one from this green run. Re-run with -PerfGate to enforce." -ForegroundColor Yellow
      [void](Write-PerfBaseline -Path $baselinePath)
    } else {
      $baseline = Get-Content $baselinePath -Raw | ConvertFrom-Json
      if ($baseline.machine -ne $env:COMPUTERNAME) {
        Write-Host ""
        Write-Host ("Perf gate: baseline is from '{0}', this is '{1}' - skipping (baselines are per-machine; record one here with -Measure)." -f $baseline.machine, $env:COMPUTERNAME) -ForegroundColor Yellow
      } else {
        $tolerance = 2.0
        $totalTolerance = 1.5
        $regressions = Get-PerfRegressions -BaselineSteps $baseline.steps -CurrentSteps $script:StepTimings -Tolerance $tolerance
        $totalLimit = [int]($baseline.totalMs * $totalTolerance)
        $totalRegressed = $totalNow -gt $totalLimit
        if ($regressions.Count -gt 0 -or $totalRegressed) {
          Write-Host ""
          Write-Host "Perf gate FAILED - gross regression vs baseline:" -ForegroundColor Red
          foreach ($r in $regressions) {
            $factor = [math]::Round($r.nowMs / [math]::Max(1, $r.baselineMs), 2)
            Write-Host ("  {0}: {1} ms -> {2} ms ({3}x baseline)" -f $r.name, $r.baselineMs, $r.nowMs, $factor) -ForegroundColor Red
          }
          if ($totalRegressed) {
            Write-Host ("  TOTAL: {0} ms -> {1} ms (limit {2} ms)" -f $baseline.totalMs, $totalNow, $totalLimit) -ForegroundColor Red
          }
          Write-Host "If this slowdown is expected, refresh the baseline with -Measure." -ForegroundColor Yellow
          throw "Perf gate failed: gross performance regression vs baseline."
        }
        Write-Host ""
        Write-Host ("Perf gate passed - every step within {0}x of baseline (total {1} ms vs baseline {2} ms, limit {3} ms)." -f $tolerance, $totalNow, $baseline.totalMs, $totalLimit) -ForegroundColor Green
      }
    }
  }

  if ($Measure) {
    # STAGE 1: (re)record the machine-local baseline from this green run.
    $baseline = Write-PerfBaseline -Path $baselinePath
    Write-Host ""
    Write-Host ("Perf baseline written to {0} ({1} steps, {2} ms total)." -f $baselinePath, $script:StepTimings.Count, $baseline.totalMs) -ForegroundColor Green
  }
}

$localCiTestIds = @(Get-CanonicalLocalCiTestClaims)
if (-not [string]::IsNullOrWhiteSpace($EvidenceDir)) {
  Write-LocalCiEvidence -RequestedDirectory $EvidenceDir -TestIds $localCiTestIds
}

Write-Host ""
Write-Host "Local CI passed." -ForegroundColor Green
