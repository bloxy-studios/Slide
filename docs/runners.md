# Milestone A runners

| Item | macOS runner | Windows runner |
|---|---|---|
| OS | macOS Tahoe 26.6.2 (25G83) | Not inspected in this Mac session |
| CPU architecture | arm64 (Apple Silicon) | CI target: x86_64 |
| Memory | 16 GiB | Not measured |
| Interactive desktop | Available | Not checked |
| Browser | Google Chrome 154.0.8037.93 | Not checked |
| Browser executable architecture | Universal (arm64 + x86_64), arm64 execution | Not checked |
| Rust / Cargo | 1.99.0, aarch64-apple-darwin | CI uses stable Rust |
| Bun / Node | Bun 1.4.5, Node.js 24.21.0 | CI uses Bun |
| Installer tools | Xcode 26.6 (17F113), hdiutil 683.160.3 | Tauri Windows bundlers (WiX and NSIS) in CI |

Intel Mac execution is not verified by this arm64 runner.
