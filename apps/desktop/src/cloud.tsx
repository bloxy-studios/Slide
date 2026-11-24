import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { SYNC_API } from "../../../packages/schemas/sync";
import type { Envelope, Lease, RemoteWorkspace, Revision, TeamRole } from "../../../packages/schemas/sync";
import type { Workspace } from "./catalog";

type Account = { id: string; email: string };
type Session = { token: string; account: Account };
type Team = { id: string; name: string; role: TeamRole };
type Invitation = { id: string; name: string; role: TeamRole };
type LeaseState = { lease: Lease | null; heldByThisDevice?: boolean; generation: number };
type Sealed = { envelope: Envelope; sha256: string };
type Upload = Sealed & { path: string; requestId: string; fence: string; generation: number };
const endpoint = "http://127.0.0.1:4321" + SYNC_API;

function identifier() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 64;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = Array.from(bytes, n => n.toString(16).padStart(2, "0")).join("");
  return [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join("-");
}

async function api<T>(path: string, token?: string, method = "GET", body?: object): Promise<T> {
  let response: Response;
  try {
    response = await fetch(endpoint + path, {
      method, cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(12_000),
      headers: {
        ...(token ? { Authorization: "Bearer " + token } : {}),
        ...(body ? { "Content-Type": "application/json" } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new Error("Could not reach the local sync service. Local workspaces still work.");
  }
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || "Sync request failed.");
  return result as T;
}

export function Cloud({ account, workspaces, refresh }: {
  account: string; workspaces: Workspace[]; refresh: () => Promise<unknown>;
}) {
  const [deviceId] = useState(identifier);
  const [session, setSession] = useState<Session>();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [teams, setTeams] = useState<Team[]>([]);
  const [invitations, setInvitations] = useState<Invitation[]>([]);
  const [remote, setRemote] = useState<RemoteWorkspace[]>([]);
  const [teamId, setTeamId] = useState("");
  const [teamName, setTeamName] = useState("");
  const [inviteEmail, setInviteEmail] = useState("");
  const [members, setMembers] = useState<Account[]>([]);
  const [memberId, setMemberId] = useState("");
  const [remoteName, setRemoteName] = useState("");
  const [workspaceId, setWorkspaceId] = useState("");
  const [localId, setLocalId] = useState("");
  const [revisions, setRevisions] = useState<Revision[]>([]);
  const [version, setVersion] = useState("");
  const [lease, setLease] = useState<LeaseState>();
  const [passphrase, setPassphrase] = useState("");
  const [ticket, setTicket] = useState("");
  const [issuedTicket, setIssuedTicket] = useState("");
  const [pendingUpload, setPendingUpload] = useState<Upload>();
  const lock = useRef(false);
  const workspace = remote.find(row => row.id === workspaceId);
  const selectedTeam = teams.find(team => team.id === teamId);
  const path = "/workspaces/" + workspaceId;
  const token = session?.token;

  async function run(action: () => Promise<void>) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError(""); setNotice("");
    try { await action(); } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { lock.current = false; setBusy(false); }
  }

  async function lists(auth: string) {
    const [nextTeams, nextInvitations, nextRemote] = await Promise.all([
      api<Team[]>("/teams", auth), api<Invitation[]>("/invitations", auth), api<RemoteWorkspace[]>("/workspaces", auth),
    ]);
    setTeams(nextTeams); setInvitations(nextInvitations); setRemote(nextRemote);
    setTeamId(current => nextTeams.some(team => team.id === current) ? current : nextTeams[0]?.id ?? "");
    setWorkspaceId(current => nextRemote.some(row => row.id === current) ? current : "");
  }

  async function detail(auth = token, selected = path) {
    const [latestLease, history] = await Promise.all([
      api<LeaseState>(selected + "/lease", auth), api<Revision[]>(selected + "/snapshots", auth),
    ]);
    setLease(latestLease); setRevisions(history);
    setVersion(current => history.some(item => String(item.version) === current) ? current : String(history[0]?.version ?? ""));
  }

  useEffect(() => {
    let active = true;
    setMembers([]); setMemberId("");
    if (token && teamId) api<Account[]>(`/teams/${teamId}/members`, token)
      .then(items => { if (active) setMembers(items); }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [token, teamId]);

  useEffect(() => {
    let active = true;
    setLease(undefined); setRevisions([]); setVersion(""); setIssuedTicket("");
    if (token && workspaceId) Promise.all([
      api<LeaseState>(path + "/lease", token), api<Revision[]>(path + "/snapshots", token),
    ]).then(([nextLease, history]) => {
      if (active) {
        setLease(nextLease); setRevisions(history); setVersion(String(history[0]?.version ?? ""));
      }
    }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [token, path, workspaceId]);

  const fence = lease?.heldByThisDevice ? lease.lease?.fence : undefined;
  useEffect(() => {
    if (!token || !fence) return;
    let active = true;
    const timer = setInterval(async () => {
      if (lock.current) return;
      try {
        const renewed = await api<LeaseState>(path + "/lease", token, "POST", { operation: "heartbeat", fence });
        if (active) setLease({ ...renewed, heldByThisDevice: true });
      } catch {
        if (active) {
          setLease(undefined);
          setError("Lease renewal failed. Refresh shared status before publishing. Your local copy is unchanged.");
        }
      }
    }, 20_000);
    return () => { active = false; clearInterval(timer); };
  }, [token, fence, path]);

  async function signIn(create: boolean) {
    try {
      if (create) await api("/accounts", undefined, "POST", { email, password });
      const signedIn = await api<Session>("/sessions", undefined, "POST", { email, password, deviceId });
      setSession(signedIn);
      await lists(signedIn.token);
      setNotice("Signed in to the local sync service.");
    } finally { setPassword(""); }
  }

  async function leaseAction(operation: string) {
    const state = await api<LeaseState>(path + "/lease", token, "POST", { operation, fence });
    setLease({ ...state, heldByThisDevice: !!state.lease });
    setNotice(operation === "release" ? "Lease released." : `Lease held by this device · fence ${state.lease?.fence}.`);
  }

  async function publish(retry?: Upload) {
    if (!retry && (!workspace || !fence || !lease || !localId)) return;
    let upload = retry;
    try {
      if (!upload) {
        const sealed = await invoke<Sealed>("seal_workspace", { account, id: localId, passphrase });
        upload = { ...sealed, path, fence: fence!, generation: lease!.generation, requestId: identifier() };
        setPendingUpload(upload);
      }
      const saved = await api<Revision>(upload.path + "/snapshots", token, "POST", {
        fence: upload.fence, generation: upload.generation, requestId: upload.requestId,
        envelope: upload.envelope, sha256: upload.sha256,
      });
      setPendingUpload(undefined);
      await detail();
      setVersion(String(saved.version));
      setNotice(`Encrypted metadata snapshot v${saved.version} published.`);
    } finally { setPassphrase(""); }
  }

  async function restoreLocal() {
    try {
      const snapshot = await api<Sealed>(path + "/snapshots/" + version, token);
      await invoke("restore_workspace", { account, ...snapshot, passphrase });
      await refresh();
      setNotice(`Snapshot v${version} restored as a new local workspace.`);
    } finally { setPassphrase(""); }
  }

  return <section aria-label="Optional shared workspaces">
    <h2>Shared workspaces</h2>
    <p>Optional local sync service. You can use the workspace manager without signing in.</p>
    <p role="status">{notice}</p><p role="alert" className="error">{error}</p>
    {!session ? <form className="form-grid" onSubmit={e => { e.preventDefault(); void run(() => signIn(false)); }}>
      <label>Email<input type="email" required value={email} autoComplete="username" onChange={e => setEmail(e.target.value)} /></label>
      <label>Sync password<input type="password" required minLength={12} autoComplete="current-password" value={password} onChange={e => setPassword(e.target.value)} /></label>
      <button disabled={busy}>Sign in</button>
      <button type="button" disabled={busy || !email || password.length < 12} onClick={() => void run(() => signIn(true))}>Create sync account</button>
    </form> : <>
      <p>Signed in as <strong>{session.account.email}</strong>. Account ID: <code>{session.account.id}</code></p>
      <div className="actions">
        <button disabled={busy} onClick={() => void run(async () => {
          await lists(session.token); if (workspaceId) await detail();
        })}>Refresh shared status</button>
        <button disabled={busy} onClick={() => void run(async () => {
          try { if (fence) await leaseAction("release"); } catch { /* The lease expires without renewal. */ }
          try { await api("/sessions/current", session.token, "DELETE"); }
          finally { setSession(undefined); setPassphrase(""); setPendingUpload(undefined); setTicket(""); }
        })}>Sign out</button>
      </div>
      {invitations.map(invite => <p key={invite.id}>{invite.name} · {invite.role} <button disabled={busy} onClick={() => void run(async () => {
        await api(`/invitations/${invite.id}/accept`, session.token, "POST", {}); await lists(session.token);
      })}>Accept invitation</button></p>)}
      <div className="form-grid">
        <label>New team name<input value={teamName} maxLength={100} onChange={e => setTeamName(e.target.value)} /></label>
        <button disabled={busy || !teamName.trim()} onClick={() => void run(async () => {
          const team = await api<Team>("/teams", session.token, "POST", { name: teamName.trim() });
          await lists(session.token); setTeamId(team.id); setTeamName("");
        })}>Create team</button>
        <label>Team<select value={teamId} onChange={e => setTeamId(e.target.value)}>
          <option value="">Select team</option>{teams.map(team => <option key={team.id} value={team.id}>{team.name} · {team.role}</option>)}
        </select></label>
        {selectedTeam && selectedTeam.role !== "member" && <>
          <label>Invite email<input type="email" value={inviteEmail} onChange={e => setInviteEmail(e.target.value)} /></label>
          <button disabled={busy || !inviteEmail} onClick={() => void run(async () => {
            await api(`/teams/${teamId}/invitations`, session.token, "POST", { email: inviteEmail, role: "member" });
            setInviteEmail(""); setNotice("Invitation issued.");
          })}>Invite member</button>
        </>}
        <label>New shared workspace name<input value={remoteName} maxLength={100} onChange={e => setRemoteName(e.target.value)} /></label>
        <button disabled={busy || !teamId || !remoteName.trim()} onClick={() => void run(async () => {
          const created = await api<RemoteWorkspace>("/workspaces", session.token, "POST", { teamId, name: remoteName.trim() });
          await lists(session.token); setWorkspaceId(created.id); setRemoteName("");
        })}>Create shared workspace</button>
        <label>Shared workspace<select value={workspaceId} disabled={busy} onChange={e => {
          setWorkspaceId(e.target.value); setTeamId(remote.find(row => row.id === e.target.value)?.teamId ?? teamId);
        }}><option value="">Select workspace</option>
          {remote.map(row => <option key={row.id} value={row.id}>{row.name} · {row.role}</option>)}
        </select></label>
      </div>
      {workspace && <>
        <h3>{workspace.name} · {workspace.role}</h3>
        <p>{lease ? lease.lease ? `Lease ${fence ? "held by this device" : "held by another device"} · fence ${lease.lease.fence} · expires ${new Date(lease.lease.expiresAt).toLocaleTimeString()}` : "No active lease." : "Lease status not loaded."} Latest revision: {lease?.generation ?? workspace.generation}.</p>
        <div className="actions">
          <button disabled={busy || workspace.role === "reader" || !!fence} onClick={() => void run(() => leaseAction("acquire"))}>Acquire lease</button>
          <button disabled={busy || !fence} onClick={() => void run(() => leaseAction("release"))}>Release lease</button>
          {workspace.role === "owner" && <button disabled={busy || !lease?.lease || !!fence} onClick={() => {
            if (window.confirm("Take over the server lease? The previous device may still have a local copy open, but cannot publish with its old lease.")) void run(() => leaseAction("takeover"));
          }}>Take over lease</button>}
        </div>
        {workspace.role === "owner" && <>
          <label>Team member<select value={memberId} disabled={teamId !== workspace.teamId} onChange={e => setMemberId(e.target.value)}>
            <option value="">Select member</option>
            {members.filter(member => member.id !== session.account.id).map(member => <option value={member.id} key={member.id}>{member.email}</option>)}
          </select></label>
          <div className="actions">
            {(["reader", "operator", "revoke"] as const).map(role => <button key={role} disabled={busy || !memberId || teamId !== workspace.teamId} onClick={() => void run(async () => {
              await api(path + "/grants/" + memberId, token, role === "revoke" ? "DELETE" : "PUT", role === "revoke" ? undefined : { role });
              setNotice(role === "revoke" ? "Workspace access revoked." : `Granted ${role} access.`);
            })}>{role === "revoke" ? "Revoke access" : `Grant ${role}`}</button>)}
            <button disabled={busy || !memberId || !!lease?.lease || teamId !== workspace.teamId} onClick={() => {
              if (window.confirm("Transfer this workspace? You will lose access.")) void run(async () => {
                await api(path + "/transfer", token, "POST", { accountId: memberId });
                await lists(session.token); setNotice("Ownership transferred.");
              });
            }}>Transfer ownership</button>
            <button disabled={busy || !memberId || !!lease?.lease || !version || teamId !== workspace.teamId} onClick={() => void run(async () => {
              const result = await api<{ ticket: string }>(path + "/exchange", token, "POST", { accountId: memberId, version: Number(version) });
              setIssuedTicket(result.ticket);
            })}>Create exchange ticket</button>
          </div>
          {issuedTicket && <p>Single-use ticket: <code>{issuedTicket}</code>. Send it separately from the snapshot passphrase.</p>}
        </>}
        <h3>Encrypted metadata snapshots</h3>
        <p>Includes workspace name, icon, notes, launch URLs and assigned Slide bookmarks. Does not copy cookies, passwords, extensions, browser storage or proxy settings. Restore creates a new independent local workspace; the server lease protects publishing, not unrelated local copies.</p>
        <div className="form-grid">
          <label>Closed local workspace<select value={localId} onChange={e => setLocalId(e.target.value)}>
            <option value="">Select local workspace</option>
            {workspaces.filter(row => !row.trashedAt && (row.status === "closed" || row.status === "crashed")).map(row => <option key={row.id} value={row.id}>{row.name}</option>)}
          </select></label>
          <label>Snapshot revision<select value={version} onChange={e => setVersion(e.target.value)}>
            <option value="">Select revision</option>{revisions.map(row => <option key={row.version} value={row.version}>v{row.version}{row.restoredFrom ? ` (restored from v${row.restoredFrom})` : ""}</option>)}
          </select></label>
          <label>Snapshot passphrase<input type="password" autoComplete="off" value={passphrase} onChange={e => setPassphrase(e.target.value)} /></label>
        </div>
        <p>Share the passphrase separately with the recipient. Slide cannot recover it.</p>
        <div className="actions">
          <button disabled={busy || !!pendingUpload || !fence || !localId || passphrase.length < 12} onClick={() => void run(() => publish())}>Publish snapshot</button>
          <button disabled={busy || !version || passphrase.length < 12} onClick={() => void run(restoreLocal)}>Restore local copy</button>
          {workspace.role === "owner" && <button disabled={busy || !version || !fence || !lease} onClick={() => void run(async () => {
            const saved = await api<Revision>(path + "/restore", token, "POST", { version: Number(version), fence, generation: lease?.generation, requestId: identifier() });
            await detail(); setVersion(String(saved.version)); setNotice(`Published restored revision v${saved.version}.`);
          })}>Restore as new shared revision</button>}
          {pendingUpload?.path === path && <button disabled={busy} onClick={() => void run(() => publish(pendingUpload))}>Retry same upload</button>}
          {pendingUpload && <button disabled={busy} onClick={() => setPendingUpload(undefined)}>Discard pending upload</button>}
        </div>
      </>}
      <h3>Redeem exchange ticket</h3>
      <label>Exchange ticket for the selected team<input value={ticket} onChange={e => setTicket(e.target.value)} /></label>
      <button disabled={busy || !teamId || !ticket.trim()} onClick={() => void run(async () => {
        const created = await api<RemoteWorkspace>("/exchange/redeem", token, "POST", { teamId, ticket: ticket.trim() });
        await lists(session.token); setTicket(""); setWorkspaceId(created.id);
      })}>Redeem into team</button>
    </>}
  </section>;
}
