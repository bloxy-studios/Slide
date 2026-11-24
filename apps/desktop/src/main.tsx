import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useEffectEvent, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type { FormEvent, ReactNode } from "react";
import { columnLabels, defaultColumns, listRows, localTime, processBatch, statusLabels } from "./catalog";
import type { Column, Extension, Operation, Outcome, Preview, Snapshot, View, Workspace } from "./catalog";
import { Checklist, ExtensionForm, kinds, LibraryForm, TaskForm, Tasks, WorkspaceForm } from "./forms";
import type { LibraryItem, LibraryKind, Run } from "./forms";
import "./style.css";

type Tab = "Workspaces" | "Library" | "Reminders" | "Import" | "Extensions" | "Trash";
type WorkspaceMode = "manual" | "quick" | "template" | "clone" | "edit";
type BatchAction = "open" | "close" | "tag" | "folder" | "network" | "trash" | "restore" | "purge";
type Modal =
  | { type: "workspace"; mode: WorkspaceMode; workspace?: Workspace }
  | { type: "library"; kind: LibraryKind; item?: LibraryItem }
  | { type: "extension"; extension?: Extension }
  | { type: "account" | "task" }
  | { type: "confirm"; title: string; detail: string; confirm: () => void };

const tabs: Tab[] = ["Workspaces", "Library", "Reminders", "Import", "Extensions", "Trash"];
const batchLabels: Record<BatchAction, string> = {
  open: "Open", close: "Close", tag: "Add tag", folder: "Move to folder",
  network: "Test local HTTP", trash: "Move to trash", restore: "Restore", purge: "Permanently delete",
};
const sleep = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms));

