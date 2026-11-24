import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type { SavedConnection, Workspace } from "./catalog";
import type { Run } from "./forms";

export function Connections({ account, entries, workspaces, run, disabled }: {
  account: string; entries: SavedConnection[]; workspaces: Workspace[]; run: Run; disabled: boolean;
}) {
  const [name, setName] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [csv, setCsv] = useState("");
  const [target, setTarget] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);
  const [results, setResults] = useState<Record<string, { label: string; time: string }>>({});
  const key = target || "direct";
  async function probe() {
    setPending(true);
    try {
      const label = await invoke<string>("test_connection", {
        account, id: target || null, credentials: target && (username || password) ? { username, password } : null,
      });
      setResults(previous => ({ ...previous, [key]: { label, time: new Date().toLocaleString() } }));
    } catch (error) {
      setResults(previous => ({ ...previous, [key]: { label: String(error), time: new Date().toLocaleString() } }));
    } finally {
      setPassword(""); setPending(false);
    }
  }
  function result(id: string) {
    return results[id] ? <>{results[id].label} <small>· {results[id].time}</small></> : "Not checked in this panel.";
  }
  return <section>
    <h2>Connections</h2>
    <p>Assign a saved HTTP proxy or a workspace-only endpoint when editing a workspace. Required browser routes have no Direct fallback. Proxy launch is available with the bundled runtime on macOS arm64; this is not a VPN or an operating-system network guarantee.</p>
    <form className="form-grid" onSubmit={event => {
      event.preventDefault(); run("save_connection", { name, endpoint }, "HTTP proxy saved.");
    }}>
      <label>Display name<input required maxLength={80} value={name} onChange={event => setName(event.target.value)} /></label>
      <label>HTTP proxy endpoint<input required value={endpoint} onChange={event => setEndpoint(event.target.value)} placeholder="http://127.0.0.1:4312" /></label>
      <button disabled={disabled}>Save HTTP proxy</button>
    </form>
    <details><summary>Bulk import proxy endpoints (CSV)</summary>
      <form onSubmit={event => { event.preventDefault(); run("import_connections", { csv }, "HTTP proxy endpoints imported."); }}>
        <label>Paste CSV with name,endpoint header. Do not include credentials.
          <textarea required rows={4} value={csv} onChange={event => setCsv(event.target.value)} placeholder={"name,endpoint\nFixture,http://127.0.0.1:4312"} /></label>
        <button disabled={disabled}>Import endpoints</button>
      </form>
    </details>
    <article><h3>Direct · no proxy</h3><p>{result("direct")}</p></article>
    {entries.map(entry => <article key={entry.id}>
      <h3>{entry.name} · HTTP proxy</h3>
      <p>{entry.endpoint} · Credentials not stored</p>
      <p>Workspaces: {workspaces.filter(row => row.settings.connection.mode === "saved" && row.settings.connection.id === entry.id).map(row => row.name).join(", ") || "None"}</p>
      <p>{result(entry.id)}</p>
    </article>)}
    <form onSubmit={event => { event.preventDefault(); void probe(); }}>
      <h3>Test controlled fixture route</h3>
      <p>These checks target the loopback fixture at 127.0.0.1:4320 from the manager, not from the browser. Use only synthetic credentials; browser authentication happens in Chromium.</p>
      <label>Test connection<select value={target} onChange={event => { setTarget(event.target.value); setUsername(""); setPassword(""); }}>
        <option value="">Direct</option>{entries.map(entry => <option key={entry.id} value={entry.id}>{entry.name}</option>)}
      </select></label>
      {target && <div className="form-grid">
        <label>Test username (optional)<input autoComplete="off" value={username} onChange={event => setUsername(event.target.value)} /></label>
        <label>Test password (optional)<input type="password" autoComplete="off" value={password} onChange={event => setPassword(event.target.value)} /></label>
      </div>}
      <button disabled={disabled || pending}>{pending ? "Testing…" : "Test fixture route"}</button>
      <p role="status">{result(key)}</p>
    </form>
  </section>;
}
