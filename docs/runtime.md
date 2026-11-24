# Bundled Chromium development installers

Slide's development installers open the included Chromium by default. The manager displays its version, architecture and location. **Choose another browser** lets you explicitly select installed Chrome or Chromium; **Use bundled Chromium** clears that override. Source-mode development (`bun run dev`) still discovers installed browsers.

Use the [bundle command](../README.md#build-a-development-installer), not `tauri build` alone. It fetches the target's pinned ZIP over HTTPS, hashes it against the SHA-256 in [bundle.ts](../apps/desktop/bundle.ts), and verifies cached ZIPs before extracting. CI uses the same command. The repository does not contain browser binaries.

| Target | Upstream snapshot | Archive |
|---|---|---|
| macOS arm64 | 1753271 | [chrome-mac.zip](https://storage.googleapis.com/chromium-browser-snapshots/Mac_Arm/1753271/chrome-mac.zip) |
| macOS x86_64 | 1753296 | [chrome-mac.zip](https://storage.googleapis.com/chromium-browser-snapshots/Mac/1753296/chrome-mac.zip) |
| Windows x64 | 1753232 | [chrome-win.zip](https://storage.googleapis.com/chromium-browser-snapshots/Win_x64/1753232/chrome-win.zip) |

These are unbranded [Chromium Project continuous builds](https://www.chromium.org/getting-involved/download-chromium/), selected because they come from the upstream project and provide a full browser for each target. They are best-effort snapshots, not Google Chrome Stable; their revisions may differ across targets. Playwright's current desktop Chromium download actually uses Chrome for Testing, which is for testing, not a redistribution choice for general browsing. Community ungoogled-Chromium releases have separate macOS and Windows maintainers, release timing and patches.

The installers include the [Chromium license notice](chromium-license.txt); third-party notices are included in Chromium's `chrome://credits`. macOS includes the nested browser and framework in `Contents/Frameworks` and signs the browser and the manager ad hoc. Windows uses the Tauri resources directory for NSIS and MSI; Chromium's upstream test executable and standalone setup program are omitted. Neither platform has a production distribution signature; the Mac DMG is not notarized.

### Profile behavior

- Each workspace still uses its own app-owned `--user-data-dir` and opens a normal, non-kiosk browser window with the configured launch URLs. Slide seeds the Chromium profile name in `Default/Preferences` if the workspace data directory is empty; it does not overwrite existing Preferences or Chromium-managed Local State. A settings-only clone gets a fresh UUID and browser directory.
- Continue to use the browser's normal, user-approved extension flow. No silent installation, arbitrary launch arguments, or proxy settings are added by this bundle. Per-workspace proxy configuration remains gated on verifying the route and fail-closed behavior.
- An installed browser override is app-wide for new profiles. Existing profiles keep using their recorded browser brand or block if no compatible copy is available. Slide can identify a legacy profile from an already-running verified session; if it cannot identify a nonempty legacy profile, it blocks rather than risk its saved sessions. Back up the manager's data before upgrading. Browser-version downgrades and Chrome/Chromium encrypted storage migration are not supported.

### Updates

The browser does not self-update. Pins and hashes must be refreshed together, following upstream security releases, with an installed-app smoke test on each supported target. A development snapshot and an ad hoc signature are not a production update or security support policy.

### macOS arm64 installed-app check

The QA app was copied out of its DMG under a separate application ID so existing profiles were not modified. The [manager](evidence/mac-bundled-manager.png) showed Bundled Chromium 160.0.8182.0 and both workspaces running. Folve Creator and Folve Viewer opened the same loopback fixture in [ordinary Chromium windows](evidence/mac-bundled-creator-viewer.png) with different cookies, localStorage and IndexedDB markers. Closing Viewer from the manager left Creator running; [reopening Viewer](evidence/mac-bundled-viewer-reopened.png) restored its own fixture identity. Both profiles retained the names set by the manager. Chromium showed its normal first-run dialog; the OS default browser was left unchanged.

### Windows installer retest

The [Milestone A Windows report](evidence/windows-milestone-a.md) observed that MSI installation and the Start shortcut worked, but **Finish** with **Launch Slide Workspaces** checked returned 1631. The MSI template now dispatches the checkbox through WiX's interactive shell action using the installed executable path. The log did not establish the original process token or cause. To verify the change, install the new MSI interactively on Windows x64 with logging enabled:

```powershell
msiexec.exe /i "Slide Workspaces_0.1.0_x64_en-US.msi" /L*v "slide-msi-install.log"
```

On a clean install, leave the checkbox checked and click **Finish**. Verify exactly one manager opens in the signed-in desktop session and displays Bundled Chromium. Close it and repeat on a clean install with the box unchecked; verify no manager opens until launched from Start. This needs an interactive Windows run; an installer build alone does not verify it.
