import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import type { Catalog, Extension, Label, Reminder, Settings, Workspace } from "./catalog";
import { emptySettings, localTime } from "./catalog";

export type Run = (command: string, args: Record<string, unknown>, success: string, close?: boolean) => void;
export type LibraryKind = "folder" | "tag" | "bookmark" | "template";
export const kinds: { kind: LibraryKind; label: string; plural: string }[] = [
  { kind: "folder", label: "Folder", plural: "Folders" },
  { kind: "tag", label: "Tag", plural: "Tags" },
  { kind: "bookmark", label: "Bookmark", plural: "Bookmarks" },
  { kind: "template", label: "Template", plural: "Templates" },
];
export type LibraryItem = Label & { color?: string; url?: string; settings?: Settings };

export function Checklist({ title, items, value, change }: {
  title: string; items: Label[]; value: string[]; change: (value: string[]) => void;
}) {
  return <fieldset><legend>{title}</legend>
    {items.length ? <div className="checks">{items.map(item =>
      <label key={item.id}><input type="checkbox" checked={value.includes(item.id)}
        onChange={e => change(e.target.checked ? [...value, item.id] : value.filter(id => id !== item.id))} />{item.name}</label>,
    )}</div> : <p>None available.</p>}
  </fieldset>;
}

function SettingsFields({ catalog, value, change }: { catalog: Catalog; value: Settings; change: (s: Settings) => void }) {
  const [urlText, setUrlText] = useState(value.launchUrls.join("\n"));
  const set = (changes: Partial<Settings>) => change({ ...value, ...changes });
  return <>
    <details className="profile-section" open><summary>Basic information</summary>
    <div className="form-grid">
      <label>Icon (emoji or text)<input value={value.icon} maxLength={12} onChange={e => set({ icon: e.target.value })} placeholder="e.g. 🌱" /></label>
      <label>Folder<select value={value.folderId ?? ""} onChange={e => set({ folderId: e.target.value || null })}>
        <option value="">No folder</option>{catalog.folders.map(f => <option key={f.id} value={f.id}>{f.name}</option>)}
      </select></label>
    </div>
    <label>Notes<textarea rows={2} value={value.notes} maxLength={8000} onChange={e => set({ notes: e.target.value })} /></label>
    </details>
    <details className="profile-section"><summary>Browser startup</summary>
    <label>Launch URLs (one HTTP(S) URL per line)<textarea rows={2} value={urlText} placeholder="http://127.0.0.1:4310"
      onChange={e => { setUrlText(e.target.value); set({ launchUrls: e.target.value.split("\n").filter(url => url.trim()) }); }} /></label>
    </details>
    <details className="profile-section"><summary>Proxy</summary>
    <label>Connection<select value={value.connection.mode === "saved" ? value.connection.id : value.connection.mode}
      onChange={e => {
        const mode = e.target.value;
        set({ connection: mode === "system" || mode === "direct" ? { mode } : mode === "workspace"
          ? { mode, endpoint: "" } : { mode: "saved", id: mode } });
      }}>
      <option value="system">System default</option><option value="direct">Direct (no proxy)</option><option value="workspace">Workspace-only HTTP proxy</option>
      {catalog.connections.map(connection => <option key={connection.id} value={connection.id}>{connection.name} (required)</option>)}
    </select></label>
    {value.connection.mode === "workspace" && <label>Required HTTP proxy endpoint<input required
      onInvalid={event => {
        const field = event.currentTarget;
        const section = field.closest("details");
        if (section && !section.open) {
          section.open = true;
          field.focus();
        }
      }}
      value={value.connection.endpoint} onChange={e => set({ connection: { mode: "workspace", endpoint: e.target.value } })}
      placeholder="http://127.0.0.1:4312" /></label>}
    </details>
    <details className="profile-section"><summary>Advanced settings</summary>
    <Checklist title="Tags" items={catalog.tags} value={value.tagIds} change={tags => set({ tagIds: tags, pinnedTagId: tags.includes(value.pinnedTagId ?? "") ? value.pinnedTagId : null })} />
    <label>Pinned tag<select value={value.pinnedTagId ?? ""} onChange={e => set({ pinnedTagId: e.target.value || null })}>
      <option value="">None</option>{catalog.tags.filter(t => value.tagIds.includes(t.id)).map(t => <option key={t.id} value={t.id}>{t.name}</option>)}
    </select></label>
    <Checklist title="Reusable bookmarks" items={catalog.bookmarks} value={value.bookmarkIds} change={bookmarkIds => set({ bookmarkIds })} />
    <Checklist title="Extension assignments (approve installation separately)" items={catalog.extensions} value={value.extensionIds} change={extensionIds => set({ extensionIds })} />
    </details>
  </>;
}

