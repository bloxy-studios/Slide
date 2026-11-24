export const API_SCOPES = ["teams:read", "workspaces:read", "workspaces:write", "leases:write", "snapshots:write", "audit:read"] as const;
export type ApiScope = typeof API_SCOPES[number];

export function requiredScope(method: string, resource: string, id?: string, action?: string, sub?: string): ApiScope | undefined {
  if (resource === "teams" && method === "GET") {
    if (!id || (action === "members" && !sub)) return "teams:read";
    if (action === "audit" && !sub) return "audit:read";
  }
  if (resource !== "workspaces") return;
  if (!id && method === "POST") return "workspaces:write";
  if (method === "GET" && (!id || action === "snapshots" || (action === "lease" && !sub))) return "workspaces:read";
  if (id && !sub && method === "POST") {
    if (action === "lease") return "leases:write";
    if (action === "snapshots" || action === "restore") return "snapshots:write";
  }
}
