# Bundled Chromium development installers

Slide's development installers default new profiles to fingerprint-chromium where a native build is bundled, otherwise to ungoogled-chromium. The manager displays the brand, version, architecture and location. **Choose another browser** lets you select a compatible installed browser; **Use bundled Chromium** clears that override. Existing recorded profiles do not switch browsers. Source-mode development (`bun run dev`) still discovers installed browsers.

Use the [bundle command](../README.md#build-a-development-installer), not `tauri build` alone. It downloads the official release asset over HTTPS and checks it against the SHA-256 in [bundle.ts](../apps/desktop/bundle.ts). It also verifies cached assets before mounting or extracting them. CI uses the same command; the repository does not contain browser binaries.

| Target | Platform release | Official asset |
|---|---|---|
| macOS arm64 | [157.0.8103.78-1.1](https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/tag/157.0.8103.78-1.1) | [arm64 DMG](https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/download/157.0.8103.78-1.1/ungoogled-chromium_157.0.8103.78-1.1_arm64-macos.dmg) |
| macOS x86_64 | [157.0.8103.78-1.1](https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/tag/157.0.8103.78-1.1) | [x86_64 DMG](https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/download/157.0.8103.78-1.1/ungoogled-chromium_157.0.8103.78-1.1_x86_64-macos.dmg) |
| Windows x64 | [158.0.8127.36-1.1](https://github.com/ungoogled-software/ungoogled-chromium-windows/releases/tag/158.0.8127.36-1.1) | [x64 portable ZIP](https://github.com/ungoogled-software/ungoogled-chromium-windows/releases/download/158.0.8127.36-1.1/ungoogled-chromium_158.0.8127.36-1.1_windows_x64.zip) |

These are non-prerelease builds from the official ungoogled-software macOS and Windows repositories, not Chromium continuous-build snapshots or Google Chrome Stable. The two platform projects release independently, so their versions can differ. Check the release dates rather than assuming either one already contains every Chromium security fix.

| Target | Additional fingerprint-chromium runtime | Verification |
|---|---|---|
| macOS arm64 | [148.0.7778.215 DMG](https://github.com/adryfish/fingerprint-chromium/releases/tag/148.0.7778.215), pinned SHA-256 | Native arm64 executable and framework; launched natively and reported overridden CPU cores and time zone |
| macOS Intel | None; the 148 DMG has no x86_64 executable slice and the 150 release contains no macOS artifact | ungoogled-chromium only; the language and WebRTC flags are available, but not seeded overrides |
| Windows x64 | [150.0.7871.186 ZIP](https://github.com/adryfish/fingerprint-chromium/releases/tag/150.0.7871.186), pinned SHA-256 | Download and archive layout verified; not installed or run on Windows |

The patched releases substantially lag Chromium Stable. SHA-256 pinning checks that the downloaded bytes match the expected asset; it does not establish security freshness, source reproducibility or provenance. Use an up-to-date browser for sensitive browsing. The original ungoogled runtime remains packaged for recorded profiles.

**Advanced settings → Fingerprint** stores a per-workspace seed and allows refresh before the next cold launch. The patched runtime accepts brand/version and platform/version overrides for UA and Client Hints, CPU core count, language, time zone, WebRTC UDP restriction, and switches to opt out of the seeded Canvas, WebGL/GPU, audio, ClientRects or font changes. It chooses device memory from the seed; there is no documented override for a specific memory size, arbitrary UA string, or WebGL vendor. GPU metadata changes are platform-dependent. Device name and MAC address are local metadata, not web-exposed values. DB-IP remains a browser-side connectivity and location check; its free response does not provide a reliable time zone or language, so automatic proxy matching is not implemented. The startup page shows observed values, not the requested settings. Client Hints and device memory may be unavailable in the local startup tab; verify them in the loopback fixture instead.

On an installed macOS arm64 build, a [workspace configured for 7 cores and UTC](evidence/mac-fingerprint-settings.png) reported both overrides in its [loopback fixture](evidence/mac-fingerprint-fixture.png). An opt-in Bun test launched the pinned arm64 binary with two separate profile directories and verified the requested core counts, platform, and time zone and differing canvas hashes. It does not verify every override. Run the test with `SLIDE_TEST_FINGERPRINT_BROWSER=/path/to/Chronium/executable bun run test`.

Bundled ungoogled-chromium [does not include Google Safe Browsing](https://ungoogled-software.github.io/ungoogled-chromium-wiki/faq#why-is-safe-browsing-disabled). It does not provide Safe Browsing warnings for known malicious sites or downloads. Use an installed browser with that protection if you need it.

ungoogled-chromium and its platform packaging are BSD 3-Clause licensed. The installers include the [ungoogled-chromium notice](ungoogled-chromium-license.txt) and the [Chromium notice](chromium-license.txt); third-party notices are in `chrome://credits`. macOS copies the browser from the read-only release DMG and signs the nested app in `Contents/Frameworks` and the manager ad hoc. Windows uses the portable ZIP contents, not the release's installer EXE, for both NSIS and MSI. Neither platform has a production distribution signature; the Mac DMG is not notarized.

### Profile behavior

- Each workspace still uses its own app-owned `--user-data-dir` and opens a normal, non-kiosk browser window with the configured launch URLs. Slide seeds the Chromium profile name in `Default/Preferences` if the workspace data directory is empty; it does not overwrite existing Preferences or Chromium-managed Local State. A settings-only clone gets a fresh UUID and browser directory.
- No silent extension installation, arbitrary launch arguments, or proxy settings are provided. Per-workspace proxy configuration remains gated on verifying the route and fail-closed behavior.
- An installed browser override is app-wide for new profiles. Existing profiles keep using their recorded browser brand at the same version or newer, or block if no compatible copy is available. Slide can identify a legacy profile from an already-running verified session; if it cannot identify a nonempty legacy profile, it blocks rather than risk its saved sessions. Back up the manager's data before upgrading. Browser-version downgrades and Chrome/Chromium encrypted storage migration are not supported.
- The previous snapshot had version `160.0.8182.0`, higher than these releases. Profiles pinned to that snapshot will not open in these older builds. On macOS the different bundle identifier also keeps ungoogled-chromium separate from stock Chromium. Use a compatible copy of the recorded browser to keep the existing session, or clone settings into a fresh workspace and sign in again.
- Milestone A and B discover a browser without saving its path. The only saved app-wide path in those releases comes from choosing a browser. Upgrading preserves that choice; select **Use bundled Chromium** to clear it for new profiles without changing browsers for existing profiles.

### Updates

The browser does not self-update. Follow the macOS and Windows release pages above for security updates. For each target, select a non-prerelease asset matching its architecture and format, verify the download's SHA-256 against the digest published with that asset, and update the release, asset suffix and hash together in `bundle.ts`. Update this table and review the release's license notices. Build all three targets and smoke-test the installed apps, including reopening a profile from the previous pin and confirming an older runtime still cannot open a newer profile.

### Extensions

For a bundled workspace, add the extension's catalog ID and exact version, assign it to the workspace, then close that workspace. In **Extensions**, select the workspace and open **Install options → Choose downloaded CRX**. Slide accepts signed Manifest V3 CRX3 files up to 8 MiB, verifies the package signature and signing-key-derived ID against the catalog entry, and checks the exact version pin. It previews required, optional, and content-script permissions from the signed manifest. Only **Approve and install** writes and enables the unpacked copy for the next launch of that workspace.

Slide does not fetch extension updates in the background. If you raise the catalog pin above the approved version, the workspace's install options show the newer pinned version. Supply the matching CRX and approve it separately. The catalog does not check whether a matching package is available online. For an installed Chrome workspace, **Install options → Open store listing** uses the browser's Web Store approval instead. The fixture entry supplies a signed CRX for testing without a download.

In a separate macOS arm64 QA profile, the app installed from the DMG displayed the fixture's [CRX permission review](evidence/mac-ungoogled-crx-review.png). After approval and workspace launch, bundled ungoogled-chromium displayed the [fixture badge v1.0.0](evidence/mac-ungoogled-crx-badge.png).

### macOS arm64 installed-app check

The app copied from the DMG ran with a separate QA home directory. The [manager](evidence/mac-ungoogled-manager.png) reported both workspaces running. [Creator](evidence/mac-ungoogled-creator.png) and [Viewer](evidence/mac-ungoogled-viewer.png) were signed into the same loopback fixture at the same time, with distinct cookies, localStorage, and IndexedDB markers. Closing Viewer from the manager left Creator signed in. The QA home had no default Keychain; the test dismissed that prompt without resetting Keychain. This checks live session isolation, not encrypted cookie persistence across restarts.

### Previous Chromium snapshot: macOS arm64 installed-app check

The QA app was copied out of its DMG under a separate application ID so existing profiles were not modified. The [manager](evidence/mac-bundled-manager.png) showed Bundled Chromium 160.0.8182.0 and both workspaces running. Folve Creator and Folve Viewer opened the same loopback fixture in [ordinary Chromium windows](evidence/mac-bundled-creator-viewer.png) with different cookies, localStorage and IndexedDB markers. Closing Viewer from the manager left Creator running; [reopening Viewer](evidence/mac-bundled-viewer-reopened.png) restored its own fixture identity. Both profiles retained the names set by the manager. Chromium showed its normal first-run dialog; the OS default browser was left unchanged.

### Windows installer retest

The [Milestone A Windows report](evidence/windows-milestone-a.md) observed that MSI installation and the Start shortcut worked, but **Finish** with **Launch Slide Workspaces** checked returned 1631. The MSI template now dispatches the checkbox through WiX's interactive shell action using the installed executable path. The log did not establish the original process token or cause. To verify the change, install the new MSI interactively on Windows x64 with logging enabled:

```powershell
msiexec.exe /i "Slide Workspaces_0.1.0_x64_en-US.msi" /L*v "slide-msi-install.log"
```

On a clean install, leave the checkbox checked and click **Finish**. Verify exactly one manager opens in the signed-in desktop session and displays Bundled Chromium. Close it and repeat on a clean install with the box unchecked; verify no manager opens until launched from Start. This needs an interactive Windows run; an installer build alone does not verify it.