export function WorkspaceForm({ catalog, mode: initialMode, workspace, run, busy, cancel, bulk, browser }: {
  catalog: Catalog; mode: "manual" | "quick" | "template" | "clone" | "edit"; workspace?: Workspace; browser?: string;
  run: Run; busy: boolean; cancel: () => void; bulk: () => void;
}) {
  const [mode, setMode] = useState(initialMode);
  const [name, setName] = useState(mode === "clone" ? `${workspace?.name} copy`.slice(0, 100) : workspace?.name ?? "");
  const [settings, setSettings] = useState(workspace?.settings ?? emptySettings);
  const [template, setTemplate] = useState(catalog.defaultTemplate ?? catalog.templates[0]?.id ?? "");
  return <form onSubmit={e => {
    e.preventDefault();
    if (mode === "edit") run("edit_workspace", { name, id: workspace?.id, settings }, `Updated ${name}.`, true);
    else {
      const creation = mode === "manual" ? { mode, settings } : mode === "template" ? { mode, id: template } :
        mode === "clone" ? { mode, id: workspace?.id } : { mode };
      run("create_workspace", { name, creation }, `Created ${name}.`, true);
    }
  }}>
    <h2>{({ manual: "New profile", quick: "New profile", template: "Create from template", clone: "Clone settings", edit: "Edit workspace" })[mode]}</h2>
    {(mode === "manual" || mode === "quick") && <nav className="create-tabs" aria-label="Create profile">
      <button type="button" aria-pressed={mode === "quick"} onClick={() => setMode("quick")}>Quick create</button>
      <button type="button" aria-pressed={mode === "manual"} onClick={() => setMode("manual")}>Advanced create</button>
      <button type="button" onClick={bulk}>Bulk import</button>
    </nav>}
    <div className="profile-layout"><div className="profile-fields">
    <label>Name<input required maxLength={100} value={name} autoFocus onChange={e => setName(e.target.value)} /></label>
    {mode === "manual" || mode === "edit" ? <SettingsFields catalog={catalog} value={settings} change={setSettings} /> : null}
    {mode === "template" && <label>Template<select required value={template} onChange={e => setTemplate(e.target.value)}>
      {catalog.templates.map(t => <option key={t.id} value={t.id}>{t.name}</option>)}
    </select></label>}
    {mode === "quick" && <p>{catalog.defaultTemplate ? `Using default template: ${catalog.templates.find(t => t.id === catalog.defaultTemplate)?.name}.` : "Using a clean browser and blank settings."}</p>}
    {mode === "clone" && <p>Copies only manager settings. Cookies, passwords and other browser data are not copied.</p>}
    </div><aside className="profile-overview">
      <h3>Browser profile overview</h3>
      <dl>
        <dt>Profile</dt><dd>{name || "Untitled profile"}</dd>
        <dt>Browser</dt><dd>{browser || "Not configured"}</dd>
        <dt>Session</dt><dd>Isolated local browser profile</dd>
        <dt>Connection</dt><dd>{mode === "manual" || mode === "edit" ? ({ system: "System default", direct: "Direct", workspace: "Workspace-only HTTP proxy", saved: "Saved HTTP proxy" })[settings.connection.mode] : "From selected creation settings"}</dd>
        <dt>Fingerprint</dt><dd>Native browser values</dd>
      </dl>
    </aside></div>
    <div className="actions"><button type="button" onClick={cancel}>Cancel</button><button className="primary" disabled={busy}>{mode === "edit" ? "Save workspace" : "Create workspace"}</button></div>
  </form>;
}

export function LibraryForm({ catalog, kind, item, run, busy, cancel }: {
  catalog: Catalog; kind: LibraryKind; item?: LibraryItem; run: Run; busy: boolean; cancel: () => void;
}) {
  const [name, setName] = useState(item?.name ?? "");
  const [url, setUrl] = useState(item?.url ?? "");
  const [color, setColor] = useState(item?.color ?? "#17645a");
  const [settings, setSettings] = useState(item?.settings ?? emptySettings);
  return <form onSubmit={e => {
    e.preventDefault();
    const entry = kind === "tag" ? { kind, name, color } : kind === "bookmark" ? { kind, name, url } : kind === "template" ? { kind, name, settings } : { kind, name };
    run("save_item", { id: item?.id ?? null, entry }, `Saved ${name}.`, true);
  }}>
    <h2>{item ? "Edit" : "Add"} {kind}</h2>
    <label>Name<input required autoFocus value={name} maxLength={100} onChange={e => setName(e.target.value)} /></label>
    {kind === "tag" && <label>Color<input type="color" value={color} onChange={e => setColor(e.target.value)} /></label>}
    {kind === "bookmark" && <label>URL<input type="url" required value={url} onChange={e => setUrl(e.target.value)} /></label>}
    {kind === "template" && <SettingsFields catalog={catalog} value={settings} change={setSettings} />}
    <div className="actions"><button type="button" onClick={cancel}>Cancel</button><button className="primary" disabled={busy}>Save {kind}</button></div>
  </form>;
}

