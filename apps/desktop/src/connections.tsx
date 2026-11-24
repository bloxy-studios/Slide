import { invoke } from "@tauri-apps/api/core";
import { useRef, useState } from "react";
import type { SavedConnection, Workspace } from "./catalog";
import type { Run } from "./forms";

export function Connections({ account, entries, workspaces, run, disabled, saved }: {
  account: string; entries: SavedConnection[]; workspaces: Workspace[]; run: Run; disabled: boolean; saved: () => void;
}) {
  const [name, setName] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [csv, setCsv] = useState("");
  const [query, setQuery] = useState("");
  const [draftStatus, setDraftStatus] = useState("");
  const [draftError, setDraftError] = useState("");
  const [saving, setSaving] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
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
  async function detect() {
    setPending(true); setDraftStatus("");
    try {
      setDraftStatus(await invoke<string>("test_connection", { account, endpoint, id: null, credentials: null }));
    } catch (error) {
      setDraftStatus(String(error));
    } finally { setPending(false); }
  }
  async function save() {
    if (saving || pending || disabled) return;
    setSaving(true); setDraftError("");
    try {
      await invoke("save_connection", { account, name, endpoint });
    } catch (error) {
      setDraftError(String(error));
      return;
    } finally {
      setSaving(false);
    }
    dialog.current?.close();
    saved();
  }
  return <section>
    <div className="section-heading"><h2>Saved proxies</h2><div className="actions">
      <input type="search" aria-label="Search proxies" placeholder="Name or address" value={query} onChange={e => setQuery(e.target.value)} />
      <button className="primary" disabled={disabled} onClick={() => { setName(""); setEndpoint(""); setDraftStatus(""); setDraftError(""); dialog.current?.showModal(); }}>+ Add proxy</button>
    </div></div>
    <p>Assign saved proxies when editing a profile. Required proxy launch has no Direct fallback and is available only with bundled Chromium on macOS arm64.</p>
    <dialog ref={dialog} onCancel={e => { if (pending || saving) e.preventDefault(); }}>
      <form onSubmit={event => {
        event.preventDefault(); void save();
      }}>
        <h2>Add proxy</h2>
        <label>Proxy type<input value="HTTP" readOnly /></label>
        <label>IP query channel<select defaultValue="DB-IP">
          <option disabled>IP2Location (not configured)</option><option disabled>MaxMind (not configured)</option>
          <option>DB-IP</option><option disabled>IP-API (HTTPS plan not configured)</option>
        </select></label>
        <p>DB-IP receives the browser’s public IP at startup. Proxy detection below checks the local QA fixture, not geolocation.</p>
        <label>Display name<input required disabled={saving} maxLength={80} value={name} onChange={event => setName(event.target.value)} /></label>
        <label>HTTP proxy endpoint<input required disabled={saving} value={endpoint} onChange={event => { setEndpoint(event.target.value); setDraftStatus(""); }} placeholder="http://127.0.0.1:4312" /></label>
        <p>Slide does not store proxy credentials. Chromium prompts for them when required.</p>
        <p role="status">{draftStatus}</p>
        {draftError && <p className="error" role="alert">{draftError}</p>}
        <div className="actions">
          <button type="button" disabled={pending || saving} onClick={() => dialog.current?.close()}>Cancel</button>
          <button type="button" disabled={!endpoint || pending || saving || disabled} onClick={() => void detect()}>{pending ? "Checking…" : "Proxy detection"}</button>
          <button className="primary" disabled={disabled || pending || saving}>{saving ? "Saving…" : "Confirm"}</button>
        </div>
      </form>
    </dialog>
    <div className="table-wrap panel"><table>
      <thead><tr><th scope="col">Proxy name</th><th scope="col">Server</th><th scope="col">IP query channel</th><th scope="col">Profiles</th><th scope="col">Fixture detection</th></tr></thead>
      <tbody>{entries.filter(entry => `${entry.name} ${entry.endpoint}`.toLowerCase().includes(query.trim().toLowerCase())).map(entry => <tr key={entry.id}>
        <th scope="row">{entry.name}</th><td>{entry.endpoint}</td><td>DB-IP</td>
        <td>{workspaces.filter(row => row.settings.connection.mode === "saved" && row.settings.connection.id === entry.id).map(row => row.name).join(", ") || "—"}</td>
        <td>{result(entry.id)}</td>
      </tr>)}</tbody>
    </table></div>
    <details><summary>Bulk import proxy endpoints (CSV)</summary>
      <form onSubmit={event => { event.preventDefault(); run("import_connections", { csv }, "HTTP proxy endpoints imported."); }}>
        <label>Paste CSV with name,endpoint header. Do not include credentials.
          <textarea required rows={4} value={csv} onChange={event => setCsv(event.target.value)} placeholder={"name,endpoint\nFixture,http://127.0.0.1:4312"} /></label>
        <button disabled={disabled}>Import endpoints</button>
      </form>
    </details>
    <form className="panel fixture-probe" onSubmit={event => { event.preventDefault(); void probe(); }}>
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
