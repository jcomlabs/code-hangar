// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

function sourceBetween(start: string, end: string) {
  const startIndex = appSource.indexOf(start);
  const endIndex = appSource.indexOf(end, startIndex + start.length);
  expect(startIndex, `missing source marker: ${start}`).toBeGreaterThanOrEqual(0);
  expect(endIndex, `missing source marker: ${end}`).toBeGreaterThan(startIndex);
  return appSource.slice(startIndex, endIndex);
}

describe("project refresh contract", () => {
  it("loads one complete project snapshot after the encrypted startup cache", () => {
    const startup = sourceBetween("const runStartup = async () =>", "void runStartup();");

    expect(startup).toContain("await api.projectsCachedSnapshot()");
    expect(startup.match(/api\.projectsList\(\)/g)).toHaveLength(1);
    expect(startup).not.toContain("api.projectsListLite()");
    expect(startup).not.toContain("Project metadata refresh failed");
  });

  it("drains an Explorer Markdown request before provider-enriched project loading", () => {
    const startup = sourceBetween("const runStartup = async () =>", "void runStartup();");
    const inventoryReadyIndex = startup.indexOf("setInventoryReady(true)");
    const shellDrainIndex = startup.indexOf("await drainShellOpenRequests()", inventoryReadyIndex);
    const projectListIndex = startup.indexOf("await api.projectsList()", inventoryReadyIndex);

    expect(inventoryReadyIndex).toBeGreaterThanOrEqual(0);
    expect(shellDrainIndex).toBeGreaterThan(inventoryReadyIndex);
    expect(projectListIndex).toBeGreaterThan(shellDrainIndex);
    expect(startup).toContain("if (!shellOpenHasPriorityRef.current)");
  });

  it("starts direct shell preview before cached hydration and preserves that newer target", () => {
    const eagerDrainIndex = appSource.indexOf("void drainShellOpenRequests();");
    const startup = sourceBetween("const runStartup = async () =>", "void runStartup();");
    const cachedReadIndex = appSource.indexOf("const cachedProjects = await api.projectsCachedSnapshot()", eagerDrainIndex);
    const cacheHydration = startup.slice(
      startup.indexOf("const cachedProjects = await api.projectsCachedSnapshot()"),
      startup.indexOf("await yieldToUi();", startup.indexOf("const cachedProjects = await api.projectsCachedSnapshot()"))
    );

    expect(eagerDrainIndex).toBeGreaterThanOrEqual(0);
    expect(cachedReadIndex).toBeGreaterThan(eagerDrainIndex);
    expect(cacheHydration).toContain("if (shellOpenHasPriorityRef.current)");
    expect(cacheHydration).toContain("setProjects((current) => {");
    expect(cacheHydration).toContain("current.filter((project) => !cachedIds.has(project.id))");
  });

  it("does not await catalogs or roots before showing the requested shell file", () => {
    const shellOpen = sourceBetween(
      "const previewShellOpenPath = useCallback",
      "const drainShellOpenRequests = useCallback"
    );
    const immediateOpenIndex = shellOpen.indexOf("openedPreview = await openShellFileImmediately(");
    const projectDetailIndex = shellOpen.indexOf("await api.projectGet(target.projectId)");
    const yieldIndex = shellOpen.indexOf("await afterFirstPaint()", immediateOpenIndex);
    const scanStartIndex = shellOpen.indexOf("await api.startOpenTargetScan", immediateOpenIndex);
    const previousViewerDisposeIndex = shellOpen.indexOf("void queueShellViewerDisposal(viewerToRetire)");
    const directReadIndex = shellOpen.indexOf("await api.openLocalFilePreview");
    const provisionalPaintIndex = shellOpen.indexOf("await openShellFileImmediately(provisionalTarget, direct.preview, true)");
    const inventoryWaitIndex = shellOpen.indexOf("await waitForShellInventory()");
    const inspectionIndex = shellOpen.indexOf("await api.inspectOpenTarget(path)");

    expect(immediateOpenIndex).toBeGreaterThanOrEqual(0);
    expect(projectDetailIndex).toBeGreaterThan(immediateOpenIndex);
    expect(yieldIndex).toBeGreaterThan(immediateOpenIndex);
    expect(scanStartIndex).toBeGreaterThan(yieldIndex);
    expect(previousViewerDisposeIndex).toBeGreaterThan(immediateOpenIndex);
    expect(directReadIndex).toBeGreaterThanOrEqual(0);
    expect(provisionalPaintIndex).toBeGreaterThan(directReadIndex);
    expect(inventoryWaitIndex).toBeGreaterThan(provisionalPaintIndex);
    expect(inspectionIndex).toBeGreaterThan(inventoryWaitIndex);
    expect(shellOpen).toContain("const immediateViewerProject: ProjectSummary");
    expect(shellOpen.indexOf("setShellViewer(immediateSession)")).toBeLessThan(yieldIndex);
    expect(shellOpen).toContain("await api.openLocalFilePreview");
    expect(shellOpen.indexOf("await api.openLocalFilePreview")).toBeLessThan(shellOpen.indexOf("await waitForShellInventory()"));
    expect(shellOpen).toContain("project.id !== -1");
    expect(shellOpen).not.toContain("await closeShellViewerSession");
    expect(shellOpen).not.toContain("await api.projectsListLite()");
    expect(shellOpen).not.toContain("await api.rootsList()");
    expect(shellOpen).toContain("void api.rootsList().then(setRoots)");
    expect(shellOpen).toContain('label: "File open"');
    expect(shellOpen).toContain("await afterFirstPaint()");
    expect(shellOpen).toContain("const reconcileShellOpenPath = useCallback");
    expect(shellOpen).toContain("shellOpenRequestOwnsFocus(");
    expect(shellOpen).toContain("const base = claimedForeground");
    expect(shellOpen).toContain("const immediateProject: ProjectSummary");
    expect(shellOpen.indexOf("setShellViewer(null)")).toBeLessThan(yieldIndex);
    const reconcileLane = sourceBetween(
      "const reconcileShellOpenPath = useCallback",
      "const drainShellOpenRequests = useCallback"
    );
    const inventoryIndex = reconcileLane.indexOf("await waitForShellInventory()");
    const staleGuardIndex = reconcileLane.indexOf("if (!requestOwnsFocus()) {", inventoryIndex);
    const inspectIndex = reconcileLane.indexOf("await api.inspectOpenTarget(path)");
    const prepareIndex = reconcileLane.indexOf("await api.prepareOpenTarget");
    const scanIndex = reconcileLane.indexOf("await api.startOpenTargetScan");
    expect(staleGuardIndex).toBeGreaterThan(inventoryIndex);
    expect(inspectIndex).toBeGreaterThan(staleGuardIndex);
    expect(prepareIndex).toBeGreaterThan(staleGuardIndex);
    expect(scanIndex).toBeGreaterThan(staleGuardIndex);
    expect(reconcileLane).toContain("sequence !== shellOpenLatestRequestSequenceRef.current && !latestViewerOwnsPath");
    expect(reconcileLane).toContain("setTabs((current) => current.filter");
    const detailIndex = reconcileLane.indexOf("await api.projectGet(target.projectId)");
    const ownershipRecheckIndex = reconcileLane.indexOf("claimedForeground = requestOwnsFocus();", detailIndex);
    const catalogPromotionIndex = reconcileLane.indexOf("setProjects((current) => {", ownershipRecheckIndex);
    const preScanGuardIndex = reconcileLane.indexOf("if (!requestOwnsFocus()) return;", catalogPromotionIndex);
    expect(ownershipRecheckIndex).toBeGreaterThan(detailIndex);
    expect(catalogPromotionIndex).toBeGreaterThan(ownershipRecheckIndex);
    expect(preScanGuardIndex).toBeGreaterThan(catalogPromotionIndex);
    expect(reconcileLane).toContain("const activeProvisional = current.find((project) => project.id === -1)");
    const discardHelperIndex = reconcileLane.indexOf("const discardStaleTarget = async () =>");
    const firstDiscardGateIndex = reconcileLane.indexOf("if (await discardStaleTarget()) return;", discardHelperIndex);
    const attachedPreviewIndex = reconcileLane.indexOf("await api.openTargetPreview", firstDiscardGateIndex);
    const secondDiscardGateIndex = reconcileLane.indexOf("if (await discardStaleTarget()) return;", attachedPreviewIndex);
    const projectGetIndex = reconcileLane.indexOf("await api.projectGet", secondDiscardGateIndex);
    const thirdDiscardGateIndex = reconcileLane.indexOf("if (await discardStaleTarget()) return;", projectGetIndex);
    expect(discardHelperIndex).toBeGreaterThan(prepareIndex);
    expect(firstDiscardGateIndex).toBeGreaterThan(discardHelperIndex);
    expect(secondDiscardGateIndex).toBeGreaterThan(attachedPreviewIndex);
    expect(thirdDiscardGateIndex).toBeGreaterThan(projectGetIndex);
    expect(reconcileLane).toContain("await api.discardInvestigation(target.rootId).catch");
    const inspectionGuardIndex = reconcileLane.indexOf("if (!requestOwnsFocus()) return;", inspectIndex);
    const choiceIndex = reconcileLane.indexOf("await requestShellOpenChoice(inspection, sequence)");
    const choiceGuardIndex = reconcileLane.indexOf("if (!requestOwnsFocus()) return;", choiceIndex);
    const pickerIndex = reconcileLane.indexOf("await api.pickFolder");
    const pickerGuardIndex = reconcileLane.indexOf("if (!requestOwnsFocus()) return;", pickerIndex);
    expect(inspectionGuardIndex).toBeGreaterThan(inspectIndex);
    expect(choiceGuardIndex).toBeGreaterThan(choiceIndex);
    expect(pickerGuardIndex).toBeGreaterThan(pickerIndex);
    expect(prepareIndex).toBeGreaterThan(pickerGuardIndex);
    const previewLane = sourceBetween(
      "const previewShellOpenPath = useCallback",
      "const reconcileShellOpenPath = useCallback"
    );
    expect(previewLane).toContain("fullPreviewPromise = afterFirstPaint().then");
    expect(previewLane).toContain("await api.openLocalFilePreviewFull");
    expect(previewLane).not.toContain("waitForShellInventory");
    const fullOwnershipGuardIndex = previewLane.indexOf("if (!ownsFocusBeforeRead) return;");
    const fullReadIndex = previewLane.indexOf("await api.openLocalFilePreviewFull");
    expect(fullOwnershipGuardIndex).toBeGreaterThan(previewLane.indexOf("afterFirstPaint().then"));
    expect(fullReadIndex).toBeGreaterThan(fullOwnershipGuardIndex);
    const directReadGuardIndex = previewLane.indexOf("previewRequestOwnsFocus()");
    const directReadIndexInLane = previewLane.indexOf("await api.openLocalFilePreview");
    const postReadGuardIndex = previewLane.indexOf("if (!previewRequestOwnsFocus())", directReadIndexInLane);
    const provisionalPublishIndex = previewLane.indexOf("await openShellFileImmediately(provisionalTarget");
    const initialSelectionIndex = previewLane.indexOf("const initialSelectionSequence = selectionSeq.current");
    expect(initialSelectionIndex).toBeGreaterThanOrEqual(0);
    expect(initialSelectionIndex).toBeLessThan(directReadIndexInLane);
    expect(previewLane).toContain("initialSelectionSequence,\n      selectionSeq.current");
    expect(directReadGuardIndex).toBeLessThan(directReadIndexInLane);
    expect(postReadGuardIndex).toBeGreaterThan(directReadIndexInLane);
    expect(provisionalPublishIndex).toBeGreaterThan(postReadGuardIndex);
    expect(previewLane.indexOf("await api.openLocalFilePreviewFull")).toBeGreaterThan(
      previewLane.indexOf("afterFirstPaint().then")
    );
    const folderOwnershipIndex = shellOpen.indexOf('if (target.targetKind === "folder") claimedForeground = requestOwnsFocus();');
    const projectPromotionIndex = shellOpen.indexOf("setProjects((current) => {", folderOwnershipIndex);
    expect(folderOwnershipIndex).toBeGreaterThanOrEqual(0);
    expect(projectPromotionIndex).toBeGreaterThan(folderOwnershipIndex);

    const immediateHelper = sourceBetween(
      "const openShellFileImmediately = useCallback",
      "const openWorkspaceTab = useCallback"
    );
    expect(immediateHelper).toContain("if (!provisional)");
    expect(immediateHelper).toContain("tab.projectId === -1");
    expect(immediateHelper).toContain("provisional ? current.filter((tab) => tab.projectId !== -1) : current");
    expect(immediateHelper).toContain("current?.projectId !== -1");
    expect(immediateHelper).toContain("transientShellNodeIdsRef.current.delete(`-1:");
  });

  it("previews a complete Explorer batch before serial inventory attachment", () => {
    const drain = sourceBetween(
      "const drainShellOpenRequests = useCallback",
      "const updateShellIntegration = useCallback"
    );
    const previewIndex = drain.indexOf("requests.push(await previewShellOpenPath");
    const reconcileIndex = drain.indexOf(".then(() => reconcileShellOpenPath(request))");

    expect(previewIndex).toBeGreaterThanOrEqual(0);
    expect(reconcileIndex).toBeGreaterThan(previewIndex);
    expect(drain).toContain("shellOpenLatestRequestSequenceRef.current = sequence");
  });

  it("invalidates an old attachment as soon as a newer native event arrives", () => {
    const listener = sourceBetween(
      'void listen("shell-open-available"',
      'void listen<string>("background-scan-started"'
    );

    expect(listener).toContain("shellOpenRequestSequenceRef.current + 1");
    expect(listener.indexOf("shellOpenLatestRequestSequenceRef.current = Math.max")).toBeLessThan(
      listener.indexOf("setShellOpenRevision")
    );
  });

  it("closes a provisional Viewer locally before attempting a catalog refresh", () => {
    const closeViewer = sourceBetween(
      "const closeShellViewerSession = useCallback",
      "const waitForShellInventory = useCallback"
    );
    const invalidateIndex = closeViewer.indexOf("invalidateShellOpenIntent()");
    const clearSessionIndex = closeViewer.indexOf("setShellViewer(null)", invalidateIndex);
    const clearPreviewIndex = closeViewer.indexOf("setPreview((current) => current");
    const catalogIndex = closeViewer.indexOf("await api.projectsListLite()");

    expect(invalidateIndex).toBeGreaterThanOrEqual(0);
    expect(clearSessionIndex).toBeGreaterThan(invalidateIndex);
    expect(clearPreviewIndex).toBeGreaterThan(clearSessionIndex);
    expect(catalogIndex).toBeGreaterThan(clearPreviewIndex);
    expect(closeViewer).toContain("await api.projectsListLite().catch");
    expect(closeViewer).toContain("const closeStillOwnsUi = () =>");
    expect(closeViewer).not.toContain("selectProject(nextProjectId)");
  });

  it("invalidates provisional tab dismissal and same-project navigation intent", () => {
    const tabs = sourceBetween(
      "const dismissProvisionalShellDestination = useCallback",
      "const copyPath = useCallback"
    );
    const navigation = sourceBetween("const selectProject = useCallback", "const handleProjectSearchKeyDown");
    const workspaceNavigation = sourceBetween("const showProjectWorkspace = useCallback", "const openProjectRecap");

    expect(tabs).toContain("invalidateShellOpenIntent()");
    expect(tabs).toContain("tabs.some((tab) => tab.projectId === -1)");
    expect(tabs).toContain("dismissProvisionalShellDestination()");
    expect(tabs).toContain("project.id !== retiredViewer?.project.id");
    expect(tabs).toContain("root.id !== retiredViewer.rootId");
    expect(navigation).toContain("if (!options?.preserveShellIntent) invalidateShellOpenIntent()");
    expect(workspaceNavigation).toContain("invalidateShellOpenIntent()");
    expect(workspaceNavigation).not.toContain("shellViewerRef.current");
    const overviewNavigation = sourceBetween("const showOverview = useCallback", "const showProjectWorkspace = useCallback");
    const discoverNavigation = sourceBetween("const showDiscover = useCallback", "const focusProjectPicker = useCallback");
    const recoveryNavigation = sourceBetween("const showRecovery = useCallback", "const showSettings = useCallback");
    const settingsNavigation = sourceBetween("const showSettings = useCallback", "const startPaneResize = useCallback");
    expect(overviewNavigation).toContain("if (!options?.preserveShellIntent) invalidateShellOpenIntent()");
    expect(discoverNavigation).toContain("invalidateShellOpenIntent()");
    expect(recoveryNavigation).toContain("invalidateShellOpenIntent()");
    expect(settingsNavigation).toContain("invalidateShellOpenIntent()");
    const duplicateNavigation = sourceBetween("const inspectCurrentFileDuplicates = useCallback", "const buildPreviewPlan = useCallback");
    const reviewNavigation = sourceBetween("const showReview = useCallback", "const selectTourExample = useCallback");
    const sessionNavigation = sourceBetween("const openSession = useCallback", "const revealSessionTokens = useCallback");
    expect(duplicateNavigation).toContain("invalidateShellOpenIntent()");
    expect(reviewNavigation).toContain("invalidateShellOpenIntent()");
    expect(sessionNavigation).toContain("invalidateShellOpenIntent()");
    const sessionBackNavigation = sourceBetween("<SessionCenterView", "<ProjectCenterView");
    const unlinkedBackIndex = sessionBackNavigation.indexOf("} else {", sessionBackNavigation.indexOf("previewSessionProject"));
    expect(sessionBackNavigation.indexOf("invalidateShellOpenIntent()", unlinkedBackIndex)).toBeGreaterThan(unlinkedBackIndex);
    expect(sessionBackNavigation.indexOf("setPreviewSession(null)", unlinkedBackIndex)).toBeGreaterThan(
      sessionBackNavigation.indexOf("invalidateShellOpenIntent()", unlinkedBackIndex)
    );
  });

  it("keeps one real Viewer retirement owner across a shell-open batch", () => {
    const previewLane = sourceBetween("const previewShellOpenPath = useCallback", "const reconcileShellOpenPath = useCallback");
    const reconcileLane = sourceBetween("const reconcileShellOpenPath = useCallback", "const drainShellOpenRequests = useCallback");

    expect(previewLane).toContain("shellViewerRetirementRef.current = activeViewer");
    expect(previewLane).not.toContain("previousViewer");
    expect(reconcileLane).toContain("const viewerToRetire = shellViewerRetirementRef.current");
    expect(reconcileLane).toContain("shellViewerRetirementRef.current = null");
    expect(reconcileLane).toContain("queueShellViewerDisposal(viewerToRetire)");
  });

  it("uses atomic scan ownership and suppresses stale scan completion writes", () => {
    const reconcileLane = sourceBetween("const reconcileShellOpenPath = useCallback", "const drainShellOpenRequests = useCallback");
    const startIndex = reconcileLane.indexOf("await api.startOpenTargetScan");
    const startedHereIndex = reconcileLane.indexOf("scanStartedHere = scanStart.startedHere", startIndex);
    const cancelIndex = reconcileLane.indexOf("if (scanStartedHere && shellScanJobId)", startedHereIndex);
    const waitIndex = reconcileLane.indexOf("await waitForShellOpenScan(jobId)");
    const preReadGuardIndex = reconcileLane.indexOf("if (!requestOwnsFocus()) return;", waitIndex);
    const projectsReadIndex = reconcileLane.indexOf("api.projectsListLite()", preReadGuardIndex);
    const postReadGuardIndex = reconcileLane.indexOf("if (!requestOwnsFocus()) return;", projectsReadIndex);
    const projectsWriteIndex = reconcileLane.indexOf("setProjects((current) =>", postReadGuardIndex);

    expect(startedHereIndex).toBeGreaterThan(startIndex);
    expect(cancelIndex).toBeGreaterThan(startedHereIndex);
    expect(preReadGuardIndex).toBeGreaterThan(waitIndex);
    expect(projectsReadIndex).toBeGreaterThan(preReadGuardIndex);
    expect(postReadGuardIndex).toBeGreaterThan(projectsReadIndex);
    expect(projectsWriteIndex).toBeGreaterThan(postReadGuardIndex);
  });

  it("refreshes projects once after a scan while retaining deferred detail work", () => {
    const refresh = sourceBetween(
      "const refreshAfterScanFinish = useCallback",
      "const prepareDiscoverScope = useCallback"
    );

    expect(refresh).toContain("await loadProjects();");
    expect(refresh).not.toContain("loadProjectsLite");
    expect(refresh).not.toContain("api.projectsList()");
    expect(refresh).toContain("await refreshSideData();");
    expect(refresh.match(/await yieldToUi\(\);/g)).toHaveLength(2);
    expect(refresh).toContain("await loadDashboardData(true);");
    expect(refresh).toContain("await loadProjectData(selectedProjectId, false);");
  });
});
