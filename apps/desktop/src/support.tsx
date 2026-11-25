import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

type SupportInfo = { report: string; log_location: string | null };
type Report = {
  appVersion: string | null;
  os: string;
  osVersion: string | null;
  architecture: string;
  defaultBrowser: { brand: string; version: string | null; bundled: boolean } | null;
};

export function Support() {
  const [info, setInfo] = useState<SupportInfo>();
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);

  async function refresh() {
    setError("");
    try { setInfo(await invoke("support_info")); }
    catch (reason) { setError(String(reason)); }
  }

  useEffect(() => { void refresh(); }, []);
  const report: Report | undefined = info ? JSON.parse(info.report) : undefined;
  const browser = report?.defaultBrowser;
  const unavailable = "Unavailable";

  return <section className="panel">
    <h2>Versions and support</h2>
    <dl>
      <dt>Slide Workspaces</dt><dd>{report?.appVersion ?? unavailable}</dd>
      <dt>Default browser</dt><dd>{browser ? `${browser.brand} ${browser.version ?? "unknown version"} · ${browser.bundled ? "bundled" : "installed"}` : unavailable}</dd>
      <dt>Operating system</dt><dd>{report ? `${report.os === "macos" ? "macOS" : report.os === "windows" ? "Windows" : report.os} ${report.osVersion ?? "unknown version"} · ${report.architecture}` : unavailable}</dd>
      <dt>Updates</dt><dd>Manual installation. Automatic update checking is not configured.</dd>
      <dt>Startup log</dt><dd className="path">{info?.log_location ?? unavailable}</dd>
    </dl>
    <p>The default browser may differ from a workspace’s recorded browser. The startup log records only whether the local catalog opened.</p>
    <div className="actions">
      <button onClick={() => void refresh()}>Refresh support info</button>
      <button onClick={() => void invoke("restart_manager").catch(reason => setError(String(reason)))}>Restart manager</button>
      <button onClick={() => void invoke("open_support_docs").catch(reason => setError(String(reason)))}>Open support docs ↗</button>
    </div>
    <h3>Diagnostics export</h3>
    <p>Review the report before saving it. It contains only the versions and platform summary below. It excludes log contents, file paths, workspace data, tokens, credentials and browser storage. Nothing is uploaded.</p>
    {info && <pre aria-label="Diagnostics preview">{info.report}</pre>}
    <button className="primary" disabled={!info || saving} onClick={async () => {
      setSaving(true);
      setError("");
      setNotice("");
      try {
        if (await invoke<boolean>("export_support")) setNotice("Diagnostics saved.");
      } catch (reason) { setError(String(reason)); }
      finally { setSaving(false); }
    }}>Export diagnostics…</button>
    {error && <p role="alert" className="error">{error}</p>}
    {notice && <p role="status">{notice}</p>}
  </section>;
}
