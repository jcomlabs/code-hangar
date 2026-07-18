import type {
  PinnedItem,
  ProtectedZone,
  RecentItem,
  ScanRoot,
  SecurityStatus
} from "./types";

export interface StartupSideData {
  recentItems: RecentItem[];
  pinnedItems: PinnedItem[];
  roots: ScanRoot[];
  zones: ProtectedZone[];
  security: SecurityStatus;
}

export type StartupSideDataLoaders = {
  [Key in keyof StartupSideData]: () => Promise<StartupSideData[Key]>;
};

export interface StartupSideDataResult {
  data: Partial<StartupSideData>;
  failures: Array<{ key: keyof StartupSideData; message: string }>;
}

const SIDE_DATA_KEYS: Array<keyof StartupSideData> = [
  "recentItems",
  "pinnedItems",
  "roots",
  "zones",
  "security"
];

export async function loadStartupSideData(
  loaders: StartupSideDataLoaders
): Promise<StartupSideDataResult> {
  const settled = await Promise.allSettled(
    SIDE_DATA_KEYS.map((key) => loaders[key]())
  );
  const data: Partial<StartupSideData> = {};
  const failures: StartupSideDataResult["failures"] = [];

  settled.forEach((result, index) => {
    const key = SIDE_DATA_KEYS[index];
    if (result.status === "fulfilled") {
      Object.assign(data, { [key]: result.value });
      return;
    }
    failures.push({
      key,
      message: result.reason instanceof Error ? result.reason.message : String(result.reason)
    });
  });

  return { data, failures };
}
