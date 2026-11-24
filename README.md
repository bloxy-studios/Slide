# Slide Workspaces

A desktop manager for persistent, isolated browser workspaces.

Slide Workspaces is local-only through Milestone B. **Open** starts the installed Google Chrome or Chromium shown in the manager, using a normal browser window and a private directory for that workspace. Workspace management, import, templates, reminders and trash work offline. External sites and the Chrome Web Store still need network access.

## Run

Install stable Rust, Bun and platform build tools (Xcode Command Line Tools on macOS, or MSVC/Windows SDK and WebView2 on Windows). Install Chrome or Chromium separately.

```sh
bun install --frozen-lockfile
bun run dev
```

For the installed-app demo, start just the synthetic fixture with `bun run fixture`. Use `http://127.0.0.1:4310` as the start URL in two named workspaces and sign in as Creator in one and Viewer in the other. There is a second fixture origin at `http://127.0.0.1:4311`.

## Build a development installer

```sh
# On macOS (native architecture)
bun run --cwd apps/desktop tauri build --bundles app,dmg

# On Windows x64
bun run --cwd apps/desktop tauri build --target x86_64-pc-windows-msvc --bundles nsis,msi
```

Artifacts are in `apps/desktop/src-tauri/target/` and [desktop installer CI](https://github.com/bloxy-studios/Slide/actions/workflows/desktop.yml). Development installers are unsigned for distribution: macOS uses an ad hoc application signature; no notarization or Windows Authenticode is provided. Use approved installation procedures; do not disable OS protections.

Removing the app leaves local workspace data in the Tauri app data folder. On macOS this is `~/Library/Application Support/com.bloxystudios.slide-workspaces`; on Windows it is `%LOCALAPPDATA%\com.bloxystudios.slide-workspaces`. Close all managed sessions and back up that folder before manually deleting it; deleting it purges the catalog and every local workspace. It is not your ordinary Chrome profile.

## Checks and evidence

```sh
bun run test
bun run --cwd apps/desktop build
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml
```

- [Milestone A acceptance and Mac artifact details](docs/milestone-a.md)
- [Milestone B local workflows and import format](docs/milestone-b.md)
- [Feature matrix and gaps](docs/feature-matrix.md)
- [Runner inventory](docs/runners.md)