export function TaskForm({ workspaces, selected, run, busy, cancel }: {
  workspaces: Workspace[]; selected: string[]; run: Run; busy: boolean; cancel: () => void;
}) {
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [date, setDate] = useState("");
  const [ids, setIds] = useState(selected);
  return <form onSubmit={e => {
    e.preventDefault();
    run("create_task", { draft: { title, description, dueAt: Math.floor(new Date(date).getTime() / 1000), workspaceIds: ids } }, "Reminder saved.", true);
  }}>
    <h2>New reminder</h2>
    <label>Title<input required autoFocus value={title} maxLength={100} onChange={e => setTitle(e.target.value)} /></label>
    <label>Details<textarea rows={2} value={description} maxLength={8000} onChange={e => setDescription(e.target.value)} /></label>
    <label>Due (local time)<input required type="datetime-local" value={date} onChange={e => setDate(e.target.value)} /></label>
    <Checklist title="Linked workspaces" items={workspaces} value={ids} change={setIds} />
    <div className="actions"><button type="button" onClick={cancel}>Cancel</button><button className="primary" disabled={busy}>Save reminder</button></div>
  </form>;
}

export function Tasks({ tasks, workspaces, run, busy }: { tasks: Reminder[]; workspaces: Workspace[]; run: Run; busy: boolean }) {
  return <ul className="rows">{tasks.map(task => <li className="panel list-entry" key={task.id}>
    <div><h3>{task.title} {task.firedAt && !task.completedAt ? <span className="badge locked">Due</span> : task.completedAt ? <span className="badge">Completed</span> : null}</h3>
      <p>{localTime(task.dueAt)} (local) · {task.workspaceIds.map(id => workspaces.find(row => row.id === id)?.name).filter(Boolean).join(", ") || "No linked workspaces"}</p>
      {task.description && <p className="prewrap">{task.description}</p>}
      {task.firedAt && <p>Marked due {localTime(task.firedAt)}.</p>}
    </div>
    {!task.completedAt && <div className="actions">
      <button disabled={busy} onClick={() => run("finish_task", { id: task.id, snooze: true }, "Reminder postponed by 10 minutes.")}>Snooze 10 min</button>
      <button disabled={busy} onClick={() => run("finish_task", { id: task.id, snooze: false }, "Reminder completed.")}>Complete</button>
    </div>}
  </li>)}</ul>;
}

export function ExtensionForm({ extension, run, busy, cancel }: {
  extension?: Extension; run: Run; busy: boolean; cancel: () => void;
}) {
  const [source, setSource] = useState(extension?.source ?? "fixture");
  const [name, setName] = useState(extension?.name ?? "");
  const [browserId, setBrowserId] = useState(extension?.browserId ?? "");
  const [version, setVersion] = useState(extension?.version ?? "");
  const [permissions, setPermissions] = useState(extension?.permissions.join("\n") ?? "");
  const [path, setPath] = useState("");
  const [error, setError] = useState("");
  return <form onSubmit={e => {
    e.preventDefault();
    const id = extension?.id ?? null;
    if (source === "store") {
      run("save_store_extension", { id, name, browserId, version, permissions: permissions.split("\n").filter(p => p.trim()) }, "Web Store listing saved.", true);
    } else {
      run("stage_extension", { id, source, path: path || null, fixtureUpdate: Boolean(extension) }, "Package staged. Approve or reload it in the browser.", true);
    }
  }}>
    <h2>{extension ? "Update extension entry" : "Add extension"}</h2>
    <label>Source<select disabled={Boolean(extension)} value={source} onChange={e => setSource(e.target.value as Extension["source"])}>
      <option value="fixture">Slide fixture badge</option><option value="zip">Custom ZIP (unpack only)</option><option value="store">Chrome Web Store</option>
    </select></label>
    {source === "store" && <>
      <label>Listing name<input required value={name} onChange={e => setName(e.target.value)} /></label>
      <label>Chrome extension ID<input required pattern="[a-p]{32}" value={browserId} onChange={e => setBrowserId(e.target.value)} /></label>
      <label>Version (optional, as listed)<input value={version} onChange={e => setVersion(e.target.value)} /></label>
      <label>Permissions (optional, as listed, one per line)<textarea value={permissions} onChange={e => setPermissions(e.target.value)} /></label>
    </>}
    {source === "zip" && <div className="actions">
      <button type="button" onClick={async () => {
        try {
          const file = await open({ multiple: false, filters: [{ name: "Extension ZIP", extensions: ["zip"] }] });
          if (typeof file === "string") setPath(file);
        } catch (e) { setError(String(e)); }
      }}>Choose ZIP…</button><span className="path">{path}</span>
    </div>}
    {source === "fixture" && <p>{extension ? "Stage the fixture badge v1.0.1." : "Stage a harmless badge extension that displays only on the local test site."}</p>}
    <p className="error" role="alert">{error}</p>
    <div className="actions"><button type="button" onClick={cancel}>Cancel</button>
      <button className="primary" disabled={busy || (source === "zip" && !path)}>{source === "store" ? "Save listing" : "Stage package"}</button></div>
  </form>;
}
