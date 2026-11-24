import type { SQL } from "bun";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { isEnvelope, MAX_ENVELOPE_BYTES, SYNC_API, TEAM_ROLES, WORKSPACE_ROLES } from "../../packages/schemas/sync";
import type { TeamRole, WorkspaceRole } from "../../packages/schemas/sync";
import type { BlobStore } from "./storage";
import { API_SCOPES, requiredScope } from "../../packages/schemas/public";
import { openapi } from "./openapi";

type Actor = { id: string; email: string; deviceId: string; apiTeam?: string; scopes?: string[] };
type Body = Record<string, unknown>;
type Workspace = {
  id: string; teamId: string; ownerId: string; generation: number; fence: string;
  holderId: string | null; holderDevice: string | null; active: boolean; role: WorkspaceRole;
};
const origins = new Set(["tauri://localhost", "http://tauri.localhost", "http://127.0.0.1:1420", "http://localhost:1420"]);
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const tokenPattern = /^[A-Za-z0-9_-]{43}$/;
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const exchangeEmailDigest = (address: string) => hash("exchange.email:" + address);
const secret = () => randomBytes(32).toString("base64url");

class Problem extends Error {
  constructor(readonly status: number, message: string) { super(message); }
}
function fail(status: number, message: string): never { throw new Problem(status, message); }
function uuid(value: unknown): string {
  if (typeof value !== "string" || !uuidPattern.test(value)) fail(400, "Invalid identifier.");
  return value;
}
function text(value: unknown, limit: number, min = 1): string {
  if (typeof value !== "string" || value.length < min || value.length > limit || /[\u0000-\u001f\u007f]/.test(value)) {
    fail(400, "Invalid text field.");
  }
  return value;
}
function email(value: unknown): string {
  const normalized = text(value, 254).toLowerCase().trim();
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(normalized)) fail(400, "Invalid email address.");
  return normalized;
}
function integer(value: unknown, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < min || value > max) fail(400, "Invalid numeric field.");
  return value;
}
function option<T extends string>(value: unknown, allowed: readonly T[]): T {
  if (typeof value !== "string" || !allowed.includes(value as T)) fail(400, "Invalid role.");
  return value as T;
}
async function body(request: Request): Promise<Body> {
  if (!request.headers.get("Content-Type")?.startsWith("application/json")) fail(415, "Send JSON.");
  const raw = await request.text();
  if (Buffer.byteLength(raw) > MAX_ENVELOPE_BYTES + 4096) fail(413, "Request too large.");
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { fail(400, "Invalid JSON."); }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) fail(400, "Expected a JSON object.");
  return parsed as Body;
}

export async function migrate(db: SQL) {
  const schema = await Bun.file(new URL("./schema.sql", import.meta.url)).text();
  await db.begin(async tx => {
    await tx`SELECT pg_advisory_xact_lock(83642001)`;
    await tx.unsafe(schema).simple();
  });
}

async function audit(db: SQL, actor: Actor, team: string, subject: string, action: string) {
  await db`INSERT INTO audit_events (team_id,actor_id,subject_id,action,api_token_id)
    VALUES (${team},${actor.id},${subject},${action},${actor.apiTeam ? actor.deviceId : null})`;
}

async function teamRole(db: SQL, team: string, member: string): Promise<TeamRole | undefined> {
  const [row] = await db`SELECT role FROM members WHERE team_id=${team} AND account_id=${member} FOR SHARE`;
  return row?.role;
}
function requireAdmin(role: TeamRole | undefined) {
  if (role !== "admin" && role !== "owner") fail(403, "Team administration required.");
}

