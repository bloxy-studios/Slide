export type Runtime = { path: string; brand: string; version: string; architecture: string; bundled: boolean };
export type Status = keyof typeof statusLabels;
export const statusLabels = {
  closed: "Closed", running: "Running", background: "No open window", starting: "Opening…",
  closing: "Closing…", locked: "Profile in use", crashed: "Exited unexpectedly",
};
export type Operation = "open" | "focus" | "close";
export type ConnectionSelection = { mode: "system" } | { mode: "direct" } | { mode: "saved"; id: string } | { mode: "workspace"; endpoint: string };
export type SavedConnection = { id: string; name: string; endpoint: string };
export type Settings = {
  icon: string; notes: string; launchUrls: string[]; folderId: string | null;
  tagIds: string[]; pinnedTagId: string | null; bookmarkIds: string[]; extensionIds: string[];
  connection: ConnectionSelection;
};
export const emptySettings: Settings = {
  icon: "", notes: "", launchUrls: [], folderId: null,
  tagIds: [], pinnedTagId: null, bookmarkIds: [], extensionIds: [],
  connection: { mode: "system" },
};
export type Workspace = {
  id: string; name: string; settings: Settings; status: Status; detail: string | null;
  canClose: boolean; createdAt: number; trashedAt: number | null;
};
export type Label = { id: string; name: string };
export type Tag = Label & { color: string };
export type Bookmark = Label & { url: string };
export type Template = Label & { settings: Settings };
export type Extension = Label & {
  browserId: string; source: "fixture" | "zip" | "store"; version: string; permissions: string[];
  updateMode: string; installUrl: string | null; unpackedPath: string | null;
};
export type Reminder = {
  id: string; title: string; description: string; dueAt: number; firedAt: number | null;
  completedAt: number | null; workspaceIds: string[];
};
export type Column = "status" | "folder" | "tags" | "urls" | "notes" | "added";
export const columnLabels: Record<Column, string> = {
  status: "Status", folder: "Folder", tags: "Tags", urls: "Launch URLs", notes: "Notes", added: "Created",
};
export const defaultColumns: Column[] = ["status", "folder", "tags", "urls"];
export type View = {
  search: string; folder: string; tags: string[]; status: string; sort: string; pageSize: number; columns: Column[] | null;
};
export type Catalog = {
  activeAccount: string; accounts: Label[]; folders: Label[]; tags: Tag[]; bookmarks: Bookmark[];
  templates: Template[]; extensions: Extension[]; tasks: Reminder[]; defaultTemplate: string | null; view: View;
  connections: SavedConnection[];
};
export type Snapshot = {
  runtime: Runtime | null; runtimeError: string | null; hasBundledRuntime: boolean;
  platform: string; requiredHttpProxyAvailable: boolean; workspaces: Workspace[]; catalog: Catalog;
};
export type PreviewRow = { number: number; name: string; duplicate: boolean; completed: boolean; error: string | null };
export type Preview = { fingerprint: string; rows: PreviewRow[] };

export function listRows(rows: Workspace[], catalog: Pick<Catalog, "folders" | "tags">, view: View, trash: boolean) {
  const search = view.search.trim().toLocaleLowerCase();
  return rows.filter(row => {
    if ((row.trashedAt !== null) !== trash) return false;
    if (view.folder === "unfiled" ? row.settings.folderId !== null : view.folder && row.settings.folderId !== view.folder) return false;
    if (view.status && row.status !== view.status) return false;
    if (!view.tags.every(tag => row.settings.tagIds.includes(tag))) return false;
    if (!search) return true;
    const folder = catalog.folders.find(folder => folder.id === row.settings.folderId)?.name ?? "";
    const tags = catalog.tags.filter(tag => row.settings.tagIds.includes(tag.id)).map(tag => tag.name);
    return [row.name, row.settings.notes, folder, ...tags, ...row.settings.launchUrls].some(value => value.toLocaleLowerCase().includes(search));
  }).sort((a, b) => {
    switch (view.sort) {
      case "nameDesc": return b.name.localeCompare(a.name);
      case "newest": return b.createdAt - a.createdAt;
      case "status": return statusLabels[a.status].localeCompare(statusLabels[b.status]) || a.name.localeCompare(b.name);
      default: return a.name.localeCompare(b.name);
    }
  });
}

export function localTime(utc: number) {
  return new Date(utc * 1000).toLocaleString();
}

export type Outcome = { key: string; name: string; state: "Queued" | "Running" | "Succeeded" | "Failed" | "Cancelled"; detail: string };
export async function processBatch(
  jobs: { key: string; name: string; work: () => Promise<string> }[], workers: number,
  isCancelled: () => boolean, update: (outcome: Outcome) => void,
) {
  let next = 0;
  const worker = async () => {
    for (let job = jobs[next++]; job; job = jobs[next++]) {
      const label = { key: job.key, name: job.name };
      if (isCancelled()) {
        update({ ...label, state: "Cancelled", detail: "Not started" });
        continue;
      }
      update({ ...label, state: "Running", detail: "" });
      try {
        update({ ...label, state: "Succeeded", detail: await job.work() });
      } catch (e) {
        update({ ...label, state: "Failed", detail: String(e) });
      }
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, Math.min(workers, 2, jobs.length)) }, worker));
}
