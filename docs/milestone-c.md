# Milestone C — local connections and QA

## Start the controlled test lab

From the repository root, start the fixture with `bun run fixture`. To run just the new lab without taking ports 4310 and 4311, use `bun apps/fixture/server.ts --qa-only`; this mode hides the second-origin link. In another terminal run `bun apps/fixture/proxy.ts --auth`. The proxy listens only on loopback and allows only the lab at `http://127.0.0.1:4320` and the second fixture origin at `http://127.0.0.1:4311` when the full fixture is running. Its optional credentials are synthetic: username and password are both `fixture`. Never use real credentials in the test lab.

In the installed manager, open **Connections**, save `http://127.0.0.1:4312` as **Fixture HTTP**, and select it in **Test controlled fixture route**. A rejected password yields a proxy authentication error; the synthetic credentials yield `Fixture route confirmed: proxy`. The Direct check yields `Fixture route confirmed: direct`. These are manager probes; they do not test Chromium. For CSV use columns `name,endpoint`, at most 100 rows; any invalid row rejects the whole batch.

Create two workspaces with launch URL `http://127.0.0.1:4320/`. Select **Direct** for one and **Fixture HTTP (required)** for the other. You can instead configure an endpoint unique to a workspace. Open both, authenticate in the second Chromium window, and compare the page's **Fixture route** markers. Close the proxy, then request a fresh URL in the proxied window; Chromium should fail rather than showing Direct. The direct window should still reach the fixture. A cold launch with the proxy stopped must show the manager's launch error. Restart the proxy and manager to check that the configured connection persists.

## Capability boundaries

- Workspaces without a connection choice use **System default**, leaving Chromium's proxy settings untouched. Choose **Direct** to bypass the system proxy for that workspace.

- The required HTTP proxy launch path is enabled for bundled Chromium on macOS arm64. The manager checks TCP reachability before starting the browser and never substitutes Direct for a required connection. Chromium applies one explicit HTTP proxy without an implicit loopback exception. This is browser routing, not a system-wide network or DNS isolation guarantee. The controlled fixture verifies HTTP navigation; this does not establish general HTTPS tunneling, arbitrary browser extension/background traffic, or third-party proxy-provider compatibility.
- Proxy endpoints and assignments are local account metadata. They cannot contain credentials. The test form sends temporary synthetic Basic credentials only to a loopback HTTP proxy; Slide does not save them or pass them to child processes. Browser authentication is handled by Chromium's own prompt. HTTP Basic is not suitable for sending secrets to an untrusted network.
- HTTPS proxy transport, SOCKS, SSH, WireGuard, and AmneziaWG launches are blocked pending transport-specific, DNS and failure checks. Required-proxy launch is blocked on Windows, Intel Mac, and installed-browser overrides; no direct fallback is offered.
- **Diagnostics** reports the default browser's local storage capabilities; it is not a sync feature. There is no verified cloud portability or extraction of OS-protected browser data.
- The lab's **QA fixture lab** page shows the synthetic cookie, localStorage and IndexedDB markers and offers JSON/Netscape interchange for the single fixture role cookie. Export explicitly downloads a synthetic file. Import explicitly selects a file and applies it to the current fixture host as a session cookie. JSON requires host-only, HttpOnly, non-Secure and SameSite=Strict attributes; Netscape uses the same contract and reconstructs Strict on import. Persistent or expired, Secure, partitioned, foreign-host, and differently scoped rows are rejected. This is not a general cookie interchange tool.
- The lab's viewport, locale, timezone and geolocation examples affect only its own readout. They do not change values exposed by browser APIs. Text input is a deterministic sample, and the generated video visibly labels itself as synthetic and does not access or substitute for a camera. No third-party test traffic or identity simulation is provided.

## macOS arm64 installed-app evidence

- Bundled ungoogled-chromium showed the [direct](evidence/mac-c-direct.png) and [authenticated proxy](evidence/mac-c-proxy.png) markers in two concurrently open workspaces. The inspected browser argument lists showed the routing flags, with no proxy credentials.
- After the proxy stopped, Chromium showed [`ERR_PROXY_CONNECTION_FAILED`](evidence/mac-c-proxy-outage.png) on a fresh navigation while the direct workspace still loaded the fixture. After restarting the manager from the final DMG, [cold open was refused](evidence/mac-c-proxy-cold-block.png) for the same saved required connection.
- The installed app displayed [runtime and local storage capabilities](evidence/mac-c-diagnostics.png). The browser displayed the fixture's [storage, test input and labeled video](evidence/mac-c-fixture.png).
- Cookie interchange in both formats was exercised in the fixture suite, not through the installed browser's file picker. The isolated QA home had no default Keychain; this run did not verify encrypted cookie persistence.