async function workspace(db: SQL, id: string, actor: Actor, lock = false): Promise<Workspace> {
  const lockClause = lock ? db`FOR UPDATE` : db`FOR SHARE`;
  const [row] = await db`SELECT id, team_id AS "teamId", owner_id AS "ownerId", generation, fence::text,
    holder_id AS "holderId", holder_device AS "holderDevice"
    FROM workspaces WHERE id=${id} ${lockClause}`;
  if (!row || (actor.apiTeam && actor.apiTeam !== row.teamId) || !await teamRole(db, row.teamId, actor.id)) fail(404, "Workspace unavailable.");
  const [grant] = await db`SELECT role FROM grants WHERE workspace_id=${id} AND account_id=${actor.id}`;
  const role = row.ownerId === actor.id ? "owner" : grant?.role;
  if (!role) fail(404, "Workspace unavailable.");
  const [{ active }] = await db`SELECT coalesce(lease_until > clock_timestamp(), false) AS active FROM workspaces WHERE id=${id}`;
  return { ...row, role, active } as Workspace;
}
function owner(row: Workspace) { if (row.role !== "owner") fail(403, "Workspace ownership required."); }
function writeAccess(row: Workspace) { if (row.role !== "owner" && row.role !== "operator") fail(403, "Workspace write access required."); }
function sameHolder(row: Workspace, actor: Actor) { return row.holderId === actor.id && row.holderDevice === actor.deviceId; }
function requireLease(row: Workspace, actor: Actor, input: Body) {
  writeAccess(row);
  if (!row.active || !sameHolder(row, actor) || input.fence !== row.fence) fail(409, "The workspace lease is missing, expired or superseded.");
}
function requireGeneration(row: Workspace, input: Body) {
  if (integer(input.generation, 0, 2147483646) !== row.generation) fail(409, "The workspace has a newer revision.");
}
const revisionFields = (db: SQL) => db`version, sha256, size, created_at AS "createdAt", restored_from AS "restoredFrom"`;

