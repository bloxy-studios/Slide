# Milestone D — optional shared workspaces

Local Workspaces and local account switching do not require the service. For shared workspaces, start the [development services](../apps/server/README.md) and open **Shared**. Sign-in is separate from the local account. Sign-in tokens, passwords and snapshot passphrases are not saved by the desktop app. Switching local accounts or closing the app ends its in-memory session and heartbeat; the server lease expires if not released.

## Two-user handoff

1. Create two synthetic sync accounts with separate emails, then sign in as the first user. Create a team, invite the second email and share the one-time token privately with the intended recipient. The second user signs in with the matching email and enters the token to accept. Create a shared workspace and grant the second user **operator** access.
2. In the first user's local catalog, create a workspace with synthetic settings. Close its browser if open. In **Shared**, acquire the lease, select the closed local workspace, enter a separate snapshot passphrase and publish. The server assigns a revision only after the encrypted blob is stored and the lease/generation check passes.
3. The second user cannot acquire while the first lease is active. Release it on the first device, then acquire on the second. Select the revision and use **Restore local copy** with the same passphrase. This creates a fresh, independent workspace in the second user's local catalog.
4. When a transfer is needed, use **Transfer ownership** after lease release. The recipient must be a teammate; the previous owner loses access. For a copy instead, issue a single-use, expiring exchange ticket for an existing account by email or account ID and selected version. Use a known account ID when possible because emails are not verified. The recipient need not belong to the source team; they redeem into a team they belong to and need the passphrase separately.

On macOS arm64, two instances of the installed DMG app used separate local catalogs and signed in against the native PostgreSQL/MinIO test stack. The team and operator grant were seeded through the API. Alice created a local fixture, published v1 under fence 1, and released the lease. Bob saw [the active-lease denial](evidence/mac-d-lease-denied.png), acquired fence 2 after release, and [restored v1](evidence/mac-d-restore.png) into [his own local catalog](evidence/mac-d-restored-workspace.png). Installed transfer, exchange, takeover and desktop team setup were not exercised.

## Permissions and v1 routes

| Role | Rights |
|---|---|
| Team owner | Invite members or admins, remove eligible members, revoke invitations and read team audit |
| Team admin | Invite/remove ordinary members, revoke member invitations and read team audit |
| Team member | List teammates, accept their own invitations and create a shared workspace |
| Workspace owner | Read, grant/revoke, acquire/renew/release or force a lease, publish, promote an older revision, transfer and issue exchange tickets |
| Workspace operator | Read, acquire/renew/release their own lease and publish |
| Workspace reader | Read workspace revisions and committed encrypted blobs only |

Team administration does not imply workspace access. Workspace access also requires current team membership. API routes under `/v1` use bearer sessions:

- `POST /accounts`, `POST /sessions`, `DELETE /sessions/current`.
- `GET|POST /teams`, `GET /teams/:id/members`, `DELETE /teams/:id/members/:account`, `POST /teams/:id/invitations`, `DELETE /teams/:id/invitations/:invitation`, `GET /teams/:id/audit`, `POST /invitations/accept` with the issued token.
- `GET|POST /workspaces`, `PUT|DELETE /workspaces/:id/grants/:account`, `GET|POST /workspaces/:id/lease`, `GET|POST /workspaces/:id/snapshots`, `GET /workspaces/:id/snapshots/:version`.
- `POST /workspaces/:id/restore` publishes another revision pointing at the validated older blob, preserving history. `POST /workspaces/:id/transfer`, `POST /workspaces/:id/exchange`, and `POST /exchange/redeem` are distinct operations.

Leases last 90 seconds by default; the desktop refreshes its selected lease every 20 seconds. A force takeover increments the fence and can leave the old device's local browser open. A stale fence cannot publish; an already committed request ID can retrieve the same receipt. Failed requests do not overwrite local state. Desktop upload retry is in memory, and unreferenced objects from a failed commit are not yet collected.

## Data and security boundaries

- **Included:** Workspace name, icon, notes, launch URLs, and assigned Slide bookmarks. The encrypted payload also records the source runtime brand and version when known. Shared workspace and team display names, memberships, lease metadata and audit records are visible to the service.
- **Excluded:** Browser directories, cookies/session data, passwords, localStorage, IndexedDB, extensions, history, service workers, OS credentials, connection settings, tags and folders. Restore does not overwrite a local profile or promise Chromium session continuity.
- The desktop encrypts before upload: AES-256-GCM with per-snapshot random salt and nonce, PBKDF2-HMAC-SHA256 at 600,000 iterations and versioned associated data. The service stores ciphertext in private object storage and checks its size and SHA-256; the recipient also checks authenticated decryption. Choose a strong unique passphrase and share it out of band. A weak passphrase can be guessed against copied ciphertext; forgotten passphrases have no recovery. This is not an account recovery or local workspace-lock feature.
- Server passwords use Argon2id, and session, invitation and exchange token digests are stored instead of raw tokens. Email ownership is not verified; an email match alone does not grant team membership. Share invitation tokens out of band with the intended recipient. Tokens expire after 24 hours and can be used once. Old invitations issued without tokens must be reissued. The audit table rejects update, delete and truncate through ordinary SQL. A privileged database administrator can change schema or remove storage; this is not a tamper-evident external log. The example stack is loopback HTTP for development, not production deployment.
- Password reset, TOTP/recovery codes, folder/unfiled grants and one-way folder drops, retention/quotas, cloud trash, browser-state portability and production operations remain incomplete.
