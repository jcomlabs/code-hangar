import { AlertTriangle, CheckCircle2, FolderOpen } from "lucide-react";

import type { FolderExplanation } from "../../types";
import { formatOptionalBytes, plainConfidenceLabel } from "../../ui";

export function folderClassificationLabel(value: string): string {
  const readable = value.trim().replace(/[_-]+/g, " ");
  if (!readable) return "Folder";
  return readable.charAt(0).toUpperCase() + readable.slice(1);
}

export function folderInventoryLabel(folder: Pick<FolderExplanation, "fullyScanned" | "scanError">): string {
  if (folder.scanError) return "Scan issue";
  return folder.fullyScanned ? "Complete" : "Partial";
}

export function FolderOverviewPane({ folder }: { folder: FolderExplanation }) {
  const partialSuffix = folder.footprintPartial ? "+" : "";
  const classification = folderClassificationLabel(folder.classification);
  const inventoryLabel = folderInventoryLabel(folder);

  return (
    <div className="project-home folder-overview">
      <div className="project-home-intro folder-overview-intro">
        <span>Folder overview</span>
        <div className="folder-overview-title">
          <span className="folder-overview-icon" aria-hidden="true"><FolderOpen size={20} /></span>
          <div>
            <h2>{folder.displayName}</h2>
            <code>{folder.displayPath}</code>
          </div>
        </div>
        <p>{folder.summary}</p>
        <div className="folder-overview-badges" aria-label="Folder classification">
          <span>{classification}</span>
          <span>{plainConfidenceLabel(folder.confidence, "folder match")}</span>
          {folder.protectedLevel ? <span>Protected metadata</span> : null}
        </div>
      </div>

      {folder.scanError ? (
        <div className="project-home-warning folder-overview-warning" role="status">
          <AlertTriangle size={15} />
          <span>{folder.scanError}</span>
        </div>
      ) : !folder.fullyScanned ? (
        <div className="project-home-warning folder-overview-warning" role="status">
          <AlertTriangle size={15} />
          <span>This folder scan is incomplete, so sizes and child counts are minimum known values.</span>
        </div>
      ) : null}

      <section className="project-home-section" aria-label="Folder facts">
        <div className="project-metric-grid folder-overview-metrics">
          <div>
            <span>Space used on disk</span>
            <strong>{formatOptionalBytes(folder.physicalBytes)}{partialSuffix}</strong>
          </div>
          <div>
            <span>Total file sizes</span>
            <strong>{formatOptionalBytes(folder.apparentBytes)}{partialSuffix}</strong>
          </div>
          <div>
            <span>Direct children</span>
            <strong>{folder.childCount.toLocaleString()}</strong>
          </div>
          <div>
            <span>Inventory state</span>
            <strong>{inventoryLabel}</strong>
          </div>
        </div>
      </section>

      {folder.signals.length ? (
        <section className="project-home-section folder-overview-evidence">
          <h3>Why it was classified this way</h3>
          <ul>
            {folder.signals.map((signal) => (
              <li key={signal}><CheckCircle2 size={15} /><span>{signal}</span></li>
            ))}
          </ul>
        </section>
      ) : null}

      {folder.caveats.length ? (
        <section className="project-home-section folder-overview-evidence caveats">
          <h3>Review notes</h3>
          <ul>
            {folder.caveats.map((caveat) => (
              <li key={caveat}><AlertTriangle size={15} /><span>{caveat}</span></li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