export function createApi(db: SQL, blobs: BlobStore, leaseSeconds = 90, apiRateLimit = 120) {
  async function apiActor(token: string, headers: Record<string, string>): Promise<Actor> {
    const now = Math.floor(Date.now() / 1000);
    const window = now - now % 60;
    const [row] = await db`UPDATE api_tokens t SET window_requests=
        CASE WHEN t.window_start=${window} THEN t.window_requests+1 ELSE 1 END, window_start=${window}
      FROM accounts a WHERE t.digest=${hash(token)} AND t.account_id=a.id AND t.expires_at > clock_timestamp()
      RETURNING a.id,a.email,t.id AS "deviceId",t.team_id AS "apiTeam",t.scopes,t.window_requests`;
    if (!row) fail(401, "API token invalid, expired or revoked.");
    headers["RateLimit-Limit"] = String(apiRateLimit);
    headers["RateLimit-Remaining"] = String(Math.max(0, apiRateLimit - row.window_requests));
    headers["RateLimit-Reset"] = String(window + 60 - now);
    if (row.window_requests > apiRateLimit) {
      headers["Retry-After"] = headers["RateLimit-Reset"];
      fail(429, "API token rate limit exceeded.");
    }
    return row;
  }

  async function dispatch(request: Request, headers: Record<string, string>): Promise<unknown> {
    const url = new URL(request.url);
    const path = url.pathname;
    const method = request.method;
    if (path === "/health" && method === "GET") return { status: "ok" };
    if (path === SYNC_API + "/openapi.json" && method === "GET") return openapi;
    if (!path.startsWith(SYNC_API + "/")) fail(404, "Unknown endpoint.");
    const [resource, id, action, sub, ...rest] = path.slice(SYNC_API.length + 1).split("/");
    if (rest.length) fail(404, "Unknown endpoint.");
    if (resource === "accounts" && !id && method === "POST") {
      const data = await body(request);
      const address = email(data.email);
      const password = text(data.password, 256, 12);
      const passwordHash = await Bun.password.hash(password, { algorithm: "argon2id" });
      const [created] = await db`INSERT INTO accounts VALUES (${randomUUID()},${address},${passwordHash})
        ON CONFLICT (email) DO NOTHING RETURNING id,email`;
      if (!created) fail(409, "Account already exists.");
      return created;
    }
    if (resource === "sessions" && !id && method === "POST") {
      const data = await body(request);
      const deviceId = uuid(data.deviceId);
      const password = text(data.password, 256, 12);
      const [user] = await db`SELECT id,email,password_hash FROM accounts WHERE email=${email(data.email)}`;
      if (!user || !await Bun.password.verify(password, user.password_hash)) fail(401, "Invalid email or password.");
      const token = secret();
      await db`INSERT INTO sessions VALUES (${hash(token)},${user.id},${deviceId},clock_timestamp() + interval '12 hours')`;
      return { token, account: { id: user.id, email: user.email } };
    }
    const token = request.headers.get("Authorization")?.replace(/^Bearer /, "") ?? "";
    const isApiToken = token.startsWith("sw_") && tokenPattern.test(token.slice(3));
    if (!isApiToken && !tokenPattern.test(token)) fail(401, "Sign in first.");
    const actor = isApiToken ? await apiActor(token, headers) : (await db`SELECT a.id,a.email,s.device_id AS "deviceId" FROM sessions s JOIN accounts a ON a.id=s.account_id
      WHERE s.digest=${hash(token)} AND s.expires_at > clock_timestamp()` as Actor[])[0];
    if (!actor) fail(401, "Session expired. Sign in again.");
    if (actor.apiTeam) {
      const scope = requiredScope(method, resource, id, action, sub);
      if (!scope || !actor.scopes?.includes(scope)) fail(403, "API token scope does not allow this operation.");
      if (resource === "teams" && id && id !== actor.apiTeam) fail(404, "Team unavailable.");
    }
    if (resource === "sessions" && id === "current" && !action && method === "DELETE") {
      await db`DELETE FROM sessions WHERE digest=${hash(token)}`;
      return { signedOut: true };
    }
    if (resource === "teams" && !id && method === "GET") {
      return await db`SELECT t.id,t.name,m.role FROM teams t JOIN members m ON t.id=m.team_id
        WHERE m.account_id=${actor.id} AND (${actor.apiTeam ?? null}::uuid IS NULL OR t.id=${actor.apiTeam ?? null})
        ORDER BY t.name,t.id LIMIT 100`;
    }
    if (resource === "teams" && !id && method === "POST") {
      const name = text((await body(request)).name, 100);
      return db.begin(async tx => {
        const team = randomUUID();
        await tx`INSERT INTO teams VALUES (${team},${name})`;
        await tx`INSERT INTO members VALUES (${team},${actor.id},'owner')`;
        await audit(tx, actor, team, team, "team.create");
        return { id: team, name, role: "owner" };
      });
    }
    if (resource === "invitations" && id === "accept" && !action && method === "POST") {
      const invitationToken = (await body(request)).token;
      if (typeof invitationToken !== "string" || !tokenPattern.test(invitationToken)) fail(400, "Invalid invitation token.");
      return db.begin(async tx => {
        const [invite] = await tx`DELETE FROM invitations WHERE token_digest=${hash(invitationToken)} AND email=${actor.email}
          AND expires_at > clock_timestamp() RETURNING id,team_id,role`;
        if (!invite) fail(404, "Invitation unavailable.");
        await tx`INSERT INTO members VALUES (${invite.team_id},${actor.id},${invite.role}) ON CONFLICT DO NOTHING`;
        if (invite.role === "admin") {
          const [upgraded] = await tx`UPDATE members SET role='admin'
            WHERE team_id=${invite.team_id} AND account_id=${actor.id} AND role='member' RETURNING account_id`;
          if (upgraded) await audit(tx, actor, invite.team_id, actor.id, "member.promote");
        }
        await audit(tx, actor, invite.team_id, invite.id, "invitation.accept");
        return { teamId: invite.team_id };
      });
    }
    if (resource === "teams" && id && action) {
      const team = uuid(id);
      if (action === "tokens" && !actor.apiTeam) {
        return db.begin(async tx => {
          if (!await teamRole(tx, team, actor.id)) fail(404, "Team unavailable.");
          if (!sub && method === "GET") return await tx`SELECT id,name,scopes,expires_at AS "expiresAt"
            FROM api_tokens WHERE team_id=${team} AND account_id=${actor.id} ORDER BY name,id LIMIT 100`;
          if (!sub && method === "POST") {
            const data = await body(request);
            const name = text(data.name, 100);
            if (!Array.isArray(data.scopes) || data.scopes.length < 1 || data.scopes.length > API_SCOPES.length)
              fail(400, "Select valid API scopes.");
            const scopes = [...new Set(data.scopes.map(scope => option(scope, API_SCOPES)))];
            const days = data.expiresInDays === undefined ? 30 : integer(data.expiresInDays, 1, 90);
            const secretToken = "sw_" + secret(), tokenId = randomUUID();
            const [created] = await tx`INSERT INTO api_tokens (id,digest,team_id,account_id,name,scopes,expires_at)
              VALUES (${tokenId},${hash(secretToken)},${team},${actor.id},${name},
                string_to_array(${scopes.join(",")}::text, ','),
                clock_timestamp() + ${days} * interval '1 day') RETURNING id,name,scopes,expires_at AS "expiresAt"`;
            await audit(tx, actor, team, tokenId, "api_token.create");
            return { ...created, token: secretToken };
          }
          if (sub && method === "DELETE") {
            const [removed] = await tx`DELETE FROM api_tokens WHERE id=${uuid(sub)} AND team_id=${team}
              AND account_id=${actor.id} RETURNING id`;
            if (!removed) fail(404, "API token unavailable.");
            await audit(tx, actor, team, removed.id, "api_token.revoke");
            return { revoked: true };
          }
          fail(404, "Unknown endpoint.");
        });
      }
      if (action === "members" && !sub && method === "GET") {
        return db.begin(async tx => {
          if (!await teamRole(tx, team, actor.id)) fail(404, "Team unavailable.");
          return await tx`SELECT a.id,a.email,m.role FROM members m JOIN accounts a ON a.id=m.account_id
            WHERE m.team_id=${team} ORDER BY a.email LIMIT 100`;
        });
      }
      if (action === "members" && sub && method === "DELETE") {
        return db.begin(async tx => {
          await tx`SELECT id FROM teams WHERE id=${team} FOR NO KEY UPDATE`;
          const role = await teamRole(tx, team, actor.id);
          requireAdmin(role);
          const member = uuid(sub);
          if (member === actor.id) fail(400, "Cannot remove yourself.");
          await tx`SELECT id FROM workspaces WHERE team_id=${team} AND owner_id=${member} ORDER BY id FOR SHARE`;
          const [target] = await tx`SELECT role FROM members WHERE team_id=${team} AND account_id=${member} FOR UPDATE`;
          const [owned] = await tx`SELECT id FROM workspaces WHERE team_id=${team} AND owner_id=${member} LIMIT 1`;
          if (owned || !(target?.role === "member" || (target?.role === "admin" && role === "owner")))
            fail(409, "Member unavailable, protected, or still owns workspaces.");
          await tx`DELETE FROM members WHERE team_id=${team} AND account_id=${member}`;
          await tx`DELETE FROM grants g USING workspaces w WHERE w.id=g.workspace_id AND w.team_id=${team} AND g.account_id=${member}`;
          await audit(tx, actor, team, member, "member.remove");
          return { removed: true };
        });
      }
      if (action === "invitations" && !sub && method === "POST") {
        const data = await body(request);
        const address = email(data.email), inviteRole = option(data.role, TEAM_ROLES);
        return db.begin(async tx => {
          const role = await teamRole(tx, team, actor.id);
          requireAdmin(role);
          if (inviteRole === "admin" && role !== "owner") fail(403, "Only the team owner can invite an administrator.");
          const invite = randomUUID(), invitationToken = secret();
          await tx`INSERT INTO invitations (id,team_id,email,role,expires_at,token_digest)
            VALUES (${invite},${team},${address},${inviteRole},clock_timestamp() + interval '24 hours',${hash(invitationToken)})`;
          await audit(tx, actor, team, invite, "invitation.create");
          return { id: invite, token: invitationToken };
        });
      }
      if (action === "invitations" && sub && method === "DELETE") {
        return db.begin(async tx => {
          const role = await teamRole(tx, team, actor.id);
          requireAdmin(role);
          const [invite] = await tx`DELETE FROM invitations WHERE id=${uuid(sub)} AND team_id=${team}
            AND (role='member' OR ${role}='owner') RETURNING id`;
          if (!invite) fail(404, "Invitation unavailable.");
          await audit(tx, actor, team, invite.id, "invitation.revoke");
          return { revoked: true };
        });
      }
      if (action === "audit" && !sub && method === "GET") {
        return db.begin(async tx => {
          requireAdmin(await teamRole(tx, team, actor.id));
          return await tx`SELECT id::text,actor_id AS "actorId",api_token_id AS "apiTokenId",subject_id AS "subjectId",action,created_at AS "createdAt"
            FROM audit_events WHERE team_id=${team} ORDER BY id DESC LIMIT 100`;
        });
      }
    }
    if (resource === "workspaces" && !id && method === "GET") {
      if (actor.apiTeam) {
        const after = url.searchParams.has("after") ? uuid(url.searchParams.get("after")) : null;
        const limit = url.searchParams.has("limit") ? integer(Number(url.searchParams.get("limit")), 1, 100) : 50;
        const rows = await db`SELECT w.id,w.team_id AS "teamId",w.name,w.generation,
          CASE WHEN w.owner_id=${actor.id} THEN 'owner' ELSE g.role END AS role FROM workspaces w
          JOIN members m ON m.team_id=w.team_id AND m.account_id=${actor.id}
          LEFT JOIN grants g ON g.workspace_id=w.id AND g.account_id=${actor.id}
          WHERE w.team_id=${actor.apiTeam} AND (w.owner_id=${actor.id} OR g.role IS NOT NULL)
            AND (${after}::uuid IS NULL OR w.id > ${after}::uuid) ORDER BY w.id LIMIT ${limit + 1}`;
        const items = rows.slice(0, limit);
        return { items, next: rows.length > limit ? items.at(-1)?.id : null };
      }
      return await db`SELECT w.id,w.team_id AS "teamId",w.name,w.generation,
        CASE WHEN w.owner_id=${actor.id} THEN 'owner' ELSE g.role END AS role FROM workspaces w
        JOIN members m ON m.team_id=w.team_id AND m.account_id=${actor.id}
        LEFT JOIN grants g ON g.workspace_id=w.id AND g.account_id=${actor.id}
        WHERE w.owner_id=${actor.id} OR g.role IS NOT NULL ORDER BY w.name,w.id LIMIT 100`;
    }
    if (resource === "workspaces" && !id && method === "POST") {
      const data = await body(request);
      const team = uuid(data.teamId), name = text(data.name, 100);
      if (actor.apiTeam && team !== actor.apiTeam) fail(404, "Team unavailable.");
      const key = actor.apiTeam ? uuid(request.headers.get("Idempotency-Key")) : null;
      return db.begin(async tx => {
        if (!await teamRole(tx, team, actor.id)) fail(403, "Team membership required.");
        const workspaceId = randomUUID();
        if (key) {
          const [reservation] = await tx`INSERT INTO api_workspace_requests VALUES (${actor.deviceId},${key},${name},${workspaceId})
            ON CONFLICT DO NOTHING RETURNING workspace_id`;
          if (!reservation) {
            const [previous] = await tx`SELECT name,workspace_id FROM api_workspace_requests
              WHERE token_id=${actor.deviceId} AND request_key=${key}`;
            if (previous.name !== name) fail(409, "Idempotency key already used for another workspace creation.");
            return { id: previous.workspace_id, teamId: team, name, generation: 0, role: "owner" };
          }
        }
        await tx`INSERT INTO workspaces (id,team_id,owner_id,name) VALUES (${workspaceId},${team},${actor.id},${name})`;
        await audit(tx, actor, team, workspaceId, "workspace.create");
        return { id: workspaceId, teamId: team, name, generation: 0, role: "owner" };
      });
    }
    if (resource === "workspaces" && id && action) {
      return workspaceAction(request, actor, uuid(id), action, sub);
    }
    if (resource === "exchange" && id === "redeem" && !action && method === "POST") {
      const data = await body(request);
      const team = uuid(data.teamId);
      if (typeof data.ticket !== "string" || !tokenPattern.test(data.ticket)) fail(400, "Invalid exchange ticket.");
      const digest = hash(data.ticket);
      return db.begin(async tx => {
        if (!await teamRole(tx, team, actor.id)) fail(403, "Destination team membership required.");
        const [ticket] = await tx`UPDATE exchange_tickets SET redeemed_at=clock_timestamp()
          WHERE digest=${digest} AND (recipient_id=${actor.id} OR recipient_email_digest=${exchangeEmailDigest(actor.email)})
            AND redeemed_at IS NULL AND expires_at > clock_timestamp()
          RETURNING workspace_id,version`;
        if (!ticket) fail(404, "Exchange ticket unavailable or already used.");
        const [source] = await tx`SELECT w.name,w.team_id,r.blob_key,r.sha256,r.size FROM workspaces w
          JOIN revisions r ON r.workspace_id=w.id WHERE w.id=${ticket.workspace_id} AND r.version=${ticket.version}`;
        const newId = randomUUID();
        await tx`INSERT INTO workspaces (id,team_id,owner_id,name,generation) VALUES (${newId},${team},${actor.id},${source.name},1)`;
        await tx`INSERT INTO revisions (workspace_id,version,request_id,actor_id,blob_key,sha256,size)
          VALUES (${newId},1,${randomUUID()},${actor.id},${source.blob_key},${source.sha256},${source.size})`;
        await audit(tx, actor, team, newId, "exchange.redeem");
        return { id: newId, teamId: team, name: source.name, generation: 1, role: "owner" };
      });
    }
    fail(404, "Unknown endpoint.");
  }

  async function workspaceAction(request: Request, actor: Actor, id: string, action: string, sub?: string) {
    const method = request.method;
    if (action === "snapshots" && method === "GET") {
      return db.begin(async tx => {
        await workspace(tx, id, actor);
        if (!sub) return await tx`SELECT ${revisionFields(tx)} FROM revisions WHERE workspace_id=${id} ORDER BY version DESC LIMIT 100`;
        const version = integer(Number(sub), 1, 2147483647);
        const [revision] = await tx`SELECT blob_key, ${revisionFields(tx)} FROM revisions WHERE workspace_id=${id} AND version=${version}`;
        if (!revision) fail(404, "Snapshot unavailable.");
        const bytes = await blobs.get(revision.blob_key);
        if (Buffer.byteLength(bytes) !== revision.size || hash(bytes) !== revision.sha256) fail(502, "Snapshot failed integrity verification.");
        return { version, sha256: revision.sha256, envelope: JSON.parse(bytes) };
      });
    }
    if (action === "grants" && sub && (method === "PUT" || method === "DELETE")) {
      const recipient = uuid(sub);
      const role = method === "PUT" ? option((await body(request)).role, WORKSPACE_ROLES) : null;
      return db.begin(async tx => {
        const row = await workspace(tx, id, actor, true);
        owner(row);
        if (recipient === row.ownerId || !await teamRole(tx, row.teamId, recipient)) fail(400, "Choose a different team member.");
        if (role) await tx`INSERT INTO grants VALUES (${id},${recipient},${role})
          ON CONFLICT (workspace_id,account_id) DO UPDATE SET role=excluded.role`;
        else await tx`DELETE FROM grants WHERE workspace_id=${id} AND account_id=${recipient}`;
        if (row.holderId === recipient && role !== "operator") {
          await tx`UPDATE workspaces SET holder_id=NULL,holder_device=NULL,lease_until=NULL WHERE id=${id}`;
        }
        await audit(tx, actor, row.teamId, id, role ? "grant.set" : "grant.revoke");
        return { accountId: recipient, role };
      });
    }
    if (action === "lease" && !sub && method === "GET") {
      return db.begin(async tx => {
        const row = await workspace(tx, id, actor);
        const [lease] = row.active ? await tx`SELECT holder_id AS "holderId", fence::text,
          lease_until AS "expiresAt", generation FROM workspaces WHERE id=${id}` : [];
        return { lease: lease ?? null, heldByThisDevice: row.active && sameHolder(row, actor), generation: row.generation };
      });
    }
    if (method !== "POST" || sub) fail(404, "Unknown endpoint.");
    const data = await body(request);
    return db.begin(async tx => {
      if (action === "transfer") {
        await tx`SELECT id FROM teams WHERE id=(SELECT team_id FROM workspaces WHERE id=${id}) FOR NO KEY UPDATE`;
      }
      const row = await workspace(tx, id, actor, true);
      switch (action) {
        case "lease": {
          writeAccess(row);
          const operation = option(data.operation, ["acquire", "heartbeat", "release", "takeover"] as const);
          if (operation === "takeover") owner(row);
          if (operation === "heartbeat" || operation === "release") requireLease(row, actor, data);
          if (operation === "release") {
            await tx`UPDATE workspaces SET holder_id=NULL,holder_device=NULL,lease_until=NULL WHERE id=${id}`;
            await audit(tx, actor, row.teamId, id, "lease.release");
            return { lease: null, generation: row.generation };
          }
          if (operation === "acquire" && row.active && !sameHolder(row, actor)) fail(409, "Workspace already leased.");
          const increment = operation === "takeover" || !row.active ? 1 : 0;
          const [lease] = await tx`UPDATE workspaces SET holder_id=${actor.id},holder_device=${actor.deviceId},
            lease_until=clock_timestamp() + ${leaseSeconds} * interval '1 second',fence=fence+${increment}
            WHERE id=${id} RETURNING holder_id AS "holderId",fence::text,lease_until AS "expiresAt",generation`;
          if (operation !== "heartbeat") await audit(tx, actor, row.teamId, id, "lease." + operation);
          return { lease, generation: row.generation };
        }
        case "snapshots":
        case "restore": {
          writeAccess(row);
          if (action === "restore") owner(row);
          const requestId = uuid(data.requestId);
          const sourceVersion = action === "restore" ? integer(data.version, 1, 2147483647) : null;
          if (action === "snapshots" && !isEnvelope(data.envelope)) fail(400, "Unsupported encrypted snapshot envelope.");
          const bytes = action === "snapshots" ? JSON.stringify(data.envelope) : null;
          const digest = bytes === null ? null : hash(bytes);
          const size = bytes === null ? 0 : Buffer.byteLength(bytes);
          if (size > MAX_ENVELOPE_BYTES) fail(413, "Snapshot too large.");
          const [previous] = await tx`SELECT actor_id, ${revisionFields(tx)} FROM revisions WHERE workspace_id=${id} AND request_id=${requestId}`;
          if (previous) {
            if (previous.actor_id !== actor.id || previous.restoredFrom !== sourceVersion || (digest !== null && previous.sha256 !== digest))
              fail(409, "Snapshot request ID was already used.");
            const { actor_id: _, ...revision } = previous;
            return revision;
          }
          requireLease(row, actor, data);
          requireGeneration(row, data);
          let blob = { blob_key: randomUUID(), sha256: digest, size };
          if (sourceVersion !== null) {
            const [source] = await tx`SELECT blob_key,sha256,size FROM revisions WHERE workspace_id=${id} AND version=${sourceVersion}`;
            if (!source) fail(404, "Snapshot unavailable.");
            const stored = await blobs.get(source.blob_key);
            if (Buffer.byteLength(stored) !== source.size || hash(stored) !== source.sha256) fail(502, "Snapshot failed integrity verification.");
            blob = source;
          } else if (bytes !== null) {
            if (data.sha256 !== digest) fail(400, "Snapshot digest mismatch.");
            await blobs.put(blob.blob_key, bytes);
          }
          const [updated] = await tx`UPDATE workspaces SET generation=generation+1 WHERE id=${id}
            AND lease_until > clock_timestamp() RETURNING generation`;
          if (!updated) fail(409, "Lease expired before the snapshot could be published.");
          const [revision] = await tx`INSERT INTO revisions (workspace_id,version,request_id,actor_id,blob_key,sha256,size,restored_from)
            VALUES (${id},${updated.generation},${requestId},${actor.id},${blob.blob_key},${blob.sha256},${blob.size},${sourceVersion})
            RETURNING ${revisionFields(tx)}`;
          await audit(tx, actor, row.teamId, id, action === "restore" ? "snapshot.restore" : "snapshot.publish");
          return revision;
        }
        case "transfer": {
          owner(row);
          if (row.active) fail(409, "Release the workspace lease before transfer.");
          const recipient = uuid(data.accountId);
          if (recipient === actor.id || !await teamRole(tx, row.teamId, recipient)) fail(400, "Choose a different team member.");
          await tx`UPDATE workspaces SET owner_id=${recipient},holder_id=NULL,holder_device=NULL,lease_until=NULL,fence=fence+1 WHERE id=${id}`;
          await tx`DELETE FROM grants WHERE workspace_id=${id} AND account_id IN (${actor.id},${recipient})`;
          await tx`UPDATE exchange_tickets SET redeemed_at=clock_timestamp() WHERE workspace_id=${id} AND redeemed_at IS NULL`;
          await audit(tx, actor, row.teamId, id, "workspace.transfer");
          return { ownerId: recipient };
        }
        case "exchange": {
          owner(row);
          if (row.active) fail(409, "Release the workspace lease before exchange.");
          if ((data.email === undefined) === (data.accountId === undefined)) fail(400, "Provide one recipient email or account ID.");
          const version = integer(data.version, 1, row.generation);
          const ttl = data.expiresIn === undefined ? 900 : integer(data.expiresIn, 1, 3600);
          let recipientId = null;
          let recipientEmailDigest = null;
          if (data.email !== undefined) {
            const address = email(data.email);
            if (address === actor.email) fail(400, "Choose a different recipient.");
            recipientEmailDigest = exchangeEmailDigest(address);
          } else {
            const [account] = await tx`SELECT id FROM accounts WHERE id=${uuid(data.accountId)}`;
            if (!account || account.id === actor.id) fail(400, "Choose a different existing account.");
            recipientId = account.id;
          }
          const ticket = secret();
          const [issued] = await tx`INSERT INTO exchange_tickets (digest,workspace_id,version,recipient_id,recipient_email_digest,expires_at)
            VALUES (${hash(ticket)},${id},${version},${recipientId},${recipientEmailDigest},clock_timestamp() + ${ttl} * interval '1 second')
            RETURNING expires_at AS "expiresAt"`;
          await audit(tx, actor, row.teamId, id, "exchange.issue");
          return { ticket, expiresAt: issued.expiresAt };
        }
        default: fail(404, "Unknown endpoint.");
      }
    });
  }

  return async (request: Request): Promise<Response> => {
    const origin = request.headers.get("Origin");
    const headers: Record<string, string> = { "Cache-Control": "no-store", Vary: "Origin" };
    if (origin) {
      if (!origins.has(origin)) return Response.json({ error: "Origin not allowed." }, { status: 403, headers });
      headers["Access-Control-Allow-Origin"] = origin;
      headers["Access-Control-Allow-Headers"] = "Authorization, Content-Type, Idempotency-Key";
      headers["Access-Control-Expose-Headers"] = "RateLimit-Limit, RateLimit-Remaining, RateLimit-Reset, Retry-After";
      headers["Access-Control-Allow-Methods"] = "GET, POST, PUT, DELETE, OPTIONS";
    }
    if (request.method === "OPTIONS") return new Response(null, { status: 204, headers });
    try {
      return Response.json(await dispatch(request, headers), { headers });
    } catch (error) {
      if (error instanceof Problem) return Response.json({ error: error.message }, { status: error.status, headers });
      console.error("Slide API operation failed", error instanceof Error ? error.name : "Unexpected error");
      return Response.json({ error: "Sync service operation failed." }, { status: 500, headers });
    }
  };
}
