import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { Workspace } from "./catalog";

type Bridge = {
  endpoint: string | null; token: string | null;
  audit: { created: number; operation: string; workspaceId: string | null; outcome: string }[];
};

export function Automation({ account, workspaces, refresh }: {
  account: string; workspaces: Workspace[]; refresh: () => Promise<unknown>;
}) {
  const [state, setState] = useState<Bridge>();
  const [token, setToken] = useState("");
  const [port, setPort] = useState("4330");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    invoke<Bridge>("automation_bridge", { action: "status", port: 0 })
      .then(status => { if (active) setState(status); })
      .catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, []);

  async function control(action: string) {
    setBusy(true); setError("");
    try {
      const status = await invoke<Bridge>("automation_bridge", { action, port: action === "enable" ? Number(port) : 0 });
      setState(status);
      if (status.token) setToken(status.token);
      if (action === "disable") setToken("");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }

  async function toggle(workspace: Workspace) {
    setBusy(true); setError("");
    try {
      await invoke("set_automation_cdp", { account, id: workspace.id, enabled: !workspace.automationCdp });
      await refresh();
      await control("status");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }

  return <section aria-labelledby="automation-title">
    <h2 id="automation-title">Local automation</h2>
    <p>The bridge listens only on 127.0.0.1. Its token lets a local tool control workspaces in the active account. Only share it with tools you trust. The bridge starts disabled after each app restart.</p>
    <p role="status">{state?.endpoint ? `Bridge enabled at ${state.endpoint}` : "Bridge disabled."}</p>
    {error && <p role="alert" className="error">{error}</p>}
    <div className="actions">
      {state?.endpoint
        ? <button disabled={busy} onClick={() => void control("disable")}>Disable bridge</button>
        : <><label>Loopback port<input inputMode="numeric" value={port} onChange={e => setPort(e.target.value)} /></label>
          <button disabled={busy || !/^\d{1,5}$/.test(port) || Number(port) > 65535} onClick={() => void control("enable")}>Enable bridge</button></>}
      <button disabled={busy || !state?.endpoint} onClick={() => void control("rotate")}>Rotate bridge token</button>
      <button disabled={busy} onClick={() => void control("status")}>Refresh audit</button>
    </div>
    {token && <p>Copy this token now; it is not shown again. <input aria-label="Bridge token" readOnly value={token} />
      <button onClick={() => setToken("")}>Hide token</button></p>}
    <h3>Per-workspace Chromium debugging</h3>
    <p>Off by default. Opting in allows any local process to use the browser's CDP port during a bridge-launched session. Use a dedicated QA workspace with no sensitive data. Close that browser to stop debugging; disabling the bridge alone does not close it.</p>
    {workspaces.filter(workspace => !workspace.trashedAt).map(workspace => <p key={workspace.id}>
      <label><input type="checkbox" checked={workspace.automationCdp} disabled={busy || !["closed", "crashed"].includes(workspace.status)}
        onChange={() => void toggle(workspace)} /> Enable CDP for {workspace.name}</label>
    </p>)}
    <h3>Recent local activity</h3>
    <ul>{state?.audit.map((entry, i) => <li key={i}>{new Date(entry.created * 1000).toLocaleString()} · {entry.operation}
      {entry.workspaceId ? ` · ${entry.workspaceId}` : ""} · {entry.outcome}</li>)}</ul>
  </section>;
}
