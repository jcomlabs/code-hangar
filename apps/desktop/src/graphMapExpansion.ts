import type { GraphMap } from "./types";

export const INITIAL_GRAPH_MAP_LIMIT = 300;
export const GRAPH_MAP_EXPANSION_BATCH = 500;

export function graphMapItemCounts(map: GraphMap): { loadedItems: number; totalItems: number } {
  const loadedItems = map.nodes.filter((node) => node.graphKind !== "project").length;
  return {
    loadedItems,
    totalItems: Math.max(loadedItems, map.totalNodes - 1)
  };
}

export function nextGraphMapExpansionLimit(map: GraphMap, batchSize = GRAPH_MAP_EXPANSION_BATCH): number | null {
  if (map.nodes.length >= map.totalNodes) return null;
  // The backend reconstructs the bounded map for each request. Grow geometrically
  // after the minimum batch so a large, explicitly requested map does not repeat
  // the same full local inventory work hundreds of times.
  const minimumNextLimit = map.nodes.length + Math.max(1, batchSize);
  const doubledLimit = map.nodes.length * 2;
  return Math.min(map.totalNodes, Math.max(minimumNextLimit, doubledLimit));
}
