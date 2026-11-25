# Platform capabilities and compatibility

“Built” means the artifact exists, not that the feature ran. “Not exercised” is not a support guarantee. See the [packaging matrix](packaging.md) for exact versions and [acceptance evidence](acceptance.md) for installed coverage.

| Capability | Mac arm64 native | Mac Intel native / via Rosetta | Windows x64 native | Linux / container | Mobile / Windows arm64 |
|---|---|---|---|---|---|
| Local catalog and browser lifecycle | Verified in an installed app | Native build only; translated build not run | Milestone A installed lifecycle, latest installer only built | Later-platform | Later-platform |
| Seeded runtime settings | Selected overrides verified; not all surfaces | Blocked: no compatible patched bundle; language and WebRTC flags only | Patched bundle built; actual overrides untested | Later-platform | Later-platform |
| Direct and required HTTP proxy | Direct and required HTTP verified with an earlier arm64 ungoogled bundle; required HTTP not reverified with patched default | Required HTTP blocked by runtime gate; Direct not exercised | Required HTTP blocked by runtime gate; Direct not exercised | Later-platform | Later-platform |
| Local loopback automation | Curl bridge and opt-in CDP verified on earlier bundle | Built, not exercised | Built, not exercised | Later-platform | Later-platform; no mobile CDP |
| Shared metadata via API | Installed lease denial, handoff and metadata restore verified | Desktop flow untested | Desktop flow untested | Server is separate; containerized deployment unverified | Shared schemas only; no client |
| Native browser data portability | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| Support export | See [installed run](acceptance.md) | Build-only | Build-only | Later-platform | Later-platform |

The effective capability model keeps **visibility, access and launchability separate**. A caller can list a shared workspace only if the server's role/grant checks permit it. A desktop installation can launch a **local** workspace only with a compatible native runtime and an eligible connection. A recorded brand/minimum version or required route must not be bypassed; a shared lease is not a browser-process lock. Future clients may list authorized desktop metadata without offering a launch or CDP action. There is no mobile or Linux client consuming those rules yet.

| Compatibility case | Expected result | Project conformance evidence |
|---|---|---|
| Existing Chrome workspace, bundled Chromium now selected | Keep the saved brand instead of replacing the profile runtime | [Manager runtime tests](../apps/desktop/src-tauri/src/manager.rs) |
| Existing browser data needs a newer runtime | Block the downgrade instead of rewriting the directory | [Manager runtime tests](../apps/desktop/src-tauri/src/manager.rs) |
| Workspace requires HTTP proxy on unsupported platform/runtime | Refuse required-proxy launch | [Runtime gate](../apps/desktop/src-tauri/src/runtime/mod.rs); [cold-block evidence](evidence/mac-c-proxy-cold-block.png) tests the separate failed-proxy case on a supported platform |
| API member can see one shared workspace but has no grant on another | Deny unauthorized access regardless of whether a desktop client could launch | [Server grant tests](../apps/server/api.test.ts); [versioned contracts](../packages/schemas/README.md) |

Backlog: native Linux and mobile clients; defined per-platform browser data stores and explicit action gates in those clients; mobile billing and automation; tested Linux headless/container execution. No new capabilities are implied by the shared schemas.
