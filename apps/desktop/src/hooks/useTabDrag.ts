import { useCallback } from "react";
import type { Dispatch, MutableRefObject, PointerEvent as ReactPointerEvent, SetStateAction } from "react";

interface DraggableTab {
  nodeId: number;
}

interface PointerTabDragState {
  nodeId: number;
  startX: number;
  startY: number;
  dragging: boolean;
}

export function reorderTabs<Tab extends DraggableTab>(
  current: Tab[],
  sourceNodeId: number,
  targetNodeId: number,
  position: "before" | "after" = "before"
) {
  if (sourceNodeId === targetNodeId) return current;
  const sourceIndex = current.findIndex((tab) => tab.nodeId === sourceNodeId);
  const targetIndex = current.findIndex((tab) => tab.nodeId === targetNodeId);
  if (sourceIndex === -1 || targetIndex === -1 || sourceIndex === targetIndex) return current;
  const nextTabs = [...current];
  const [movedTab] = nextTabs.splice(sourceIndex, 1);
  const adjustedTargetIndex = sourceIndex < targetIndex ? targetIndex - 1 : targetIndex;
  const insertionIndex = Math.max(
    0,
    Math.min(nextTabs.length, adjustedTargetIndex + (position === "after" ? 1 : 0))
  );
  nextTabs.splice(insertionIndex, 0, movedTab);
  return nextTabs;
}

export function tabStripEdgeTarget(
  sourceNodeId: number,
  pointerX: number,
  tabs: Array<{ nodeId: number; left: number; right: number }>
): { nodeId: number; position: "before" | "after" } | null {
  const candidates = tabs.filter((tab) => tab.nodeId !== sourceNodeId);
  if (candidates.length === 0) return null;
  const first = candidates.reduce((left, tab) => (tab.left < left.left ? tab : left));
  const last = candidates.reduce((right, tab) => (tab.right > right.right ? tab : right));
  if (pointerX <= first.left) return { nodeId: first.nodeId, position: "before" };
  if (pointerX >= last.right) return { nodeId: last.nodeId, position: "after" };
  return null;
}

export function useTabDrag<Tab extends DraggableTab>({
  pointerTabDragRef,
  suppressNextTabClickRef,
  setDraggedTabNodeId,
  setTabDropTargetNodeId,
  setTabs
}: {
  pointerTabDragRef: MutableRefObject<PointerTabDragState | null>;
  suppressNextTabClickRef: MutableRefObject<boolean>;
  setDraggedTabNodeId: Dispatch<SetStateAction<number | null>>;
  setTabDropTargetNodeId: Dispatch<SetStateAction<number | null>>;
  setTabs: Dispatch<SetStateAction<Tab[]>>;
}) {
  const moveTab = useCallback((sourceNodeId: number, targetNodeId: number, position: "before" | "after" = "before") => {
    setTabs((current) => reorderTabs(current, sourceNodeId, targetNodeId, position));
  }, [setTabs]);

  const startTabPointerDrag = useCallback(
    (tab: Tab, event: ReactPointerEvent<HTMLButtonElement>) => {
      if (event.button !== 0) return;
      pointerTabDragRef.current = {
        nodeId: tab.nodeId,
        startX: event.clientX,
        startY: event.clientY,
        dragging: false
      };

      const finishPointerDrag = () => {
        const wasDragging = pointerTabDragRef.current?.dragging ?? false;
        pointerTabDragRef.current = null;
        setDraggedTabNodeId(null);
        setTabDropTargetNodeId(null);
        window.removeEventListener("pointermove", handlePointerMove);
        window.removeEventListener("pointerup", finishPointerDrag);
        window.removeEventListener("pointercancel", finishPointerDrag);
        if (wasDragging) {
          window.setTimeout(() => {
            suppressNextTabClickRef.current = false;
          }, 0);
        }
      };

      const handlePointerMove = (moveEvent: globalThis.PointerEvent) => {
        const dragState = pointerTabDragRef.current;
        if (!dragState) return;
        const deltaX = moveEvent.clientX - dragState.startX;
        const deltaY = moveEvent.clientY - dragState.startY;
        if (!dragState.dragging) {
          if (Math.hypot(deltaX, deltaY) < 6) return;
          dragState.dragging = true;
          suppressNextTabClickRef.current = true;
          setDraggedTabNodeId(dragState.nodeId);
        }

        moveEvent.preventDefault();
        const target = document.elementFromPoint(moveEvent.clientX, moveEvent.clientY);
        const targetTab = target instanceof Element ? target.closest<HTMLElement>(".tab[data-node-id]") : null;
        const targetNodeId = Number(targetTab?.dataset.nodeId);
        if (!Number.isFinite(targetNodeId)) {
          const strip = target instanceof Element ? target.closest<HTMLElement>(".tab-strip") : null;
          if (strip) {
            const bounds = Array.from(strip.querySelectorAll<HTMLElement>(".tab[data-node-id]"))
              .map((element) => {
                const nodeId = Number(element.dataset.nodeId);
                const rect = element.getBoundingClientRect();
                return { nodeId, left: rect.left, right: rect.right };
              })
              .filter((entry) => Number.isFinite(entry.nodeId));
            const edgeTarget = tabStripEdgeTarget(dragState.nodeId, moveEvent.clientX, bounds);
            if (edgeTarget) {
              setTabDropTargetNodeId(edgeTarget.nodeId);
              moveTab(dragState.nodeId, edgeTarget.nodeId, edgeTarget.position);
              return;
            }
          }
          setTabDropTargetNodeId(null);
          return;
        }
        setTabDropTargetNodeId(targetNodeId);
        if (targetNodeId !== dragState.nodeId) {
          const rect = targetTab?.getBoundingClientRect();
          const position = rect && moveEvent.clientX > rect.left + rect.width / 2 ? "after" : "before";
          moveTab(dragState.nodeId, targetNodeId, position);
        }
      };

      window.addEventListener("pointermove", handlePointerMove, { passive: false });
      window.addEventListener("pointerup", finishPointerDrag);
      window.addEventListener("pointercancel", finishPointerDrag);
    },
    [moveTab, pointerTabDragRef, setDraggedTabNodeId, setTabDropTargetNodeId, suppressNextTabClickRef]
  );

  return { moveTab, startTabPointerDrag };
}
