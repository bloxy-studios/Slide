# Slide Workspaces 0.1.0 — development handoff

Start with [packaging and downloads](packaging.md), [platform compatibility](platform-compatibility.md), [feature status](feature-matrix.md), and [acceptance evidence](acceptance.md). Build and source-mode commands are in the [README](../README.md). This is a development handoff, not a signed production release.

## Release notes

- Added **Support** with installed app, OS and default-browser versions, a startup-log location, manager restart and an export preview.
- Diagnostics are a small, versioned JSON report. It includes only allowlisted platform, version and runtime fields. It excludes paths, raw logs, hostnames, profile data, URLs and credentials. Export is local and does not upload anything.
- Browser pins and profile compatibility rules are unchanged. The browser in an existing workspace can differ from the default shown in Support. See [runtime behavior and dependency notices](runtime.md).

## Install, update and remove

- **Mac:** Mount the DMG for the Mac's native architecture, copy Slide Workspaces to Applications, then open it from Finder. Intel and arm64 are separate builds. Follow your organization's normal approval process if a development artifact is blocked. Do not turn off Gatekeeper.
- **Windows x64:** Run the NSIS installer or MSI, then open Slide from Start. WebView2 is required. If MSI Finish does not launch the app, use the Start shortcut and save the install log; see the [historical Windows report](evidence/windows-milestone-a.md).
- **Update:** Slide and the bundled browser do not self-update. Close managed browsers and the manager, back up the data directory, and install the newer native package. Check the pinned browser version before opening existing profiles. Do not downgrade a browser against a profile already used by a newer one.
- **Uninstall:** Quit Slide and its managed browsers, then remove the app from Applications or uninstall from Windows Apps. Do not assume uninstall is a data purge; verify the data location below. To explicitly purge local Slide workspaces after backing up anything needed, remove the Slide data directory separately. This does not remove server snapshots or data belonging to an ordinary installed browser.

## Backup and recovery

1. Close managed browsers through Slide, wait for them to close, then quit the manager. Copy the entire Slide app-local data directory to a protected backup. It contains the private catalog and browser data; do not use this copy as a support attachment.
2. The default directory is `~/Library/Application Support/com.bloxystudios.slide-workspaces` on macOS and `%LOCALAPPDATA%\com.bloxystudios.slide-workspaces` on Windows. Back up the existing destination before replacing anything.
3. Restore to the same location while Slide and its browsers are closed, preferably under the same OS account with the original app and browser or a compatible newer browser. OS keychain-protected sessions are not guaranteed to survive migration. Cloud snapshots carry selected Slide metadata, not browser profile data.
4. In **Support**, use **Refresh support info** to re-read versions, **Restart manager** to retry catalog startup, or export diagnostics to a new local file. The startup log shows only whether the local catalog opened. For a missing or invalid browser, use **Diagnostics → Choose another browser**.

## Production signing and update plan

Current downloads are manual and unsigned for distribution. When authorized signing identities and runners are available, build the same pinned targets. On Mac, sign the nested runtime and helpers and then the application with Developer ID and hardened runtime before building and notarizing the distribution; staple the ticket and validate with `codesign --verify --deep --strict`, `xcrun stapler validate`, and `spctl --assess`. On Windows, sign the app and both installer formats with a timestamped Authenticode signature; verify with `signtool verify /pa`. Keep signing keys in restricted release CI, not the repository.

Only enable automatic updates after adding signed Tauri update manifests and per-target artifacts, protecting the update-signing key separately, and testing rejection of tampered updates, preservation of profile data, and interrupted-update recovery. Publish signed versions and hashes together with release notes. No update channel or production signing was exercised in this milestone.
