# Milestone E — API, local automation, and sandbox adapters

## Public API v1

The sync service publishes the [OpenAPI 3.1 document](http://127.0.0.1:4321/v1/openapi.json). It defines the supported scoped routes for teams, members, workspaces, leases, encrypted snapshots, revision restore, and audit. Existing desktop session routes remain unchanged.

Use an account session to call `POST /v1/teams/{teamId}/tokens` with, for example, `{"name":"QA read","scopes":["workspaces:read"],"expiresInDays":7}`. The response includes a token exactly once. List token metadata at the same path with GET. Revoke a token with `DELETE /v1/teams/{teamId}/tokens/{tokenId}`; to rotate, issue a replacement and revoke the old token. Removing an account from the team revokes its team tokens.

```sh
# Set SLIDE_API_TOKEN without putting its value in your command line or shell history.
printf 'Authorization: Bearer %s\n' "$SLIDE_API_TOKEN" |
  curl --fail-with-body -H @- 'http://127.0.0.1:4321/v1/workspaces?limit=10'
```

Use `next` from the response as `after` in the next workspace list request. API token workspace creation requires a UUID `Idempotency-Key`; retry with the same body and key. Tokens never bypass roles or grants and can't be used to administer tokens. The service stores SHA-256 digests of random credentials. Each token has a PostgreSQL-backed fixed UTC minute window (120 requests by default); authenticated scoped denials also count. Back off on HTTP 429 according to `Retry-After`. Audit records for API mutations include the token ID, not the secret.

Loopback is the sync server's default. An operator making it network-accessible needs TLS and infrastructure-level request limits, including for invalid credentials and account/session traffic. Folders, tags, templates, connections, tasks, exchange and deletion/bulk-delete are not available through scoped tokens. Member, token, revision and audit lists are capped rather than cursor-paginated.

## Installed desktop automation

Open **Automation**, choose a port (default 4330, or 0 for an available port), then **Enable bridge**. Copy the token when first shown or rotate it to get a new one. The listener is bound to exactly `127.0.0.1`, compares the request's Host with the bound address, rejects requests carrying Origin, and requires the token in the Authorization header. It is disabled at app startup. Its authority is the current Slide local catalog account under this OS user; disable it before handing an unlocked manager to another user.

```sh
bridge() { printf 'Authorization: Bearer %s\n' "$BRIDGE_TOKEN" | curl --fail-with-body -H @- "$@"; }
base=http://127.0.0.1:4330/v1/workspaces
bridge "$base"
# Set workspace to the UUID returned by the list command.
bridge -X POST "$base/$workspace/open"
bridge "$base/$workspace"
bridge -X POST "$base/$workspace/focus"
bridge -X POST "$base/$workspace/close"
```

Open and close acknowledge the request; poll the workspace status to confirm the asynchronous transition. The Automation panel displays the last 30 authenticated operations/settings events for the active local account. An invalid token cannot modify the catalog, including its audit trail.

CDP is off by default. Per-workspace opt-in permits a **cold launch via the bridge** in bundled Chromium to start a loopback debugging port; status can return `cdpEndpoint` for the verified process. The bridge token does not protect this separate Chromium port from other local processes. Use a dedicated QA workspace and close its browser to stop CDP. Normal UI launches don't start a debugging port. Disabling the bridge stops its HTTP listener, not the opted-in browser.

| Integration | Status |
|---|---|
| HTTP bridge with curl | macOS arm64 installed-app evidence below |
| CDP via bundled Chromium | Loopback discovery verified on macOS arm64; no claim for installed Chrome or other OSes |
| Playwright / Puppeteer / Selenium | Not verified |
| Disposable auto-cleaned profiles / headless | Not implemented |

### macOS arm64 installed-app check

From the local DMG, an installed copy ran with an isolated QA catalog and the bundled browser. The listener check reported `TCP 127.0.0.1:4330 (LISTEN)`. The first curl run returned HTTP 401 for an invalid token and HTTP 403 for a different Host. Authenticated list and status showed the fixture workspace changing from `closed` to `running` with no `cdpEndpoint` while opted out.

After opting in through the desktop UI, a second curl open cold-launched the fixture. Status returned an endpoint bound to `127.0.0.1`; Chromium's `/json/version` returned `{"Browser":"Chrome/157.0.8103.78","Protocol-Version":"1.3"}`. Curl focus and close returned `{"accepted":true}`, status returned `closed` with no endpoint, and the CDP listener exited. The isolated QA home raised a macOS Keychain prompt; it was dismissed before the successful close check. No credential persistence claim was tested.

[Automation controls and recent audit](evidence/mac-e-automation-audit.png)

After disabling the bridge in the app, curl got connection refused on port 4330.

## Provider-only / sandbox

`apps/server/sandbox.ts` defines billing and network provider interfaces and deterministic in-memory fakes. Sample plan prices and limits are developer inputs, not offers. Tests exercise signed event delivery, upgrade/downgrade, failed renewal, stale and duplicate delivery, quote acceptance, and purchase retries without double-deductions.

No live provider, charge, order, webhook receiver, user-facing checkout, persistent usage ledger, or seat/profile quota is enabled. The sandbox is not connected to real accounts or exposed as a purchase API; live provider operations are unavailable.
