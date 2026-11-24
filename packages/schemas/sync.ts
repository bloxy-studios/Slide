export const SYNC_API = "/v1";
export const ENVELOPE_VERSION = 1;
export const MAX_ENVELOPE_BYTES = 256 * 1024;
export const KDF_ITERATIONS = 600_000;
export const TEAM_ROLES = ["admin", "member"] as const;
export const WORKSPACE_ROLES = ["reader", "operator"] as const;
export type TeamRole = "owner" | typeof TEAM_ROLES[number];
export type WorkspaceRole = "owner" | typeof WORKSPACE_ROLES[number];
export type Lease = { holderId: string; fence: string; expiresAt: string; generation: number };
export type RemoteWorkspace = { id: string; teamId: string; name: string; role: WorkspaceRole; generation: number };
export type Revision = { version: number; sha256: string; size: number; createdAt: string; restoredFrom: number | null };
export type Envelope = { version: 1; salt: string; nonce: string; ciphertext: string };
export type WorkspaceMetadata = {
  version: 1;
  name: string;
  icon: string;
  notes: string;
  launchUrls: string[];
  bookmarks: { name: string; url: string }[];
  runtime: { brand: string; version: string } | null;
};

export function isEnvelope(input: unknown): input is Envelope {
  if (!input || typeof input !== "object") return false;
  const { version, salt, nonce, ciphertext } = input as Partial<Envelope>;
  return Object.keys(input).length === 4 && version === ENVELOPE_VERSION &&
    typeof salt === "string" && /^[A-Za-z0-9+/]{22}==$/.test(salt) &&
    typeof nonce === "string" && /^[A-Za-z0-9+/]{16}$/.test(nonce) &&
    typeof ciphertext === "string" && ciphertext.length >= 24 && ciphertext.length % 4 === 0 &&
    /^[A-Za-z0-9+/]+={0,2}$/.test(ciphertext);
}
