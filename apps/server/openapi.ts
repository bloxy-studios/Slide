import { API_SCOPES, type ApiScope } from "../../packages/schemas/public";

const uuid = { type: "string", format: "uuid" };
const string = { type: "string" };
const integer = { type: "integer" };
const json = (schema: object) => ({ "application/json": { schema } });
const path = (name: string, schema = uuid) => ({ name, in: "path", required: true, schema });
const header = (description: string) => ({ description, schema: integer });
const rateHeaders = {
  "RateLimit-Limit": header("Requests allowed per minute"),
  "RateLimit-Remaining": header("Requests left in this window"),
  "RateLimit-Reset": header("Seconds until reset"),
};
const error = (description: string) => ({
  description, content: json({ type: "object", required: ["error"], properties: { error: string } }),
});
const object = (properties: object, required: string[] = []) => ({ type: "object", properties, required });
const array = (items: object) => ({ type: "array", items });
const workspace = object({ id: uuid, teamId: uuid, name: string, role: string, generation: integer });
const envelope = object({ version: { const: 1 }, salt: string, nonce: string, ciphertext: string },
  ["version", "salt", "nonce", "ciphertext"]);
const revision = object({ version: integer, sha256: string, size: integer,
  createdAt: { type: "string", format: "date-time" }, restoredFrom: { type: ["integer", "null"] } });
const tokenMetadata = { id: uuid, name: string, scopes: array({ enum: API_SCOPES }), expiresAt: { type: "string", format: "date-time" } };
const fence = { type: "string", pattern: "^[0-9]+$" };
const leaseResult = object({ generation: integer, heldByThisDevice: { type: "boolean" },
  lease: { anyOf: [object({ holderId: uuid, fence, generation: integer, expiresAt: string }), { type: "null" }] } });

function operation(summary: string, scope: ApiScope | "session", result: object,
    parameters: object[] = [], input?: object) {
  return {
    summary,
    security: scope === "session" ? [{ session: [] }] : [{ apiToken: [] }],
    ...(scope === "session" ? {} : { "x-required-scope": scope }),
    ...(parameters.length > 0 ? { parameters } : {}),
    ...(input ? { requestBody: { required: true, content: json(input) } } : {}),
    responses: {
      "200": { description: "Success", content: json(result), ...(scope !== "session" ? { headers: rateHeaders } : {}) },
      "400": error("Invalid input"), "401": error("Missing, expired or revoked credential"),
      "403": error("Insufficient scope or role"), "404": error("Resource unavailable"),
      "409": error("Resource conflict"), "429": { ...error("API token rate limit exceeded"),
        headers: { ...rateHeaders, "Retry-After": header("Seconds until retry") } },
    },
  };
}

export const openapi = {
  openapi: "3.1.0",
  info: { title: "Slide Public API", version: "1.0.0",
    description: "Team-bound API tokens inherit their issuer's current roles and grants. Session-only routes manage tokens." },
  servers: [{ url: "/v1" }],
  components: { securitySchemes: {
    apiToken: { type: "http", scheme: "bearer", description: "Scoped sw_ token supplied in Authorization." },
    session: { type: "http", scheme: "bearer", description: "Slide account session supplied in Authorization." },
  } },
  paths: {
    "/teams": { get: operation("List the token's team", "teams:read", array(object({ id: uuid, name: string, role: string }))) },
    "/teams/{teamId}/members": { get: operation("List team members", "teams:read",
      array(object({ id: uuid, email: string, role: string })), [path("teamId")]) },
    "/teams/{teamId}/audit": { get: operation("List recent team activity (owner or admin)", "audit:read",
      array(object({ id: string, actorId: uuid, apiTokenId: { type: ["string", "null"] }, subjectId: uuid, action: string, createdAt: string })),
      [path("teamId")]) },
    "/teams/{teamId}/tokens": {
      get: operation("List your team tokens", "session", array(object(tokenMetadata)), [path("teamId")]),
      post: operation("Issue a token; the secret is returned once", "session", object({ ...tokenMetadata, token: string }),
        [path("teamId")], object({ name: string, scopes: array({ enum: API_SCOPES }),
          expiresInDays: { type: "integer", minimum: 1, maximum: 90, default: 30 } }, ["name", "scopes"])),
    },
    "/teams/{teamId}/tokens/{tokenId}": { delete: operation("Revoke your team token", "session",
      object({ revoked: { const: true } }), [path("teamId"), path("tokenId")]) },
    "/workspaces": {
      get: operation("List granted workspaces in stable ID order", "workspaces:read",
        object({ items: array(workspace), next: { type: ["string", "null"] } }),
        [{ name: "after", in: "query", schema: uuid }, { name: "limit", in: "query", schema: { ...integer, minimum: 1, maximum: 100, default: 50 } }]),
      post: operation("Create a shared workspace", "workspaces:write", workspace,
        [{ name: "Idempotency-Key", in: "header", required: true, schema: uuid }],
        object({ teamId: uuid, name: string }, ["teamId", "name"])),
    },
    "/workspaces/{workspaceId}/lease": {
      get: operation("Read the current lease", "workspaces:read", leaseResult, [path("workspaceId")]),
      post: operation("Acquire, renew, release or take over a lease", "leases:write", leaseResult, [path("workspaceId")],
        object({ operation: { enum: ["acquire", "heartbeat", "release", "takeover"] }, fence }, ["operation"])),
    },
    "/workspaces/{workspaceId}/snapshots": {
      get: operation("List recent snapshot revisions", "workspaces:read", array(revision), [path("workspaceId")]),
      post: operation("Publish an encrypted snapshot", "snapshots:write", revision, [path("workspaceId")],
        object({ requestId: uuid, generation: integer, sha256: string, fence, envelope },
          ["requestId", "generation", "sha256", "fence", "envelope"])),
    },
    "/workspaces/{workspaceId}/snapshots/{version}": { get: operation("Read an encrypted snapshot", "workspaces:read",
      object({ version: integer, sha256: string, envelope }), [path("workspaceId"), { ...path("version"), schema: integer }]) },
    "/workspaces/{workspaceId}/restore": { post: operation("Restore a revision as workspace owner", "snapshots:write",
      revision, [path("workspaceId")], object({ requestId: uuid, generation: integer, fence, version: integer },
        ["requestId", "generation", "fence", "version"])) },
  },
};
