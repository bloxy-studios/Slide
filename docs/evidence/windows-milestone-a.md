# Milestone A — Windows x64 acceptance

Tested 2026-11-24 on Windows Server 2022 using the installed Slide Workspaces 0.1.0 app and synthetic fixture roles only. Branch `devin/milestone-a`, commit `62bea6e685056d7ef50a9f3f43475abce50fe246`, [PR #1](https://github.com/bloxy-studios/Slide/pull/1). No repository edits or pushes.

**Result:** The primary two-role isolation flow passed. The MSI Finish-screen launcher failed on this runner, but the MSI installed and its Start entry worked. Focus worked on mouse retry; Windows rejected an earlier foreground activation request. This is not a blanket pass of the full spec: VM reboot, a fully matched native-Chrome comparison, and the extra T11 checks listed below were not exercised.

## Runner and installer provenance

- Windows Server 2022 Standard x64, build `20348.5622`; physical RAM `34,355,564,544` bytes (~32 GiB); interactive Administrator console session 1 with Explorer running.
- Git `2.47.1.windows.1`, Node `24.22.2`, npm `11.19.1`, Bun `1.4.5`; Windows Installer `5.0.20348.4171`. No local Rust/NSIS/WiX build was run: the packages came from CI.
- Pre-existing Chrome for Testing: `156.0.8078.47`. Installed signed Google Chrome Enterprise x64 (`158.0.8127.89`) at `C:\Program Files\Google\Chrome\Application\chrome.exe`; both the manager and the normal control used installed Chrome. WebView2 `158.0.4370.33` was present after NSIS setup.
- Source: [GitHub Actions run 45423189067](https://github.com/bloxy-studios/Slide/actions/runs/45423189067), CI artifact `slide-workspaces-x86_64-pc-windows-msvc-unsigned`, ID `14936491333`; downloaded via the [numeric artifact mirror](https://nightly.link/bloxy-studios/Slide/actions/artifacts/14936491333.zip) after the GitHub API rate-limited. The ZIP digest matched the published value. Both installers returned Authenticode `NotSigned`.

| File | SHA-256 |
|---|---|
| `slide-windows.zip` | `5f7c2728bed8f3ac062fe296f2b143e9161f5da09684cdc9ba44c8b1165622bf` |
| `Slide Workspaces_0.1.0_x64-setup.exe` | `cc2c7f60c58fd56efa39b3b020ec527871e09bcf00217fa110ad5fc770d2e` |
| `Slide Workspaces_0.1.0_x64_en-US.msi` | `e739ad3898dcbaeb11332be60e389f182668c667fe9da537ec6220f51ee07517` |

## Procedures and results

Started the documented fixture with `bun run fixture` in the repository. It served `http://127.0.0.1:4310/` (second origin `4311`). Used visible manager and Chrome controls for the acceptance flows; process inspection, the disposable crash/background preconditions and missing-runtime preparation used the shell.

| Case | Procedure and expected result | Observed result |
|---|---|---|
| T01 — NSIS installation and Start | Install the x64 EXE, launch Slide Workspaces from Start, and open a workspace. Expect manager and x64 runtime discovery without manual selection. | **Passed.** NSIS completed, Start launched the app, and it detected installed Chrome `158.0.8127.89`. |
| T01 — native Chrome | Open a workspace left without a URL; expect native New Tab plus the usual browser controls, Settings and Downloads. Compare against a normal Chrome window. | **Passed for native functionality.** New Tab, tabs/address bar, menus, Settings and Downloads were available. **Untested:** the formal spec comparison with all window size, extensions, theme and first-run conditions matched. |
| T01 — MSI installation and Start | After NSIS removal, install the x64 MSI. Expect installer exit 0 and Start entry to launch the manager. | **Passed.** MSI exit 0; Start opened the installed manager with all three saved workspace entries. Wizard auto-launch was a separate failure below. |
| T02 — concurrent fixture roles | Create `Folve Creator` and `Folve Viewer` with `http://127.0.0.1:4310/`; open each and sign into the matching fixture role. Expect the distinct identities at the same URL. | **Passed.** Creator: cookie `creator`, localStorage and IndexedDB `fixture:creator`. Viewer: cookie `viewer`, localStorage and IndexedDB `fixture:viewer`. |
| T02 — logout and ordinary Chrome control | Log out only Viewer; revisit the fixture in Creator and in ordinary Chrome. Expect Viewer signed out and no state change in either other window. | **Passed.** Viewer lost its identity cookie (its existing storage markers remained, as the fixture only clears the cookie). Creator remained signed in with Creator markers; ordinary Chrome remained signed out without managed markers. |
| T03 — restart and duplicate protection | With both managed windows open, quit the manager, relaunch from Start and double-click Viewer Open. Expect attachment to the existing sessions and one root process per workspace. | **Passed.** Both remained Running; process identities `4776` and `5276` with their original creation times and distinct profile directories were unchanged. |
| T03 — last-tab close and cold double-click | Close Viewer's last Chrome tab. Expect Closed; double-click Viewer Open and expect only one replacement process. | **Passed.** Viewer reopened as one new root PID `4952`, while Creator retained PID `4776`. |
| T03 — crash isolation/recovery | Force-stop only Viewer's managed root process, then reopen using the manager. Expect an actionable crash state, Creator unaffected, and Viewer data retained. | **Passed.** Viewer showed `Exited unexpectedly` and `Browser exited (exit code: 0xffffffff). Open the workspace to recover.` Reopen returned the Viewer identity; Creator stayed running. |
| T03 — background Close | Run real Chrome with the disposable workspace profile and `--keep-alive-for-test` to create a background process with no window. Expect disabled Close and a remedy; Open should bring up the existing session. | **Passed.** Close was disabled with `Open this workspace first to close it from the manager.` Open used the same PID `10588`. The disposable process was cleaned up. |
| T03 — full close and reopen | After MSI Start launch, with Creator and Viewer signed in, Close both in the manager; verify both Closed, quit manager, relaunch from Start and reopen both without signing in again. | **Passed.** A new manager process showed both Closed before reopening; both returned signed in at the same URL with their original cookie, localStorage and IndexedDB values. Ordinary Chrome was not closed. |
| T03 — VM reboot | Reboot when permitted; expect reconciliation and retained data. | **Untested.** Tested app relaunch, not an OS reboot. |
| T11 — keyboard-only | Use keyboard navigation and activation to create `Keyboard Native Tab`, then Open and Close it. Expect a native new tab and Closed afterward. | **Passed.** Completed create/open/close without a mouse in that run. |
| T11 — invalid runtime | Use Change browser and submit a non-existent Chrome executable path. Expect an actionable error without losing workspaces or the working browser. | **Passed.** Displayed `Cannot use this browser: The selected path does not exist. Choose an installed Google Chrome or Chromium.` Catalog retained. |
| T11 — missing runtime | Temporarily make installed runtime undiscoverable and restart manager. Expect setup guidance and retained catalog; restore Chrome and select the installed path. | **Passed.** Showed `Set up your browser` and `No compatible Google Chrome or Chromium installation was found. Install one and choose its application or executable below.` Restoring/selecting Chrome recovered the runtime. |
| T11 — additional spec checks | Recover from a lost connection and choose a different valid Chrome/Chromium runtime. | **Untested.** Invalid-path rejection and recovery to the original valid Chrome were covered; offline recovery and switching to another valid runtime were not. |
| Focus/Close | With Creator and Viewer both Running, Focus Viewer from the manager and Close Viewer. Expect Viewer to come forward, then close independently of Creator. | **Mixed.** Mouse retry raised Viewer, and Close stopped Viewer while Creator stayed Running. An earlier Focus request failed to activate the window (see below). |
| Default NSIS uninstall | Exit manager and uninstall, leaving `Delete the application data` unchecked. Expect the app executable removed, profile data retained. | **Passed.** Executable removed; `catalog.sqlite3` and all three workspace profile directories remained under `%LOCALAPPDATA%\com.bloxystudios.slide-workspaces`. Subsequent MSI launch reopened both profiles with saved fixture accounts. MSI removal and opt-in data removal were not exercised. |

## Findings and repro

1. **MSI Finish checkbox did not launch Slide Workspaces (observed once).** Remove the NSIS build using the default data-retaining option; run `msiexec.exe /i "Slide Workspaces_0.1.0_x64_en-US.msi" /L*v "slide-msi-install.log"`; leave **Launch Slide Workspaces** checked at the completion screen, then press **Finish**. Expected: manager opens. Observed: wizard closed without a manager window; Start > Slide Workspaces opened it afterward. Log: `Action ended 15:54:16: LaunchApplication. Return value 1631.` The same log reports `Installation success or error status: 0.` No root cause inferred.
2. **Focus was not reliable across activation methods.** With both role workspaces Running, activate Focus on the Viewer row while the manager is visible. An initial accessibility-driven activation did not raise Viewer, and the manager showed `Folve Viewer: Windows did not allow foreground activation. Select this browser window from the taskbar.` Selecting from the taskbar worked; a subsequent physical mouse click on Focus raised Viewer. This proves the fallback and mouse retry, not that every foreground activation works.

## Screenshots

| Before | After |
|---|---|
| ![Creator and Viewer signed in at the same URL](https://app.devin.ai/attachments/749dda29-8fdd-46e6-a283-024194bfc9ad/ss_5cc151ad.png) **Both roles signed in before Viewer logout** | ![Viewer signed out; Creator remains signed in](https://app.devin.ai/attachments/3e21ab30-2ce0-40a8-b0f7-dec8d56b77bb/ss_3e4c9c83.png) **Viewer logged out; Creator still signed in** |

| Control or failure state | Related test surface |
|---|---|
| ![Ordinary Chrome signed out](https://app.devin.ai/attachments/321455e9-4884-4daf-a0ea-ecbea21199b7/ss_c2fc6ee2.png) **Ordinary Chrome remains signed out** | ![Chrome Native New Tab](https://app.devin.ai/attachments/c2b8fdc4-626b-4926-80ef-6736155605f6/ss_7e70b9ea.png) **Native New Tab in an empty workspace** |
| ![Windows focus feedback](https://app.devin.ai/attachments/3617d6e8-1901-4666-8bef-57a2a8a031da/ss_875d0b05.png) **Windows denied one activation request** | ![Viewer exited unexpectedly](https://app.devin.ai/attachments/9224b194-b7fd-44e1-9511-967942df2116/ss_2a420fa0.png) **Viewer crash; Creator still Running** |
| ![Background workspace Close disabled](https://app.devin.ai/attachments/fa5068b7-132b-4655-8fda-d8571885b106/ss_3d7c2c3b.png) **Background Close disabled with hint** | ![Missing runtime setup state](https://app.devin.ai/attachments/85c1dacd-8faf-4cc0-b294-746908c43ff0/ss_17572326.png) **Missing runtime recovery prompt** |
| ![MSI completion screen](https://app.devin.ai/attachments/65ef957e-d5fd-4abc-97f7-f81549c7e76e/ss_d58429f4.png) **MSI Finish with Launch checked** | ![MSI app launched from Start after both workspaces closed](https://app.devin.ai/attachments/ba8dfbfc-4b2e-45ee-9ab5-afcb6e4ff02a/ss_766c286e.png) **MSI manager at Start relaunch, before both browsers reopen** |

## Local artifacts

- Annotated recording: `C:\Users\Administrator\screencasts\slide-windows-roles\slide-windows-roles-edited.mp4` (primary two-role flow and Focus/Close retry).
- Runner: `C:\Users\Administrator\Downloads\slide-runner.txt`
- Fixture startup: `C:\Users\Administrator\Downloads\slide-fixture.log`
- Chrome MSI installation: `C:\Users\Administrator\Downloads\slide-chrome-install.log`
- Lifecycle/process observations: `C:\Users\Administrator\Downloads\slide-lifecycle.log`
- NSIS uninstall and retained data: `C:\Users\Administrator\Downloads\slide-uninstall.log`
- MSI verbose installation: `C:\Users\Administrator\Downloads\slide-msi-install.log`
- Screenshots: `C:\Users\Administrator\screenshots\` (filenames in the screenshot links above).