function App() {
  const [data, setData] = useState<Snapshot>();
  const [tab, setTab] = useState<Tab>("Workspaces");
  const [view, setView] = useState<(View & { owner: string })>();
  const [page, setPage] = useState(0);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [path, setPath] = useState("");
  const [runtimeEditor, showRuntimeEditor] = useState(false);
  const [modal, setModal] = useState<Modal>();
  const [selection, setSelection] = useState<string[]>([]);
  const [batchAction, setBatchAction] = useState<BatchAction>("open");
  const [batchValue, setBatchValue] = useState("");
  const [jobRunning, setJobRunning] = useState(false);
  const [jobTitle, setJobTitle] = useState("");
  const [outcomes, setOutcomes] = useState<Outcome[]>([]);
  const [preview, setPreview] = useState<Preview>();
  const [importSelection, setImportSelection] = useState<string[]>([]);
  const [extensionProfile, setExtensionProfile] = useState("");
  const dialog = useRef<HTMLDialogElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const opLock = useRef(false);
  const jobLock = useRef(false);
  const stopJob = useRef(false);
  const refreshSequence = useRef(0);
  const desiredView = useRef<{ account: string; view: View } | null>(null);
  const writingView = useRef(false);

  async function refresh() {
    const request = ++refreshSequence.current;
    const snapshot = await invoke<Snapshot>("snapshot");
    if (request === refreshSequence.current) setData(snapshot);
    return snapshot;
  }

  useEffect(() => {
    let reading = false, mounted = true;
    const poll = async () => {
      if (reading || opLock.current) return;
      reading = true;
      try { await refresh(); }
      catch (e) { if (mounted) setError(String(e)); }
      finally { reading = false; }
    };
    void poll();
    const timer = setInterval(poll, 1200);
    return () => { mounted = false; clearInterval(timer); };
  }, []);

  const account = data?.catalog.activeAccount;
  useEffect(() => {
    setSelection([]); setPage(0); setPreview(undefined); setImportSelection([]);
    setOutcomes([]); setExtensionProfile("");
  }, [account]);

  const showSearch = useEffectEvent(() => {
    navigate("Workspaces");
    requestAnimationFrame(() => search.current?.focus());
  });

  useEffect(() => {
    function shortcut(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k" && !dialog.current?.open) {
        e.preventDefault();
        showSearch();
      }
    }
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  }, []);

  useEffect(() => {
    if (modal && !dialog.current?.open) dialog.current?.showModal();
  }, [modal]);

  function dismiss() { dialog.current?.close(); setModal(undefined); setError(""); }
  function show(next: Modal) { setError(""); setModal(next); }

  async function run(action: () => Promise<void>, success: string, close = false) {
    if (opLock.current || jobLock.current) return;
    opLock.current = true; setBusy(true); setError(""); setNotice("");
    try {
      await action();
      await refresh();
      if (close) dismiss();
      setNotice(success);
    } catch (e) { setError(String(e)); }
    finally { opLock.current = false; setBusy(false); }
  }
  const command: Run = (name, args, success, close) => {
    void run(async () => {
      await invoke(name, { account, ...args });
      if (name === "remove_item") setView(undefined);
    }, success, close);
  };

  const catalog = data?.catalog;
  const currentView = catalog && (view?.owner === account ? view : catalog.view);
  const workspaces = data?.workspaces ?? [];
  const active = workspaces.filter(row => row.trashedAt === null);
  const due = catalog?.tasks.filter(task => task.firedAt && !task.completedAt).length ?? 0;
  const trash = tab === "Trash";
  const rows = catalog && currentView ? listRows(workspaces, catalog, trash ? { ...currentView, folder: "", tags: [], search: "", status: "" } : currentView, trash) : [];
  const pageSize = currentView?.pageSize || 10;
  const totalPages = Math.max(1, Math.ceil(rows.length / pageSize));
  const pageIndex = Math.min(page, totalPages - 1);
  const pageRows = rows.slice(pageIndex * pageSize, (pageIndex + 1) * pageSize);
  const selectedRows = workspaces.filter(row => selection.includes(row.id) && (row.trashedAt !== null) === trash);
  const columns = currentView?.columns ?? defaultColumns;
  const disabled = busy || jobRunning;

  async function flushView() {
    if (writingView.current) return;
    writingView.current = true;
    try {
      while (desiredView.current) {
        const pending = desiredView.current;
        desiredView.current = null;
        await invoke("save_view", pending);
      }
    } catch (e) { setError(String(e)); }
    finally { writingView.current = false; }
  }

  function updateView(change: Partial<View>) {
    if (!account || !currentView) return;
    const updated: View = { ...currentView, ...change };
    const saved = { search: updated.search, folder: updated.folder, tags: updated.tags, status: updated.status, sort: updated.sort, pageSize: updated.pageSize, columns: updated.columns };
    setView({ ...saved, owner: account }); setPage(0);
    desiredView.current = { account, view: saved };
    void flushView();
  }

  function navigate(next: Tab) {
    setTab(next); setPage(0);
    if (next === "Trash" || tab === "Trash") {
      setSelection([]); setBatchAction(next === "Trash" ? "restore" : "open"); setBatchValue("");
    }
  }

  function action(row: Workspace, operation: Operation) {
    command("workspace_action", { id: row.id, operation }, `${row.name}: ${operation} requested.`);
  }

  async function settle(id: string, operation: "open" | "close") {
    const start = Date.now();
    while (Date.now() - start < 15_000) {
      const snapshot = await refresh();
      const row = snapshot.workspaces.find(row => row.id === id);
      if (!row) throw new Error("Workspace no longer exists.");
      if (operation === "open" && row.status === "running") return "Browser window open";
      if (operation === "close" && (row.status === "closed" || row.status === "background")) {
        return row.status === "background" ? "Window closed; browser remains in background" : "Browser closed";
      }
      if (["locked", "crashed"].includes(row.status)) throw new Error(row.detail || statusLabels[row.status]);
      await sleep(500);
    }
    throw new Error("The browser did not reach the requested state in time.");
  }

  async function startJob(title: string, jobs: { key: string; name: string; work: () => Promise<string> }[], workers: number) {
    if (!jobs.length || jobLock.current || opLock.current) return;
    jobLock.current = true; stopJob.current = false; setJobRunning(true); setError(""); setNotice(""); setJobTitle(title);
    setOutcomes(jobs.map(job => ({ key: job.key, name: job.name, state: "Queued", detail: "" })));
    try {
      await processBatch(jobs, workers, () => stopJob.current, outcome =>
        setOutcomes(current => current.map(row => row.key === outcome.key ? outcome : row)));
      await refresh();
    } catch (e) { setError(String(e)); }
    finally { jobLock.current = false; setJobRunning(false); }
  }

  function beginWorkspaceBatch(scope: Workspace[], action: BatchAction) {
    const title = batchLabels[action];
    const destructive = action === "trash" || action === "purge";
    const execute = () => {
      void startJob(title, scope.map(row => ({
        key: row.id, name: row.name, work: async () => {
          const args = { account, id: row.id };
          if (action === "open" || action === "close") {
            if (action === "close" && ["closed", "crashed"].includes(row.status)) return "Already closed";
            await invoke("workspace_action", { ...args, operation: action });
            return settle(row.id, action);
          }
          if (action === "network") return invoke<string>("network_test", args);
          if (action === "tag" || action === "folder") {
            await invoke("batch_assign", { ...args, kind: action, value: batchValue || null });
            return action === "tag" ? "Tagged" : "Moved";
          }
          await invoke("trash_workspace", { ...args, action, confirmed: action === "purge" });
          return action === "restore" ? "Restored" : action === "trash" ? "Moved to trash" : "Deleted permanently";
        },
      })), action === "open" || action === "close" || destructive ? 1 : 2);
      if (action === "trash" || action === "restore" || action === "purge") setSelection([]);
    };
    if (destructive) {
      show({ type: "confirm", title: `${title} ${scope.length} workspace${scope.length === 1 ? "" : "s"}?`,
        detail: `Scope: ${scope.length === 1 ? scope[0].name : `${scope.length} explicitly selected workspaces, including selected rows on other pages.`} ${action === "purge" ? "Browser data will be removed from this device. This cannot be undone." : "You can restore these workspaces from local Trash."}`,
        confirm: execute });
    } else execute();
  }

  function chooseImport() {
    void run(async () => {
      const path = await open({ multiple: false, filters: [{ name: "Workspace data", extensions: ["csv", "tsv", "txt", "xlsx"] }] });
      if (typeof path !== "string") return;
      setPreview(undefined); setImportSelection([]);
      const document = await invoke<Preview>("preview_import", { account, path });
      setPreview(document);
    }, "");
  }

  function importRows() {
    if (!preview) return;
    const eligible = preview.rows.filter(row => importSelection.includes(String(row.number)) && !row.error && !row.duplicate && !row.completed);
    void startJob("Import", eligible.map(row => ({
      key: String(row.number), name: `Row ${row.number} · ${row.name}`,
      work: () => invoke<string>("import_profile_row", { account, fingerprint: preview.fingerprint, row: row.number }),
    })), 1);
  }

  function pickRuntime(event: FormEvent) {
    event.preventDefault();
    void run(async () => {
      await invoke("choose_runtime", { path });
      showRuntimeEditor(false);
    }, "Browser runtime selected.");
  }

  function cell(row: Workspace, column: Column): ReactNode {
    switch (column) {
      case "status": return <><span className={`badge ${row.status}`}>{statusLabels[row.status]}</span>{row.detail && <p>{row.detail}</p>}</>;
      case "folder": return catalog?.folders.find(folder => folder.id === row.settings.folderId)?.name ?? "—";
      case "tags": return catalog?.tags.filter(tag => row.settings.tagIds.includes(tag.id)).map(tag => {
        const pinned = tag.id === row.settings.pinnedTagId;
        return <span className="badge tag-chip" key={tag.id}><svg width="10" height="10" aria-hidden="true"><circle cx="5" cy="5" r="5" fill={tag.color} /></svg> {pinned ? "★ " : ""}{tag.name}</span>;
      });
      case "urls": return row.settings.launchUrls.length ? <ul className="plain-list">{row.settings.launchUrls.map((url, i) => <li key={i}>{url}</li>)}</ul> : "Browser default";
      case "notes": return row.settings.notes ? <span className="prewrap">{row.settings.notes}</span> : "—";
      case "added": return row.createdAt ? localTime(row.createdAt) : "—";
    }
  }

  return <main>
    <header className="masthead">
      <div><div className="eyebrow">SLIDE / DESKTOP</div><h1>Workspaces</h1><p>Separate browser sessions, on this device.</p></div>
      {catalog && <div className="account-control">
        <label>Local account<select value={account} disabled={disabled} onChange={e => command("switch_account", { id: e.target.value }, "Local account switched.")}>
          {catalog.accounts.map(a => <option key={a.id} value={a.id}>{a.name}</option>)}
        </select></label>
        <button disabled={disabled} onClick={() => show({ type: "account" })}>Add account</button>
      </div>}
    </header>
    <section className="panel runtime" aria-labelledby="runtime-title">
      <div>
        <div className="eyebrow" id="runtime-title">BROWSER RUNTIME</div>
        {data?.runtime ? <>
          <h2>{data.runtime.brand} <span className="version">{data.runtime.version} · {data.runtime.architecture}</span></h2><p className="path">{data.runtime.path}</p>
        </> : <>
          <h2>{data ? "Set up your browser" : "Checking installed browsers…"}</h2>
          <p>{data?.runtimeError ?? "Workspaces open in an installed Google Chrome or Chromium browser."}</p>
        </>}
      </div>
      {data?.runtime && <button type="button" disabled={disabled} onClick={() => { setPath(data.runtime!.path); showRuntimeEditor(!runtimeEditor); }} aria-expanded={runtimeEditor}>Change browser</button>}
      {data && (!data.runtime || runtimeEditor) && <form className="runtime-path" onSubmit={pickRuntime}>
        <label>Chrome or Chromium path<input required value={path} onChange={event => setPath(event.target.value)} placeholder="Path to the browser application" /></label>
        <button type="button" disabled={disabled} onClick={async () => {
          try {
            const selected = await open({ title: "Select Chrome or Chromium", multiple: false });
            if (typeof selected === "string") setPath(selected);
          } catch (e) { setError(String(e)); }
        }}>Browse…</button><button disabled={disabled} type="submit">Use browser</button>
      </form>}
    </section>
    <p className="announcement error" role="alert">{!modal && error}</p>
    <p className="announcement" role="status">{notice}</p>
    {jobRunning && <div className="actions job-control"><a href="#batch-result">View {jobTitle.toLowerCase()} progress</a>
      <button onClick={() => { stopJob.current = true; }}>Cancel remaining</button></div>}
    {catalog && currentView && <>
      <nav className="tabs" aria-label="Catalog sections">
        {tabs.map(label => <button key={label} aria-current={tab === label ? "page" : undefined} onClick={() => navigate(label)}>
          {label}{label === "Reminders" && due ? ` (${due} due)` : ""}
        </button>)}
      </nav>
      {(tab === "Workspaces" || trash) && <section aria-labelledby="workspace-title">
        <div className="section-heading"><h2 id="workspace-title">{trash ? "Local trash" : "Your workspaces"}</h2>
          {!trash && <div className="actions">
            <button disabled={disabled} onClick={() => show({ type: "workspace", mode: "quick" })}>Quick workspace</button>
            <button disabled={disabled || !catalog.templates.length} onClick={() => show({ type: "workspace", mode: "template" })}>From template</button>
            <button className="primary" disabled={disabled} onClick={() => show({ type: "workspace", mode: "manual" })}>New workspace</button>
          </div>}
        </div>
        {!trash && <div className="panel filters">
          <label>Search <input ref={search} type="search" maxLength={200} value={currentView.search} onChange={e => updateView({ search: e.target.value })} placeholder="Name, tag, URL or notes" /></label>
          <label>Folder<select value={currentView.folder} onChange={e => updateView({ folder: e.target.value })}>
            <option value="">All folders</option><option value="unfiled">No folder</option>
            {catalog.folders.map(folder => <option key={folder.id} value={folder.id}>{folder.name}</option>)}
          </select></label>
          <label>Status<select value={currentView.status} onChange={e => updateView({ status: e.target.value })}>
            <option value="">All statuses</option>{Object.entries(statusLabels).map(([id, label]) => <option key={id} value={id}>{label}</option>)}
          </select></label>
          <label>Sort<select value={currentView.sort} onChange={e => updateView({ sort: e.target.value })}>
            <option value="">Name A–Z</option><option value="nameDesc">Name Z–A</option><option value="newest">Newest first</option><option value="status">Status</option>
          </select></label>
          <details><summary>Tags and columns</summary>
            <Checklist title="Filter by all selected tags" items={catalog.tags} value={currentView.tags} change={tags => updateView({ tags })} />
            <Checklist title="Visible columns" items={Object.entries(columnLabels).map(([id, name]) => ({ id, name }))}
              value={columns} change={value => updateView({ columns: value as Column[] })} />
          </details>
        </div>}
        <div className="panel bulk">
          <label className="inline"><input type="checkbox" aria-label="Select this page" checked={pageRows.length > 0 && pageRows.every(row => selection.includes(row.id))}
            onChange={e => setSelection(old => e.target.checked ? [...new Set([...old, ...pageRows.map(row => row.id)])] : old.filter(id => !pageRows.some(row => row.id === id)))} />Select this page</label>
          <span>{selectedRows.length} selected</span>
          <button disabled={!selectedRows.length} onClick={() => setSelection([])}>Clear selection</button>
          <select aria-label="Batch action" value={batchAction} onChange={e => { setBatchValue(""); setBatchAction(e.target.value as BatchAction); }}>
            {(trash ? ["restore", "purge"] as const : ["open", "close", "network", "folder", "tag", "trash"] as const).map(key => <option key={key} value={key}>{batchLabels[key]}</option>)}
          </select>
          {(batchAction === "tag" || batchAction === "folder") && <select aria-label={batchAction === "tag" ? "Batch tag" : "Batch folder"} value={batchValue} onChange={e => setBatchValue(e.target.value)}>
            <option value="">{batchAction === "tag" ? "Choose a tag" : "No folder"}</option>
            {(batchAction === "tag" ? catalog.tags : catalog.folders).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
          </select>}
          <button disabled={disabled || !selectedRows.length || (batchAction === "tag" && !batchValue)} onClick={() => beginWorkspaceBatch(selectedRows, batchAction)}>Run selected</button>
        </div>
        {pageRows.length ? <div className="table-wrap panel"><table>
          <thead><tr><th scope="col">Select</th><th scope="col">Workspace</th>{columns.map(column => <th scope="col" key={column}>{columnLabels[column]}</th>)}<th scope="col">Actions</th></tr></thead>
          <tbody>{pageRows.map(row => {
            const pending = disabled || row.status === "starting" || row.status === "closing";
            const running = row.status === "running" || row.status === "background";
            return <tr key={row.id}>
              <td><input type="checkbox" aria-label={`Select ${row.name}`} checked={selection.includes(row.id)} onChange={e => setSelection(old => e.target.checked ? [...old, row.id] : old.filter(id => id !== row.id))} /></td>
              <th scope="row"><span>{row.settings.icon ? `${row.settings.icon} ` : ""}{row.name}</span>
                {row.trashedAt !== null && <p>Trashed {localTime(row.trashedAt)}</p>}
                {!columns.includes("status") && row.detail && <p>{row.detail}</p>}
              </th>
              {columns.map(column => <td key={column}>{cell(row, column)}</td>)}
              <td>
                {trash ? <div className="actions"><button disabled={disabled} onClick={() => beginWorkspaceBatch([row], "restore")}>Restore</button>
                  <button disabled={disabled} onClick={() => beginWorkspaceBatch([row], "purge")}>Permanently delete</button></div> :
                  <><div className="actions" aria-label={`Actions for ${row.name}`}>
                    <button className="primary" disabled={pending || (!data?.runtime && !running) || row.status === "locked"} onClick={() => action(row, "open")}>Open</button>
                    <button disabled={pending || row.status !== "running"} onClick={() => action(row, "focus")}>Focus</button>
                    <button disabled={pending || !row.canClose} onClick={() => action(row, "close")}>Close</button>
                  </div>
                  <details className="row-tools"><summary>Manage</summary><div className="actions">
                    <button disabled={disabled} onClick={() => show({ type: "workspace", mode: "edit", workspace: row })}>Edit</button>
                    <button disabled={disabled} onClick={() => show({ type: "workspace", mode: "clone", workspace: row })}>Clone settings</button>
                    <button disabled={pending || running || row.status === "locked"} onClick={() => beginWorkspaceBatch([row], "trash")}>Trash</button>
                  </div>
                  {catalog.bookmarks.filter(b => row.settings.bookmarkIds.includes(b.id)).map(bookmark =>
                    <button key={bookmark.id} disabled={pending} onClick={() => command("open_bookmark", { id: row.id, bookmark: bookmark.id }, `Opened ${bookmark.name} in ${row.name}.`)}>{bookmark.name}</button>)}
                  </details></>}
              </td>
            </tr>;
          })}</tbody>
        </table></div> : <div className="panel empty"><h3>{trash ? "Trash is empty" : active.length ? "No matching workspaces" : "No workspaces yet"}</h3>
          <p>{trash ? "Trashed workspaces retain their browser data until permanently removed." : active.length ? "Try narrowing or clearing your filters." : "Create a workspace to keep browser sessions separate."}</p></div>}
        <div className="paging actions">
          <button disabled={pageIndex === 0} onClick={() => setPage(pageIndex - 1)}>Previous</button><span>Page {pageIndex + 1} of {totalPages} · {rows.length} rows</span>
          <button disabled={pageIndex + 1 === totalPages} onClick={() => setPage(pageIndex + 1)}>Next</button>
          <label className="inline">Rows per page<select value={pageSize} onChange={e => updateView({ pageSize: Number(e.target.value) })}>
            {[5, 10, 20, 50].map(n => <option key={n}>{n}</option>)}
          </select></label>
        </div>
      </section>}
      {tab === "Library" && <section className="library">
        {kinds.map(({ kind, label, plural }) => {
          const entries: LibraryItem[] = kind === "folder" ? catalog.folders : kind === "tag" ? catalog.tags : kind === "bookmark" ? catalog.bookmarks : catalog.templates;
          return <section className="panel" key={kind}>
            <div className="section-heading"><h2>{plural}</h2><button disabled={disabled} onClick={() => show({ type: "library", kind })}>Add {label.toLowerCase()}</button></div>
            {kind === "template" && <label>Quick workspace template<select value={catalog.defaultTemplate ?? ""} disabled={disabled}
              onChange={e => command("set_default_template", { id: e.target.value || null }, "Quick template updated.")}>
              <option value="">No default template</option>{catalog.templates.map(t => <option key={t.id} value={t.id}>{t.name}</option>)}
            </select></label>}
            {entries.length ? <ul className="rows">{entries.map(item => <li className="list-entry" key={item.id}>
              <div><h3>{item.name}</h3>{item.url || item.color ? <p className="path">{item.url ?? item.color}</p> : null}</div>
              <div className="actions"><button disabled={disabled} onClick={() => show({ type: "library", kind, item })}>Edit</button>
                <button disabled={disabled} onClick={() => show({ type: "confirm", title: `Remove ${item.name}?`,
                  detail: kind === "folder" ? "Workspaces are kept and become unfiled." : "Existing references to this entry will be cleared.",
                  confirm: () => command("remove_item", { kind, id: item.id }, `Removed ${item.name}.`) })}>Remove</button></div>
            </li>)}</ul> : <p>None yet.</p>}
          </section>;
        })}
      </section>}
      {tab === "Reminders" && <section>
        <div className="section-heading"><h2>Reminders</h2><button className="primary" disabled={disabled} onClick={() => show({ type: "task" })}>New reminder</button></div>
        {!catalog.tasks.length && <p className="panel empty">No reminders yet.</p>}
        <Tasks tasks={catalog.tasks} workspaces={workspaces} run={command} busy={disabled} />
      </section>}
      {tab === "Import" && <section>
        <div className="section-heading"><h2>Import workspaces</h2><button disabled={disabled} onClick={chooseImport}>Choose file…</button></div>
        <p>CSV, TSV, tab-delimited TXT or XLSX · up to 1,000 data rows and 2 MiB. Required column: name. Preview before importing.</p>
        {preview && <>
          <div className="bulk panel"><label className="inline"><input type="checkbox" disabled={disabled} aria-label="Select all valid rows"
            checked={importSelection.length > 0 && importSelection.length === preview.rows.filter(row => !row.error && !row.duplicate && !row.completed).length}
            onChange={e => setImportSelection(e.target.checked ? preview.rows.filter(row => !row.error && !row.duplicate && !row.completed).map(row => String(row.number)) : [])} />Select all valid rows</label>
            <button className="primary" disabled={disabled || !importSelection.length} onClick={importRows}>Import {importSelection.length} selected</button></div>
          <div className="panel table-wrap import-preview"><table><thead><tr><th>Select</th><th>Row</th><th>Name</th><th>Preview</th></tr></thead>
            <tbody>{preview.rows.map(row => <tr key={row.number}><td><input type="checkbox" aria-label={`Import row ${row.number}`} disabled={disabled || Boolean(row.error || row.duplicate || row.completed)}
              checked={importSelection.includes(String(row.number))} onChange={e => setImportSelection(old => e.target.checked ? [...old, String(row.number)] : old.filter(key => key !== String(row.number)))} /></td>
              <td>{row.number}</td><td>{row.name || "—"}</td><td>{row.completed ? "Already imported" : row.error ?? (row.duplicate ? "Duplicate name" : "Valid")}</td></tr>)}</tbody>
          </table></div>
        </>}
      </section>}
      {tab === "Extensions" && <section>
        <div className="section-heading"><h2>Extension catalog</h2><button className="primary" disabled={disabled} onClick={() => show({ type: "extension" })}>Add extension</button></div>
        <p>Assignments record intent, not browser install state. Install from the Web Store in each assigned workspace, or use Chrome’s <strong>Load unpacked</strong> flow if allowed. Silent install, CRX install and automatic removal are not available.</p>
        {catalog.extensions.map(extension => <article className="panel extension" key={extension.id}>
          <div className="section-heading"><h3>{extension.name}</h3><div className="actions">
            <button disabled={disabled} onClick={() => show({ type: "extension", extension })}>Update</button>
            <button disabled={disabled} onClick={() => show({ type: "confirm", title: `Remove ${extension.name} from the catalog?`, detail: "This removes assignments only. Disable or remove a browser-installed copy in chrome://extensions.", confirm: () => command("remove_item", { kind: "extension", id: extension.id }, "Catalog entry removed.") })}>Remove</button>
          </div></div>
          <p>{extension.source} · {extension.version || "Version unspecified"} · {extension.updateMode}</p>
          <p className="path">ID: {extension.browserId || `catalog ${extension.id} (Chrome assigns its own ID after loading)`}</p>
          <p>Permissions: {extension.permissions.join(", ") || "None listed"}</p>
          <p>Workspaces: {active.filter(row => row.settings.extensionIds.includes(extension.id)).map(row => row.name).join(", ") || "None"}</p>
          <p>Templates: {catalog.templates.filter(t => t.settings.extensionIds.includes(extension.id)).map(t => t.name).join(", ") || "None"}</p>
          {extension.unpackedPath && <><p>In the assigned browser open <code>chrome://extensions</code>, turn on Developer mode, click Load unpacked, and choose this directory. To update, remove the old copy and load the staged version. Disable or remove it in Chrome.</p>
            <label>Staged directory<input readOnly value={extension.unpackedPath} onFocus={e => e.target.select()} /></label></>}
          {extension.installUrl && <div className="actions">
            <select aria-label="Target workspace" value={extensionProfile} onChange={e => setExtensionProfile(e.target.value)}>
              <option value="">Select target workspace</option>{active.filter(row => row.settings.extensionIds.includes(extension.id)).map(row => <option key={row.id} value={row.id}>{row.name}</option>)}
            </select>
            <button disabled={disabled || !active.some(row => row.id === extensionProfile && row.settings.extensionIds.includes(extension.id))}
              onClick={() => command("open_extension_page", { id: extensionProfile, extension: extension.id }, "Store page requested in the selected workspace.")}>Open store listing</button>
          </div>}
        </article>)}
      </section>}
    </>}
    {outcomes.length > 0 && <section className="panel outcomes" aria-labelledby="batch-result">
      <div className="section-heading"><h2 id="batch-result">{jobTitle} results</h2>{jobRunning ?
        <button onClick={() => { stopJob.current = true; }}>Cancel remaining</button> :
        <button onClick={() => setOutcomes([])}>Dismiss results</button>}</div>
      <p role="status">{["Queued", "Running", "Succeeded", "Failed", "Cancelled"].map(state => `${outcomes.filter(row => row.state === state).length} ${state.toLowerCase()}`).join(" · ")}</p>
      <ol>{outcomes.map(row => <li key={row.key}>{row.name} — <strong>{row.state}</strong>{row.detail && `: ${row.detail}`}</li>)}</ol>
    </section>}
    <footer>Unsigned development build · Browser data stays local</footer>
    <dialog ref={dialog} onClose={() => setModal(undefined)} onCancel={e => { if (busy) e.preventDefault(); }}>
      {modal && catalog && <>
        {modal.type === "workspace" && <WorkspaceForm key={`${modal.mode}-${modal.workspace?.id}`} mode={modal.mode} workspace={modal.workspace} catalog={catalog} run={command} busy={busy} cancel={dismiss} />}
        {modal.type === "library" && <LibraryForm kind={modal.kind} item={modal.item} catalog={catalog} run={command} busy={busy} cancel={dismiss} />}
        {modal.type === "task" && <TaskForm workspaces={active} selected={selection.filter(id => active.some(w => w.id === id))} run={command} busy={busy} cancel={dismiss} />}
        {modal.type === "extension" && <ExtensionForm extension={modal.extension} run={command} busy={busy} cancel={dismiss} />}
        {modal.type === "account" && <form onSubmit={e => {
          e.preventDefault();
          const name = String(new FormData(e.currentTarget).get("name"));
          command("add_account", { name }, `Added ${name}. Select it to switch accounts.`, true);
        }}><h2>Add local account</h2><label>Name<input name="name" autoFocus required maxLength={100} /></label>
          <p>Each local account has its own catalog. Switching stops the outgoing account’s browsers. No sign-in or cloud account is created.</p>
          <div className="actions"><button type="button" onClick={dismiss}>Cancel</button><button className="primary" disabled={busy}>Add account</button></div></form>}
        {modal.type === "confirm" && <div className="confirm"><h2>{modal.title}</h2><p>{modal.detail}</p><div className="actions">
          <button autoFocus onClick={dismiss}>Cancel</button><button className="primary" onClick={() => { dismiss(); modal.confirm(); }}>Confirm</button>
        </div></div>}
        <p className="error" role="alert">{error}</p>
      </>}
    </dialog>
  </main>;
}

createRoot(document.getElementById("root")!).render(<App />);
