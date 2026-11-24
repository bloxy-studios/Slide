import type { Snapshot } from "./catalog";

const categories = [
  ["Cookies and session data", "Browser profile; the QA fixture only exchanges its synthetic role cookie."],
  ["Passwords", "Browser and OS credential store; Slide does not read or export it."],
  ["Extensions", "Browser profile and approved workspace CRX packages; state is not portable."],
  ["Local storage and IndexedDB", "Browser profile; the fixture displays only its own markers."],
  ["History", "Browser profile; no Slide import or export."],
  ["Bookmarks", "Native bookmarks stay in the profile; Slide shortcuts are separate catalog metadata."],
  ["Service workers and cache", "Browser profile; no category sync or export."],
  ["Addresses and payments", "Not managed by Slide."],
] as const;

export function Diagnostics({ runtime, runtimeError, platform, requiredHttpProxyAvailable }: Snapshot) {
  return <section>
    <h2>Runtime diagnostics</h2>
    <p>Read-only capabilities for the default browser on this device. Existing workspaces can remain pinned to a different browser.</p>
    <dl>
      <dt>Platform</dt><dd>{platform}</dd>
      <dt>Runtime</dt><dd>{runtime ? `${runtime.brand} ${runtime.version} · ${runtime.architecture} · ${runtime.bundled ? "bundled" : "installed"}` : runtimeError ?? "Unavailable"}</dd>
      {runtime && <><dt>Executable</dt><dd className="path">{runtime.path}</dd></>}
      <dt>Workspace networking</dt><dd>Direct · {requiredHttpProxyAvailable ? "required HTTP proxy available" : "required proxy launch unsupported for this runtime/platform"}</dd>
      <dt>Other transports</dt><dd>HTTPS and SOCKS proxy transport, SSH, WireGuard, and AmneziaWG are blocked pending route, DNS and failure verification.</dd>
      <dt>Fixture presets</dt><dd>First-party simulation only; ordinary browsing keeps its standard device reporting.</dd>
    </dl>
    <h3>Browser storage categories</h3>
    <p>These are capabilities, not sync controls. No browser category is verified portable.</p>
    <div className="table-scroll"><table>
      <thead><tr><th>Category</th><th>Capability</th><th>Scope</th></tr></thead>
      <tbody>{categories.map(([name, scope]) => <tr key={name}>
        <th scope="row">{name}</th><td>{runtime && name !== "Addresses and payments" ? "Local only" : "Unsupported"}</td><td>{scope}</td>
      </tr>)}</tbody>
    </table></div>
  </section>;
}
