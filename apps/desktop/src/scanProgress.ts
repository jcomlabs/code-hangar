import type { ScanStatus } from "./types";

export function formatScanDuration(ms: number) {
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes < 60) return seconds === 0 ? `${minutes}m` : `${minutes}m ${seconds}s`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes === 0 ? `${hours}h` : `${hours}h ${remainingMinutes}m`;
}

export function compactScanPath(path: string, maxLength = 56) {
  if (path.length <= maxLength) return path;
  const tailLength = Math.max(16, maxLength - 12);
  return `...${path.slice(-tailLength)}`;
}

export function formatScanBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${Math.round(value)} ${units[unit]}` : `${value.toFixed(value >= 10 ? 1 : 2)} ${units[unit]}`;
}

function formatScanPercent(percent: number, completed = false) {
  if (completed) return "100%";
  const capped = Math.min(99, percent);
  return `${capped >= 10 ? capped.toFixed(0) : capped.toFixed(1)}%`;
}

export function scanProgressParts(status: ScanStatus) {
  const isPartial = status.partial || status.state === "partial";
  const terminal = ["completed", "cancelled", "failed", "unknown", "partial"].includes(status.state);
  const endMs = terminal ? status.updatedAtMs : Date.now();
  const elapsedMs = Math.max(0, endMs - status.startedAtMs);
  const lastProgressAtMs = status.lastProgressAtMs ?? status.updatedAtMs;
  const updateAgeMs = terminal ? 0 : Math.max(0, Date.now() - lastProgressAtMs);
  const phaseStartedAtMs = status.phaseStartedAtMs ?? status.updatedAtMs;
  const phaseElapsedMs = Math.max(0, endMs - phaseStartedAtMs);
  const waitingOnFilesystem = updateAgeMs >= 6_000;
  const elapsedSeconds = Math.max(1, elapsedMs / 1000);
  const rate = status.scannedFiles / elapsedSeconds;
  const currentPath = status.currentPath ? compactScanPath(status.currentPath) : null;
  const bottleneckText = scanBottleneckText(status, updateAgeMs, phaseElapsedMs, rate);
  const measuredTimingText = scanMeasuredTimingText(status);

  if (status.scanPhase === "estimating") {
    const bytesText = status.estimatedTotalBytes ? formatScanBytes(status.estimatedTotalBytes) : null;
    const countText = status.estimatedTotalFiles
      ? `Estimate: ${status.estimatedTotalFiles.toLocaleString()} items${bytesText ? ` · ${bytesText}` : ""}`
      : "Estimating items and size before indexing";
    return {
      percent: null,
      countText,
      progressText: "Estimating",
      rateText: waitingOnFilesystem ? `waiting on filesystem ${formatScanDuration(updateAgeMs)}` : "pre-counting folder",
      timeText: `${formatScanDuration(elapsedMs)} elapsed`,
      bottleneckText,
      estimateText: waitingOnFilesystem
        ? "filesystem or cloud provider has not returned new items recently"
        : "building a fresh total before scan",
      currentPath,
      detailText: [
        countText,
        waitingOnFilesystem ? `waiting on filesystem ${formatScanDuration(updateAgeMs)}` : "pre-counting folder",
        `${formatScanDuration(elapsedMs)} elapsed`,
        waitingOnFilesystem ? "try Stop if this remains stuck" : null,
        measuredTimingText ?? bottleneckText,
        currentPath
      ].filter(Boolean).join(" · ")
    };
  }

  const rateText = waitingOnFilesystem
    ? `waiting on filesystem ${formatScanDuration(updateAgeMs)}`
    : status.scannedFiles > 0
      ? `${Math.round(rate).toLocaleString()} items/s`
      : "measuring speed";
  const estimate = status.estimatedTotalFiles && status.estimatedTotalFiles > 0 ? status.estimatedTotalFiles : null;
  if (status.scanPhase === "persisting" || status.scanPhase === "finalizing") {
    const bytesText = status.estimatedTotalBytes ? formatScanBytes(status.estimatedTotalBytes) : null;
    const countText = estimate && status.scannedFiles <= estimate
      ? `${status.scannedFiles.toLocaleString()} / ${estimate.toLocaleString()} items visited${bytesText ? ` · ${bytesText}` : ""}`
      : `${status.scannedFiles.toLocaleString()} items visited${bytesText ? ` · ${bytesText}` : ""}`;
    const progressText = status.scanPhase === "persisting" ? "Persisting" : "Finalizing";
    const estimateText = status.scanPhase === "persisting"
      ? "writing local metadata to SQLite"
      : "updating tree sizes and context";
    return {
      percent: null,
      countText,
      progressText,
      rateText,
      timeText: `${formatScanDuration(elapsedMs)} elapsed`,
      bottleneckText,
      estimateText: waitingOnFilesystem ? `${estimateText}; no new backend update recently` : estimateText,
      currentPath,
      detailText: [countText, progressText, rateText, `${formatScanDuration(elapsedMs)} elapsed`, measuredTimingText ?? bottleneckText, waitingOnFilesystem ? "still responsive; Stop remains available" : estimateText].filter(Boolean).join(" · ")
    };
  }

  const estimateWasReached = Boolean(estimate && status.scannedFiles >= estimate && status.state !== "completed");
  const estimateWasExceeded = Boolean(estimate && status.scannedFiles > estimate && status.state !== "completed");
  const percent = estimate && !estimateWasReached
    ? status.state === "completed"
      ? 100
      : Math.min(99.5, (status.scannedFiles / estimate) * 100)
    : null;
  const remainingMs = !isPartial && estimate && rate > 0 && status.scannedFiles < estimate
    ? ((estimate - status.scannedFiles) / rate) * 1000
    : null;
  const bytesText = status.estimatedTotalBytes ? formatScanBytes(status.estimatedTotalBytes) : null;
  const countText = status.state === "completed"
    ? `${status.scannedFiles.toLocaleString()} items${bytesText ? ` · ${bytesText}` : ""}`
    : estimate
    ? isPartial
      ? `at least ${status.scannedFiles.toLocaleString()} / ${estimate.toLocaleString()} items counted${bytesText ? ` · ${bytesText}` : ""}`
      : estimateWasExceeded
      ? `${status.scannedFiles.toLocaleString()} items · exceeded ${estimate.toLocaleString()} estimate`
      : `${status.scannedFiles.toLocaleString()} / ${estimate.toLocaleString()} items${bytesText ? ` · ${bytesText}` : ""}`
    : isPartial
      ? `at least ${status.scannedFiles.toLocaleString()} items counted`
      : `${status.scannedFiles.toLocaleString()} items`;
  const progressText = isPartial
    ? percent == null ? "Partial" : `Partial ${formatScanPercent(percent)}`
    : estimateWasReached ? "Wrapping up" : percent == null ? null : formatScanPercent(percent, status.state === "completed");
  const timeText = remainingMs == null
    ? `${formatScanDuration(elapsedMs)} elapsed`
    : `ETA ${formatScanDuration(remainingMs)}`;
  const estimateText = isPartial
    ? "Incomplete count; continue scan to complete this folder"
    : estimateWasReached
      ? "scan reached estimate; persisting and final checks remain"
      : waitingOnFilesystem
        ? "filesystem or cloud provider has not returned new items recently"
        : estimate
        ? "based on current estimate"
        : "total unknown";

  return {
    percent,
    countText,
    progressText,
    rateText,
    timeText,
    bottleneckText,
    estimateText,
    currentPath,
    detailText: [countText, progressText, rateText, timeText, measuredTimingText ?? bottleneckText, estimateText, currentPath].filter(Boolean).join(" · ")
  };
}

function scanBottleneckText(status: ScanStatus, updateAgeMs: number, phaseElapsedMs: number, rate: number) {
  if (status.state === "completed") return "finished";
  if (status.state === "cancelled") return "stopped by user";
  if (status.state === "failed") return "failed";
  if (status.scanPhase === "persisting") return status.persistMs ? `SQLite writes measured ${formatScanDuration(status.persistMs)}` : "likely bottleneck: SQLite writes";
  if (status.scanPhase === "finalizing") return status.finalizeMs ? `final accounting measured ${formatScanDuration(status.finalizeMs)}` : "likely bottleneck: final accounting";
  if (status.scanPhase === "cancelling") return "waiting for safe stop checkpoint";
  if (status.currentPath?.toLowerCase().includes("\\onedrive\\")) {
    return updateAgeMs >= 6_000 ? "likely bottleneck: OneDrive filesystem" : "source: OneDrive-backed folder";
  }
  if (updateAgeMs >= 6_000) return "likely bottleneck: filesystem waiting";
  if (status.scanPhase === "estimating" && phaseElapsedMs >= 3_000) return "counting files before indexing";
  if ((status.workerCount ?? 0) > 1 && rate > 0 && rate < 50) return "likely bottleneck: disk or cloud sync";
  if ((status.workerCount ?? 0) > 1 && rate >= 50) return "workers active";
  return "monitoring scan resources";
}

function scanMeasuredTimingText(status: ScanStatus) {
  const parts: string[] = [];
  if (status.estimateMs != null && status.estimateMs > 0) parts.push(`estimate ${formatScanDuration(status.estimateMs)}`);
  if (status.scanMs != null && status.scanMs > 0) parts.push(`walk ${formatScanDuration(status.scanMs)}`);
  if (status.bodyReadMs != null && status.bodyReadMs > 0) parts.push(`read ${formatScanDuration(status.bodyReadMs)}`);
  if (status.persistMs != null && status.persistMs > 0) parts.push(`SQLite ${formatScanDuration(status.persistMs)}`);
  if (status.finalizeMs != null && status.finalizeMs > 0) {
    const details = [
      status.accountingSelectMs != null ? `select ${formatScanDuration(status.accountingSelectMs)}` : null,
      status.accountingComputeMs != null ? `compute ${formatScanDuration(status.accountingComputeMs)}` : null,
      status.accountingUpdateMs != null ? `update ${formatScanDuration(status.accountingUpdateMs)}` : null
    ].filter(Boolean).join(", ");
    parts.push(`final accounting ${formatScanDuration(status.finalizeMs)}${details ? ` (${details})` : ""}`);
  }
  return parts.length ? `measured: ${parts.join(" · ")}` : null;
}
