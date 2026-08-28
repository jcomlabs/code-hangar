import { X } from "lucide-react";

import type { TabStripProps } from "./types";

export function TabStrip({
  tabs,
  activeNodeId,
  draggedTabNodeId,
  tabDropTargetNodeId,
  showTabMenu,
  suppressNextTabClick,
  openNode,
  startTabPointerDrag,
  closeTab
}: TabStripProps) {
  if (tabs.length === 0) return null;
  return (
    <div className="tab-strip">
      {tabs.map((tab) => (
        <div
          className={[
            "tab",
            activeNodeId === tab.nodeId ? "active" : "",
            draggedTabNodeId === tab.nodeId ? "dragging" : "",
            tabDropTargetNodeId === tab.nodeId && draggedTabNodeId !== tab.nodeId ? "drop-target" : ""
          ]
            .filter(Boolean)
            .join(" ")}
          key={tab.nodeId}
          data-node-id={tab.nodeId}
          onContextMenu={(event) => showTabMenu(tab, event)}
        >
          <button
            className="tab-main"
            type="button"
            data-help={`Open or drag tab ${tab.label}. Right-click for close options.`}
            onClick={() => {
              if (suppressNextTabClick()) return;
              openNode(tab.nodeId);
            }}
            onPointerDown={(event) => startTabPointerDrag(tab, event)}
          >
            <span>{tab.label}</span>
          </button>
          <button className="tab-close" type="button" aria-label={`Close tab ${tab.label}`} draggable={false} data-help={`Close tab ${tab.label}.`} onClick={() => void closeTab(tab.nodeId)}>
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}
