import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { createRoot } from "react-dom/client";
import "./style.css";

type Runtime = { path: string; brand: string; version: string; architecture: string };
type Operation = "open" | "focus" | "close";
type Status = "closed" | "running" | "background" | "starting" | "closing" | "locked" | "crashed";
type Workspace = { id: string; name: string; startUrl: string | null; status: Status; detail: string | null };
type Snapshot = { runtime: Runtime | null; runtimeError: string | null; workspaces: Workspace[] };

const statusLabels: Record<Status, string> = {
  closed: "Closed", running: "Running", background: "No open window", starting: "Opening…",
  closing: "Closing…", locked: "Profile in use", crashed: "Exited unexpectedly",
};

function App() {
  const [data, setData] = useState<Snapshot>();
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(new Set<string>());
  const [runtimeEditor, showRuntimeEditor] = useState(false);
  const [name, setName] = useState("");
  const [startUrl, setStartUrl] = useState("");
  const dialog = useRef<HTMLDialogElement>(null);
  const newButton = useRef<HTMLButtonElement>(null);
  const queue = useRef(new Set<string>());

  async function refresh() {
    setData(await invoke<Snapshot>("snapshot"));
  }

  useEffect(() => {
    let reading = false;
    const poll = async () => {
      if (reading) return;
      reading = true;
      try { await refresh(); }
      catch (e) { setError(String(e)); }
      finally { reading = false; }
    };
    void poll();
    const timer = setInterval(poll, 1200);
    return () => clearInterval(timer);
  }, []);

  async function run(key: string, action: () => Promise<void>, success: string) {
    if (queue.current.has(key)) return;
    queue.current.add(key);
    setBusy(new Set(queue.current));
    setError("");
    setNotice("");
    try {
      await action();
      await refresh();
      setNotice(success);
    } catch (e) {
      setError(String(e));
    } finally {
      queue.current.delete(key);
      setBusy(new Set(queue.current));
    }
  }

  function create(event: FormEvent) {
    event.preventDefault();
    void run("create", async () => {
      await invoke("create_workspace", { name, startUrl });
      dialog.current?.close();
    }, `Created ${name.trim()}.`);
  }

  function action(workspace: Workspace, operation: Operation) {
    void run(workspace.id, async () => {
      try { await invoke("workspace_action", { id: workspace.id, operation }); }
      catch (e) { throw `${workspace.name}: ${e}`; }
    }, `${workspace.name}: ${operation} requested.`);
  }

  function pickRuntime(event: FormEvent) {
    event.preventDefault();
    void run("runtime", async () => {
      await invoke("choose_runtime", { path });
      showRuntimeEditor(false);
    }, "Browser runtime selected.");
  }

  return <main>
    <header className="masthead">
      <div><div className="eyebrow">SLIDE / DESKTOP</div><h1>Workspaces</h1><p>Separate browser sessions, on this device.</p></div>
      <span className="tag">Local only</span>
    </header>

    <section className="panel runtime" aria-labelledby="runtime-title">
      <div>
        <div className="eyebrow" id="runtime-title">BROWSER RUNTIME</div>
        {data?.runtime ? <>
          <h2>{data.runtime.brand} <span className="version">{data.runtime.version} · {data.runtime.architecture}</span></h2>
          <p className="path">{data.runtime.path}</p>
        </> : <>
          <h2>{data ? "Set up your browser" : "Checking installed browsers…"}</h2>
          <p>{data?.runtimeError ?? "Workspaces open in an installed Google Chrome or Chromium browser."}</p>
        </>}
      </div>
      {data?.runtime && <button type="button" onClick={() => { setPath(data.runtime!.path); showRuntimeEditor(!runtimeEditor); }} aria-expanded={runtimeEditor}>Change browser</button>}
      {data && (!data.runtime || runtimeEditor) && <form className="runtime-path" onSubmit={pickRuntime}>
        <label>Chrome or Chromium path<input required value={path} onChange={event => setPath(event.target.value)} placeholder="Path to the browser application" /></label>
        <button type="button" onClick={async () => {
          try {
            const selected = await open({ title: "Select Chrome or Chromium", multiple: false });
            if (typeof selected === "string") setPath(selected);
          } catch (e) { setError(String(e)); }
        }}>Browse…</button>
        <button disabled={busy.has("runtime")} type="submit">Use browser</button>
      </form>}
    </section>

    <section className="catalog" aria-labelledby="workspace-title">
      <div className="section-heading">
        <h2 id="workspace-title">Your workspaces</h2>
        <button className="primary" ref={newButton} disabled={!data} onClick={() => {
          setName(""); setStartUrl(""); setError(""); dialog.current?.showModal();
        }}>New workspace</button>
      </div>
      {data && !data.workspaces.length && <div className="panel empty">
        <h3>No workspaces yet</h3>
        <p>Create a workspace to open a separate browser window with its own cookies and site storage.</p>
      </div>}
      <ul className="rows">
        {data?.workspaces.map(workspace => {
          const pending = busy.has(workspace.id) || workspace.status === "starting" || workspace.status === "closing";
          const active = workspace.status === "running" || workspace.status === "background";
          return <li key={workspace.id} className="panel row">
            <div className="identity"><h3>{workspace.name}</h3><p>{workspace.startUrl ?? "Browser default"}</p></div>
            <div className="status"><span className={`badge ${workspace.status}`}>{statusLabels[workspace.status]}</span>{workspace.detail && <p>{workspace.detail}</p>}</div>
            <div className="actions" aria-label={`Actions for ${workspace.name}`}>
              <button className="primary" disabled={pending || (!data.runtime && !active) || workspace.status === "locked"} onClick={() => action(workspace, "open")}>Open</button>
              <button disabled={pending || workspace.status !== "running"} onClick={() => action(workspace, "focus")}>Focus</button>
              <button disabled={pending || !active} onClick={() => action(workspace, "close")}>Close</button>
            </div>
          </li>;
        })}
      </ul>
    </section>
    <footer>Unsigned development build · Browser data stays local</footer>
    <p className="announcement error" role="alert">{error}</p>
    <p className="announcement" role="status">{notice}</p>

    <dialog ref={dialog} onClose={() => { newButton.current?.focus(); }}>
      <form onSubmit={create}>
        <h2>New workspace</h2>
        <label>Name<input value={name} required maxLength={100} onChange={event => setName(event.target.value)} placeholder="e.g. Folve Creator" autoFocus /></label>
        <label>Start URL (optional)<input type="url" value={startUrl} onChange={event => setStartUrl(event.target.value)} placeholder="http://127.0.0.1:4310" /></label>
        <p>Leave this empty to use the browser’s normal first-run or new-tab page.</p>
        <p className="error" role="alert">{error}</p>
        <div className="actions"><button type="button" onClick={() => dialog.current?.close()}>Cancel</button><button className="primary" disabled={busy.has("create")}>Create workspace</button></div>
      </form>
    </dialog>
  </main>;
}

createRoot(document.getElementById("root")!).render(<App />);
