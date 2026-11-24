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
async function account() {
  const email = `${randomUUID()}@slide.test`;
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
  expect((await call(path + "/exchange", "POST", alice.token, { email: "missing@slide.test", version: 1 })).status).toBe(400);
  expect((await call(path + "/exchange", "POST", alice.token, { email: external.account.email, accountId: external.account.id, version: 1 })).status).toBe(400);
  const externalInvite = await ok(path + "/exchange", "POST", alice.token, { email: external.account.email.toUpperCase(), version: 1 });
  expect((await ok("/exchange/redeem", "POST", external.token, { ticket: externalInvite.ticket, teamId: destination.id })).teamId).toBe(destination.id);
  await ok(path + "/transfer", "POST", alice.token, { accountId: bob.account.id });
  expect((await call(path + "/snapshots/1", "GET", alice.token)).status).toBe(404);
  const c = await ok(path + "/lease", "POST", bob.token, { operation: "acquire" });
  const restored = await ok(path + "/restore", "POST", bob.token, { version: 1, generation: 1, requestId: randomUUID(), fence: c.lease.fence });
  expect(restored).toMatchObject({ version: 2, restoredFrom: 1, sha256 });
  expect((await ok(path + "/snapshots", "GET", bob.token)).map((row: { version: number }) => row.version)).toEqual([2, 1]);
});
