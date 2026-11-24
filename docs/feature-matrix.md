# Feature matrix — Milestone A

Mac results refer to the arm64 runner unless stated. The [reference inventory](https://docs.octobrowser.net/en/llms.txt) is a competitor comparison, not a claim of compatibility or affiliation. The following rows reflect only the milestone in this branch.

| ID | User-visible behavior | Competitor source | Scope | Implementation path | Mac result | Windows result | Later platform | Evidence | Remaining gap |
|---|---|---|---|---|---|---|---|---|---|
| F01a | Local manager launches installed Chrome/Chromium | [Installation](https://docs.octobrowser.net/en/start/installation-and-registration/) | A | desktop runtime adapters | verified with Chrome | implemented / untested on target | Intel, Linux | T01 | Chromium not separately demonstrated, no bundled browser |
| F01b | Separate runtime update/catalog across devices | [Octium](https://docs.octobrowser.net/en/changelog/octium-changelog/) | D/F | — | partial | partial | Linux | T01 | Installed browser updates itself; no shared catalog |
| F02 | Accounts and device sessions | [Account](https://docs.octobrowser.net/en/start/account-settings/) | D | — | partial | partial | mobile | — | No product account system; fixture roles are not app accounts |
| F03 | Create a named workspace and optional start URL | [Create](https://docs.octobrowser.net/en/profiles/start/) | A/B | desktop manager and migration | partial | implemented / untested on target | — | T02 | No folders, tags or templates |
| F04a | Local open, focus, close and reattach | [Launch](https://docs.octobrowser.net/en/profiles/start/) | A | desktop manager and runtime adapters | verified | implemented / untested on target | — | T02/T03 | Reboot and Windows acceptance pending |
| F04b | Shared lease and force-stop | [Manage](https://docs.octobrowser.net/en/profiles/edit-profiles/) | D | — | partial | partial | — | — | No shared leases |
| F05 | Profile list, actions and status | [Profile list](https://docs.octobrowser.net/en/profiles/profiles-page/) | A/B | desktop UI | partial | implemented / untested on target | — | T02/T03 | No search, filters, sort or column settings |
| F06 | Folders, tags and grants | [Folders](https://docs.octobrowser.net/en/profiles/folders/) | B/D | — | partial | partial | — | — | Not started |
| F07 | Templates and settings-only clone | [Templates](https://docs.octobrowser.net/en/profiles/profile-templates/) | B/D | — | partial | partial | — | — | Not started |
| F08 | Bulk creation and edits | [Bulk create](https://docs.octobrowser.net/en/profiles/profiles-in-bulk/) | B | — | partial | partial | — | — | Not started |
| F09 | Tasks and reminders | [Manage](https://docs.octobrowser.net/en/profiles/edit-profiles/) | B | — | partial | partial | — | — | Not started |
| F10 | Start pages, browser bookmarks and accessibility | [Profile settings](https://docs.octobrowser.net/en/profiles/browser-profile-settings/) | A/B | desktop UI, runtime and fixture | partial | implemented / untested on target | — | T01–T03 | Single URL; reusable bookmarks not started |
| F11 | Resource and launch controls | [Profile settings](https://docs.octobrowser.net/en/profiles/browser-profile-settings/) | B/C/F | — | research only | research only | — | — | No extra arbitrary browser flags or focus emulation |
| F12 | Cloud browser storage categories | [Storage settings](https://docs.octobrowser.net/en/profiles/browser-profile-settings/) | D | Native local data directories only | partial | implemented / untested on target | — | T02 | No interchange or cloud fidelity |
| F13 | Sync and historical versions | [Restore](https://docs.octobrowser.net/en/profiles/restore-profile-versions/) | D | — | partial | partial | — | — | Not started |
| F14 | Trash and recovery | [Trash](https://docs.octobrowser.net/en/profiles/trash-bin/) | B/D | — | partial | partial | — | — | Not started |
| F15 | Profile password | [Manage](https://docs.octobrowser.net/en/profiles/edit-profiles/) | D | — | partial | partial | — | — | Not started |
| F16 | Ownership transfer and portable export | [Exchange](https://docs.octobrowser.net/en/profiles/export-import-octo-browser-profile/) | D | — | partial | partial | — | — | Not started |
| F17 | Cookie import/export and supported attributes | [Cookie tools](https://docs.octobrowser.net/en/cookies/cookie-upload/) | C | Synthetic fixture cookies only | partial | partial | — | T02 | No import/export |
| F18 | Extension management | [Extensions](https://docs.octobrowser.net/en/profiles/extensions/) | B/C | — | research only | research only | — | — | No managed catalog; third-party integration is research |
| F19 | Per-workspace connection | [Network](https://docs.octobrowser.net/en/category/proxy-settings/) | C | Browser default networking | partial | partial | — | — | No per-profile routing verification |
| F20 | Connection management | [Proxy manager](https://docs.octobrowser.net/en/proxy/proxy-manager/) | C/E | — | partial | partial | — | — | Not started |
| F21 | Network provider purchasing | [Proxy shop](https://docs.octobrowser.net/en/proxy/internal-proxy/) | E | — | blocked with reason | blocked with reason | — | — | No provider or sandbox adapter yet |
| F22 | Transparent first-party device QA | [Profile settings](https://docs.octobrowser.net/en/profiles/browser-profile-settings/) | C/research | — | research only | research only | mobile | — | No QA presets; evasion out of scope |
| F23 | Transparent QA storage, input and video | [Cookie Robot](https://docs.octobrowser.net/en/profiles/cookie-robot/) | C/research | fixture storage markers | partial | partial | — | T02 | No text/video diagnostics; third-party simulation excluded |
| F24 | Versioned, authorized management API | [API](https://docs.octobrowser.net/en/api/start-api/) | E | apps/server reserved | partial | partial | — | — | No API |
| F25 | Local runtime API and authorized QA examples | [API](https://docs.octobrowser.net/en/api/start-api/) | E/F | internal desktop IPC only | partial | partial | Linux/mobile | — | No external bridge or automation compatibility claim |
| F26 | Team access and organization permissions | [Team](https://docs.octobrowser.net/en/start/team-settings/) | D | — | partial | partial | mobile | — | No organization API |
| F27 | Actor audit and diagnostics | [Account](https://docs.octobrowser.net/en/start/account-settings/) | D/F | runtime summary and row errors | partial | implemented / untested on target | — | T03 | No audit, diagnostics export or update health |
| F28 | Mobile profile catalog with platform restrictions | [Mobile visibility](https://docs.octobrowser.net/en/mobile/profiles/profiles-list/) | F | packages/schemas reserved | later platform | later platform | iOS/Android | — | No mobile app or capability schema |
| F29 | Mobile storage, account and automation | [Mobile settings](https://docs.octobrowser.net/en/mobile/profiles/profiles-settings/) | F | — | later platform | later platform | iOS/Android | — | No mobile tests |
| F30 | Billing and entitlements | [Subscriptions](https://docs.octobrowser.net/en/payments/subscriptions/) | E | — | blocked with reason | blocked with reason | mobile | — | No provider or sandbox adapter yet |
| F31 | Support and ecosystem | [Product site](https://octobrowser.net/) | F | README and acceptance docs | partial | partial | — | T01 | Support UI, update strategy and full runbooks pending |

See [the acceptance log](milestone-a.md) and [runner inventory](runners.md) for procedures, observed results, and evidence.
