import { afterAll, beforeAll, expect, test } from "bun:test";
import { SQL } from "bun";
import { createHash, randomBytes, randomUUID, webcrypto } from "node:crypto";
import { createApi, migrate } from "./api";
import { objectStorage } from "./storage";

const url = process.env.SLIDE_TEST_DATABASE_URL;
const db = url ? new SQL(url) : null;
const saved = new Map<string, string>();
const blobs = process.env.S3_ENDPOINT ? objectStorage() : {
  async put(key: string, bytes: string) { saved.set(key, bytes); },
  async get(key: string) { return saved.get(key) ?? ""; },
};
const api = db ? createApi(db, blobs) : undefined;
const password = randomBytes(24).toString("base64url");

beforeAll(async () => { if (db) await migrate(db); });
afterAll(async () => { await db?.close(); });

async function call(path: string, method = "GET", token?: string, data?: unknown) {
  const response = await api!(new Request("http://localhost/v1" + path, {
    method, headers: {
      ...(token ? { Authorization: "Bearer " + token } : {}),
      ...(data !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: data === undefined ? undefined : JSON.stringify(data),
  }));
  return { status: response.status, data: await response.json() };
}
async function ok(path: string, method = "GET", token?: string, data?: unknown) {
  const result = await call(path, method, token, data);
  if (result.status !== 200) throw new Error(`${method} ${path}: HTTP ${result.status}: ${result.data.error}`);
  return result.data;
}
async function account(email = `${randomUUID()}@slide.test`) {
  await ok("/accounts", "POST", undefined, { email, password });
  return ok("/sessions", "POST", undefined, { email, password, deviceId: randomUUID() });
}
async function scenario() {
  const [alice, bob] = await Promise.all([account(), account()]);
  const team = await ok("/teams", "POST", alice.token, { name: "Fixture team" });
  const workspace = await ok("/workspaces", "POST", alice.token, { teamId: team.id, name: "Fixture workspace" });
  const invite = await ok(`/teams/${team.id}/invitations`, "POST", alice.token, { email: bob.account.email, role: "member" });
  await ok("/invitations/accept", "POST", bob.token, { token: invite.token });
  return { alice, bob, team, workspace, path: `/workspaces/${workspace.id}` };
}

test.skipIf(!url)("API tokens are hashed, scoped to one team, revocable, and limited atomically", async () => {
  const { alice, bob, team, workspace } = await scenario();
  const other = await ok("/teams", "POST", alice.token, { name: "Other team" });
  const otherWorkspace = await ok("/workspaces", "POST", alice.token, { teamId: other.id, name: "Other workspace" });
  const tokens = `/teams/${team.id}/tokens`;
  const issued = await ok(tokens, "POST", alice.token, { name: "Narrow QA token", scopes: ["workspaces:read"] });
  const [stored] = await db!`SELECT digest FROM api_tokens WHERE id=${issued.id}`;
  expect(stored.digest).toBe(createHash("sha256").update(issued.token).digest("hex"));
  expect((await ok(tokens, "GET", alice.token))[0].token).toBeUndefined();
  expect(await ok("/workspaces?limit=1", "GET", issued.token)).toMatchObject({ items: [{ id: workspace.id }], next: null });
  expect((await call("/workspaces", "POST", issued.token, { teamId: team.id, name: "Denied" })).status).toBe(403);
  expect((await call(`/workspaces/${otherWorkspace.id}/snapshots`, "GET", issued.token)).status).toBe(404);
  expect((await call(tokens, "POST", issued.token, { name: "Denied", scopes: ["workspaces:write"] })).status).toBe(403);
  const member = await ok(tokens, "POST", bob.token, { name: "Member audit", scopes: ["audit:read"] });
  expect((await call(`/teams/${team.id}/audit`, "GET", member.token)).status).toBe(403);
  const writer = await ok(tokens, "POST", alice.token, { name: "Writer", scopes: ["workspaces:write"] });
  const key = randomUUID();
  const create = (name: string) => api!(new Request("http://localhost/v1/workspaces", { method: "POST",
    headers: { Authorization: "Bearer " + writer.token, "Content-Type": "application/json", "Idempotency-Key": key },
    body: JSON.stringify({ name, teamId: team.id }),
  }));
  const creations = await Promise.all([create("Idempotent"), create("Idempotent")]);
  expect(creations.map(response => response.status)).toEqual([200, 200]);
  const [first, second] = await Promise.all(creations.map(response => response.json()));
  expect(first).toEqual(second);
  expect((await create("Different")).status).toBe(409);
  expect(await ok(`/teams/${team.id}/audit`, "GET", alice.token)).toContainEqual(expect.objectContaining({
    apiTokenId: writer.id, subjectId: first.id, action: "workspace.create",
  }));
  expect(await ok("/openapi.json")).toMatchObject({ openapi: "3.1.0", paths: { "/workspaces": {
    get: { "x-required-scope": "workspaces:read" },
  } } });
  const limited = createApi(db!, blobs, 90, 2);
  const fresh = await ok(tokens, "POST", alice.token, { name: "Burst test", scopes: ["workspaces:read"] });
  const burst = await Promise.all(Array.from({ length: 3 }, () => limited(new Request("http://localhost/v1/workspaces", {
    headers: { Authorization: "Bearer " + fresh.token },
  }))));
  expect(burst.map(row => row.status).sort()).toEqual([200, 200, 429]);
  const rejected = burst.find(row => row.status === 429)!;
  expect(rejected.headers.get("RateLimit-Remaining")).toBe("0");
  expect(Number(rejected.headers.get("Retry-After"))).toBeGreaterThan(0);
  await ok(tokens + "/" + issued.id, "DELETE", alice.token);
  expect((await call("/workspaces", "GET", issued.token)).status).toBe(401);
});

test.skipIf(!url)("invitation acceptance requires an unexpired private token, the intended account, and single use", async () => {
  const [alice, bob] = await Promise.all([account(), account()]);
  const team = await ok("/teams", "POST", alice.token, { name: "Private invitation" });
  const issue = () => ok(`/teams/${team.id}/invitations`, "POST", alice.token, { email: bob.account.email, role: "member" });
  const accept = (auth: string, token?: string) => call("/invitations/accept", "POST", auth, { token });
  const invite = await issue();
  expect((await accept(bob.token)).status).toBe(400);
  expect((await accept(bob.token, randomBytes(32).toString("base64url"))).status).toBe(404);
  expect((await accept(alice.token, invite.token)).status).toBe(404);
  expect(await ok("/teams", "GET", bob.token)).toEqual([]);
  const expired = await issue();
  await db!`UPDATE invitations SET expires_at=clock_timestamp() - interval '1 second' WHERE id=${expired.id}`;
  expect((await accept(bob.token, expired.token)).status).toBe(404);
  expect((await Promise.all([accept(bob.token, invite.token), accept(bob.token, invite.token)])).map(reply => reply.status).sort()).toEqual([200, 404]);
});

test.skipIf(!url)("server enforces workspace grants, invitation roles, session revocation and audit access", async () => {
  const { alice, bob, team, path } = await scenario();
  expect((await call(path + "/snapshots")).status).toBe(401);
  expect(await ok("/workspaces", "GET", bob.token)).toEqual([]);
  expect((await call(path + "/snapshots", "GET", bob.token)).status).toBe(404);
  expect((await call(`/teams/${team.id}/audit`, "GET", bob.token)).status).toBe(403);
  expect((await call(`/teams/${team.id}/invitations`, "POST", bob.token, { email: bob.account.email, role: "admin" })).status).toBe(403);
  await ok(path + "/grants/" + bob.account.id, "PUT", alice.token, { role: "reader" });
  expect(await ok(path + "/snapshots", "GET", bob.token)).toEqual([]);
  expect((await call(path + "/lease", "POST", bob.token, { operation: "acquire" })).status).toBe(403);
  await ok(path + "/grants/" + bob.account.id, "DELETE", alice.token);
  expect((await call(path + "/snapshots", "GET", bob.token)).status).toBe(404);
  const audit = await ok(`/teams/${team.id}/audit`, "GET", alice.token);
  expect(audit.map((row: { action: string }) => row.action)).toContain("grant.revoke");
  await expect(db!`DELETE FROM audit_events WHERE team_id=${team.id}`).rejects.toThrow("append-only");
  await ok("/sessions/current", "DELETE", alice.token);
  expect((await call("/teams", "GET", alice.token)).status).toBe(401);
});

test.skipIf(!url)("an admin invitation upgrades an existing member and lower invitations never downgrade", async () => {
  const { alice, bob, team } = await scenario();
  const issueAndAccept = async (sender: typeof alice, recipient: typeof bob, role: string) => {
    const invite = await ok(`/teams/${team.id}/invitations`, "POST", sender.token, { email: recipient.account.email, role });
    await ok("/invitations/accept", "POST", recipient.token, { token: invite.token });
  };
  await issueAndAccept(alice, bob, "admin");
  expect(await ok("/teams", "GET", bob.token)).toMatchObject([{ role: "admin" }]);
  await issueAndAccept(alice, bob, "member");
  expect(await ok("/teams", "GET", bob.token)).toMatchObject([{ role: "admin" }]);
  await issueAndAccept(alice, alice, "admin");
  expect(await ok("/teams", "GET", alice.token)).toMatchObject([{ role: "owner" }]);
  const audit = await ok(`/teams/${team.id}/audit`, "GET", bob.token);
  expect(audit.filter((row: { action: string }) => row.action === "member.promote")).toMatchObject([
    { actorId: bob.account.id, subjectId: bob.account.id },
  ]);
});

test.skipIf(!url)("member removal rechecks ownership after a concurrent transfer commits", async () => {
  const { alice, bob, team, workspace, path } = await scenario();
  let release!: () => void;
  const canCommit = new Promise<void>(resolve => { release = resolve; });
  const locked = Promise.withResolvers<number>();
  const transfer = db!.begin(async tx => {
    await tx`SELECT id FROM teams WHERE id=${team.id} FOR NO KEY UPDATE`;
    await tx`SELECT id FROM workspaces WHERE id=${workspace.id} FOR UPDATE`;
    await tx`SELECT role FROM members WHERE team_id=${team.id} AND account_id=${bob.account.id} FOR SHARE`;
    await tx`UPDATE workspaces SET owner_id=${bob.account.id} WHERE id=${workspace.id}`;
    const [{ pid }] = await tx`SELECT pg_backend_pid() AS pid`;
    locked.resolve(pid);
    await canCommit;
  });
  const pid = await Promise.race([locked.promise, transfer.then(() => { throw new Error("Transfer did not hold its locks."); })]);
  const removal = call(`/teams/${team.id}/members/${bob.account.id}`, "DELETE", alice.token);
  let blocked = false;
  try {
    const deadline = performance.now() + 2_000;
    while (!blocked && performance.now() < deadline) {
      const [state] = await db!`SELECT EXISTS (SELECT FROM pg_stat_activity WHERE ${pid}::int = ANY(pg_blocking_pids(pid))) AS blocked`;
      blocked = state.blocked;
      if (!blocked) await Bun.sleep(10);
    }
  } finally {
    release();
    await transfer;
  }
  expect(blocked).toBe(true);
  expect((await removal).status).toBe(409);
  expect((await ok("/workspaces", "GET", bob.token))[0]).toMatchObject({ id: workspace.id, role: "owner" });
  expect((await ok(path + "/lease", "POST", bob.token, { operation: "acquire" })).lease.holderId).toBe(bob.account.id);
});

test.skipIf(!url)("exclusive leases serialize races, expire and reject a stale device after takeover", async () => {
  const { alice, bob, workspace, path } = await scenario();
  await ok(path + "/grants/" + bob.account.id, "PUT", alice.token, { role: "operator" });
  const race = await Promise.all([alice, bob].map(user => call(path + "/lease", "POST", user.token, { operation: "acquire" })));
  expect(race.map(reply => reply.status).sort()).toEqual([200, 409]);
  const winner = race[0].status === 200 ? alice : bob;
  const first = race.find(reply => reply.status === 200)!.data.lease.fence;
  const takeover = await ok(path + "/lease", "POST", alice.token, { operation: "takeover" });
  expect(BigInt(takeover.lease.fence)).toBe(BigInt(first) + 1n);
  expect((await call(path + "/lease", "POST", winner.token, { operation: "heartbeat", fence: first })).status).toBe(409);
  await db!`UPDATE workspaces SET lease_until=clock_timestamp() - interval '1 second' WHERE id=${workspace.id}`;
  const next = await ok(path + "/lease", "POST", bob.token, { operation: "acquire" });
  expect(BigInt(next.lease.fence)).toBe(BigInt(takeover.lease.fence) + 1n);
  expect((await call(path + "/lease", "POST", alice.token, { operation: "release", fence: takeover.lease.fence })).status).toBe(409);
});

test.skipIf(!url)("encrypted handoff verifies bytes, retains versions, and fences transfer and redemption", async () => {
  const { alice, bob, team, workspace, path } = await scenario();
  await ok(path + "/grants/" + bob.account.id, "PUT", alice.token, { role: "operator" });
  const a = await ok(path + "/lease", "POST", alice.token, { operation: "acquire" });
  const nonce = randomBytes(12), key = await webcrypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
  const plaintext = new TextEncoder().encode("synthetic workspace content");
  const ciphertext = await webcrypto.subtle.encrypt({ name: "AES-GCM", iv: nonce }, key, plaintext);
  const envelope = { version: 1, salt: randomBytes(16).toString("base64"), nonce: nonce.toString("base64"), ciphertext: Buffer.from(ciphertext).toString("base64") };
  const sha256 = createHash("sha256").update(JSON.stringify(envelope)).digest("hex");
  const draft = { fence: a.lease.fence, generation: 0, envelope, sha256, requestId: randomUUID() };
  expect((await call(path + "/snapshots", "POST", alice.token, { ...draft, sha256: "invalid" })).status).toBe(400);
  expect((await ok(path + "/snapshots", "POST", alice.token, draft)).version).toBe(1);
  expect((await ok(path + "/snapshots", "POST", alice.token, draft)).version).toBe(1);
  expect((await call(path + "/transfer", "POST", alice.token, { accountId: bob.account.id })).status).toBe(409);
  await ok(path + "/lease", "POST", alice.token, { operation: "release", fence: a.lease.fence });
  const b = await ok(path + "/lease", "POST", bob.token, { operation: "acquire" });
  expect(b.generation).toBe(1);
  expect((await call(path + "/snapshots", "POST", alice.token, { ...draft, requestId: randomUUID(), generation: 1 })).status).toBe(409);
  const downloaded = await ok(path + "/snapshots/1", "GET", bob.token);
  expect(downloaded.sha256).toBe(sha256);
  const decrypted = await webcrypto.subtle.decrypt({ name: "AES-GCM", iv: nonce }, key, Buffer.from(downloaded.envelope.ciphertext, "base64"));
  expect(new TextDecoder().decode(decrypted)).toBe("synthetic workspace content");
  const [blob] = await db!`SELECT blob_key FROM revisions WHERE workspace_id=${workspace.id} AND version=1`;
  await blobs.put(blob.blob_key, "corrupted");
  expect((await call(path + "/snapshots/1", "GET", bob.token)).status).toBe(502);
  await blobs.put(blob.blob_key, JSON.stringify(envelope));
  await ok(path + "/lease", "POST", bob.token, { operation: "release", fence: b.lease.fence });
  const expired = await ok(path + "/exchange", "POST", alice.token, { accountId: bob.account.id, version: 1 });
  const expiredDigest = createHash("sha256").update(expired.ticket).digest("hex");
  await db!`UPDATE exchange_tickets SET expires_at=clock_timestamp() - interval '1 second' WHERE digest=${expiredDigest}`;
  expect((await call("/exchange/redeem", "POST", bob.token, { ticket: expired.ticket, teamId: team.id })).status).toBe(404);
  const { ticket } = await ok(path + "/exchange", "POST", alice.token, { accountId: bob.account.id, version: 1 });
  expect((await call("/exchange/redeem", "POST", alice.token, { ticket, teamId: team.id })).status).toBe(404);
  const race = await Promise.all([1, 2].map(() => call("/exchange/redeem", "POST", bob.token, { ticket, teamId: team.id })));
  expect(race.map(reply => reply.status).sort()).toEqual([200, 404]);
  const external = await account();
  const destination = await ok("/teams", "POST", external.token, { name: "Destination team" });
  const unregistered = `${randomUUID()}@slide.test`;
  expect(await call(path + "/exchange", "POST", alice.token, { email: alice.account.email.toUpperCase(), version: 1 }))
    .toEqual({ status: 400, data: { error: "Choose a different recipient." } });
  const missingInvite = await ok(path + "/exchange", "POST", alice.token, { email: unregistered, version: 1 });
  expect((await call(path + "/exchange", "POST", alice.token, { email: external.account.email, accountId: external.account.id, version: 1 })).status).toBe(400);
  const externalInvite = await ok(path + "/exchange", "POST", alice.token, { email: external.account.email.toUpperCase(), version: 1 });
  expect(missingInvite).toEqual({ ticket: expect.stringMatching(/^[A-Za-z0-9_-]{43}$/), expiresAt: expect.any(String) });
  expect(Object.keys(missingInvite)).toEqual(Object.keys(externalInvite));
  expect((await call("/exchange/redeem", "POST", external.token, { ticket: missingInvite.ticket, teamId: destination.id })).status).toBe(404);
  expect((await ok("/exchange/redeem", "POST", external.token, { ticket: externalInvite.ticket, teamId: destination.id })).teamId).toBe(destination.id);
  const newRecipient = await account(unregistered);
  const newTeam = await ok("/teams", "POST", newRecipient.token, { name: "New recipient team" });
  expect((await ok("/exchange/redeem", "POST", newRecipient.token, { ticket: missingInvite.ticket, teamId: newTeam.id })).teamId).toBe(newTeam.id);
  await ok(path + "/transfer", "POST", alice.token, { accountId: bob.account.id });
  expect((await call(path + "/snapshots/1", "GET", alice.token)).status).toBe(404);
  const c = await ok(path + "/lease", "POST", bob.token, { operation: "acquire" });
  const restored = await ok(path + "/restore", "POST", bob.token, { version: 1, generation: 1, requestId: randomUUID(), fence: c.lease.fence });
  expect(restored).toMatchObject({ version: 2, restoredFrom: 1, sha256 });
  expect((await ok(path + "/snapshots", "GET", bob.token)).map((row: { version: number }) => row.version)).toEqual([2, 1]);
});
