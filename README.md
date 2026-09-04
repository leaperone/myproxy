# myproxy

macOS menu-bar controller ([GPUI](https://github.com/zed-industries/zed)) for a bundled [mihomo](https://github.com/MetaCubeX/mihomo) core.

Strategy JSON under `~/Library/Application Support/myproxy/` is the source of truth: subscriptions, node groups, rules, and Mixed port (HTTP + SOCKS5 on one loopback port). The UI is Chinese: **节点组**, **规则**, **设置**.

## Install

Download `myproxy-*.sparkle.zip` from [Releases](https://github.com/leaperone/myproxy/releases), unzip, and move `myproxy.app` into Applications.

If the build is not Developer ID–notarized, macOS Gatekeeper may require a right-click → Open the first time.

## Updates

Stable channel (Sparkle 2 appcast):

https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml

Check from **设置 → 检查更新** or the menu-bar extra. v0.0.1 is a full install with no previous archive, so there is no Sparkle delta yet. Later `v*` tags keep prior zips and run `generate_appcast` so incremental deltas can be published.

## Develop

```sh
scripts/fetch-mihomo.sh
scripts/fetch-sparkle.sh          # Sparkle.framework + generate_appcast
cargo run --bin myproxy           # debug UI, no Sparkle
scripts/package-macos-app.sh      # release .app with Sparkle
scripts/release-macos.sh          # zip + appcast into dist/
cargo run --bin myproxyctl -- capabilities
```

Rust 1.98+ (`rust-toolchain.toml`). Subscription URLs are never written to `myproxy.log`.

## CLI

```sh
cargo run --bin myproxyctl -- subscription add 'https://…' --name Example
cargo run --bin myproxyctl -- filter --set '(?i)(流量|剩余|到期|官网)'
cargo run --bin myproxyctl -- group add PROXY --all
cargo run --bin myproxyctl -- rule add --name GitHub --keyword github --via PROXY
cargo run --bin myproxyctl -- apply
cargo run --bin myproxyctl -- connect
```

Default Mixed port is **17890**.

## Signing

Local packaging uses a Developer ID identity from the login keychain when one exists; otherwise it ad-hoc signs (same as the existing Air push scripts). GitHub Actions ad-hoc signs unless you add secrets later:

| Secret | Purpose |
| --- | --- |
| `SPARKLE_ED_PRIVATE_KEY` | EdDSA seed for `generate_appcast` / `sign_update` (required for in-app updates) |
| `APPLE_ID` / `APPLE_TEAM_ID` / `APPLE_APP_SPECIFIC_PASSWORD` | Optional notarization |
| `CODESIGN_IDENTITY` plus a Developer ID cert in the runner keychain | Optional Developer ID sign |

Do not commit `.env`, `.p12`, or the Sparkle private key.
