# Milestone B: local workspace management

Run `bun install --frozen-lockfile` and `bun run dev` from the repository root. To build a Mac installer, run `bun run --cwd apps/desktop tauri build --ci --bundles app,dmg`. The app opens the installed Chrome or Chromium in a normal browser window using a separate data directory for each workspace.

## Local behavior

- Quick workspace applies the selected default template, if any; New workspace takes explicit settings; From template uses the template you select. A settings clone gets a new profile ID and does not copy browser files, cookies, passwords, history or tabs.
- The Library manages folders, tags, templates and reusable bookmarks. Removing a folder unfiles the affected workspaces without deleting them. Open an assigned bookmark from the workspace's **Manage** menu. These shortcuts are stored in Slide, not synced into Chrome's native bookmarks.
- Use **Command+K** on Mac or **Ctrl+K** on Windows to focus search. Search, filters, sort, page size and named column selections are saved per local account. Page selections remain explicit across pages until cleared. A local account is an on-device catalog, not an online login or OS security boundary. Switching requests closure of the outgoing account's browsers; wait for closure and retry if prompted.
- Reminders store UTC epoch seconds and display local time. A due task is recorded once even if it was missed while the manager was closed. Open the Reminders section to see it and snooze or complete it. No background notifier runs while the app is closed.
- Move closed workspaces to Trash to retain their browser files. Restore from Trash or confirm permanent removal. Locked or running profiles cannot be trashed or purged.
- Bulk operations show row results. Cancel stops queued work; an operation that already began can finish. Opens are serialized and each waits for its window or timeout before the next starts; other eligible actions use at most two workers. **Test local HTTP** probes the fixture from the manager. It does not claim to test a browser's proxy, credentials, or route.

## Import files

Start with [the non-sensitive example](workspace-import-example.csv). Use a case-insensitive header named `name`; optional headers are `start_url`, `launch_urls` (one URL per line inside a single quoted cell), `icon` and `notes`. Extra headers are ignored. Save CSV as UTF-8; TSV and TXT use tabs and the same quoting rules. XLSX reads the first sheet as values; formulas, defined names, external relationships and external workbook parts are rejected.

The app accepts at most 2 MiB per file, 1,000 data rows, 32 columns and at most 16 MiB of expanded workbook data. Preview reports valid rows, validation errors, duplicates (case-insensitive, including Trash), and already imported rows. Select valid rows and explicitly start import. Retrying an identical file in the same account skips rows already committed, including after restart; a modified file gets a new fingerprint and is still subject to duplicate name checks.

## Extensions

Add a Web Store listing, stage the fixture badge, or stage a custom Manifest V3 ZIP with `manifest.json` at its root. ZIP limits are 8 MiB compressed, 24 MiB expanded and 512 entries. Unsafe paths and symlinks are rejected. CRX staging and silent installation are unavailable. Store metadata is locally recorded, not fetched from or verified against the store.

Assignments persist on workspaces and templates and are distinct from browser installation state. Approve a Store install in the target browser. For a staged package, open `chrome://extensions` in that workspace, enable Developer mode if permitted, click **Load unpacked**, and select the staged directory. Disable or remove it in the same browser. Staging an update does not alter the browser; remove the old copy and load the newly staged directory. No runtime restrictions are bypassed. The fixture extension draws a versioned badge only on loopback fixture ports 4310 and 4311.

## Installed Mac acceptance

macOS Tahoe 26.6.2 (arm64), Google Chrome 154.0.8037.93. Installed the QA bundle from its DMG into the build directory, with an alternate app identifier and window title to leave the existing Milestone A installation and its catalog untouched. The QA bundle and normal release package were built from the same source. macOS displayed an app-modification notification on Chrome launch; no permissions or protections were changed.

| Flow | Observed result | Evidence |
|---|---|---|
| Import and retry | Previewed 100 CSV data rows: 98 valid, one duplicate and one rejected URL. Imported 98 selected rows. Both the workspace table and retry ledger had 98 entries; reopening the same file displayed Already imported for previously committed rows. | [Preview](evidence/mac-b-import-preview.png), [Retry](evidence/mac-b-import-retry.png) |
| Settings-only clone | Opened the original in Chrome, signed in as synthetic Creator, and cloned from the manager. The clone inherited the launch URL but opened with no cookie, localStorage, IndexedDB or separately installed Chrome extension. | [Original session](evidence/mac-b-fixture-badge.png), [Fresh clone](evidence/mac-b-clone-no-session.png) |
| Fixture extension | Catalog showed permissions, source, version and manual capability. Chrome loaded v1.0.0 via Load unpacked; disabling then reloading removed the badge without signing the Creator out. Staging v1.0.1 in the manager and approving the replacement in Chrome changed the fixture badge to v1.0.1. | [Catalog](evidence/mac-b-extension-catalog.png), [Enabled](evidence/mac-b-fixture-badge.png), [Disabled](evidence/mac-b-fixture-badge-disabled.png), [Updated](evidence/mac-b-fixture-badge-updated.png) |
| Missed reminder | Saved a reminder linked to two workspaces and quit before it was due. After relaunch, the app displayed one due reminder and both workspace names. SQLite stored the local 07:50 deadline as 15:50 UTC and recorded the fire timestamp at 15:50:12 UTC. Snoozing set a new deadline and cleared the due state; completing then persisted the completion timestamp. | [Due after restart](evidence/mac-b-reminder-due.png) |

Mac release DMG: `apps/desktop/src-tauri/target/release/bundle/dmg/Slide Workspaces_0.1.0_aarch64.dmg`. SHA-256: `1cc72e69d6e7575940b113591c41545bb0ec96b6ccfec0799f3343ed416c7732`. The app is arm64, ad hoc signed and not notarized. `hdiutil verify` reported its checksum valid.
