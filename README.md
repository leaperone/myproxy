# myproxy

macOS menu-bar controller ([GPUI](https://github.com/zed-industries/zed)) for a bundled [mihomo](https://github.com/MetaCubeX/mihomo) core.

Strategy JSON under `~/Library/Application Support/myproxy/` is the source of truth: subscriptions, node groups, rules, and Mixed port (HTTP + SOCKS5 on one loopback port). The UI is Chinese: **连接**, **节点组**, **规则**, **设置**.

## Install

Download `myproxy-*.sparkle.zip` from [Releases](https://github.com/leaperone/myproxy/releases), unzip, and move `myproxy.app` into Applications.

If the build is not Developer ID–notarized, macOS Gatekeeper may require a right-click → Open the first time.

## Updates

Choose **正式版（Prod）** or **Nightly** under **设置 → 更新 → 更新通道**, then use **检查更新** or the menu-bar extra. The choice is saved in `strategy.json`; older configurations default to the installed build's channel. Switching back to Prod receives the next newer Prod build and does not downgrade the installed app.

| Channel | Sparkle feed | CI/CD |
| --- | --- | --- |
| Prod | `https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml` | Push a `vMAJOR.MINOR.PATCH` tag, or run Release with channel `prod` and that existing tag. |
| Nightly | `https://github.com/leaperone/myproxy/releases/download/nightly/appcast.xml` | Builds `main` daily at 18:00 UTC, or run Release with channel `nightly`. |

Nightly builds are GitHub prereleases with immutable build tags and full archives. The `nightly` prerelease points to the latest Nightly feed. Prod generates deltas only from previous Prod archives; Nightly never replaces GitHub's latest stable release. Existing published Prod tags cannot be overwritten by the workflow.

`Cargo.toml` holds the next target release version. Since `v0.0.3` is already released, `main` now targets `0.0.4`: Prod uses `v0.0.4`, while Nightly uses `v0.0.4-nightly.20260905.42.1` (UTC date, Release run number, attempt). The app displays the same version without the `v` prefix; the source commit is recorded in release notes. When a Prod release is ready, tag its matching commit, then advance `main` to the next target version. Nightly tags do not trigger the Prod workflow.

Both channels use the Release workflow's run number and attempt as `CFBundleVersion`, so Sparkle can compare builds across channels. The display version, build number, feed, and channel are validated against the packaged app before publication. A manually dispatched Prod release checks out the requested tag; its version must match `Cargo.toml` at that commit.

The title shows **Dev** for development builds and **Nightly** for Nightly builds; Prod has no badge. This identifies the installed build, independently of the selected update channel or developer logging. Local debug builds default to Dev and release builds to Prod; `MYPROXY_BUILD_CHANNEL=dev|prod|nightly` overrides this at build time.

The repository's [release skill](.agents/skills/release/SKILL.md) handles `/release patch`, `/release minor`, `/release major`, and `/release nightly` (`$release` in Codex). Stable increments start from the latest published Prod version, so an already-advanced development version is not incremented twice. Creating or reviewing the skill does not publish a release.

## Develop

```sh
scripts/fetch-mihomo.sh
scripts/fetch-sparkle.sh          # Sparkle.framework + generate_appcast
cargo run --bin myproxy           # debug UI, no Sparkle
MYPROXY_PAGE=connections cargo run --bin myproxy
scripts/package-macos-app.sh      # release .app with Sparkle
scripts/release-macos.sh          # zip + appcast into dist/
cargo run --bin myproxyctl -- capabilities
```

Rust 1.98+ (`rust-toolchain.toml`). Subscription URLs are never written to `myproxy.log`.

## CLI

`myproxyctl` is included in `myproxy.app/Contents/MacOS/` and updated with the app. After a drag-and-drop installation, invoke `/Applications/myproxy.app/Contents/MacOS/myproxyctl` directly or add your own PATH link. The MacBook Air install script creates `~/.cargo/bin/myproxyctl` automatically.

The app's **设置 → 命令行工具** panel can create or update the same `~/.cargo/bin/myproxyctl` link. It refuses to replace an existing regular file.

All commands accept `--json` for one machine-readable success result on stdout. Runtime errors return a JSON `error` on stderr and a non-zero exit. The official [Agent CLI skill](.agents/skills/agent/SKILL.md) explains how to inspect and configure myproxy.

```sh
myproxyctl --json capabilities
myproxyctl --json status
myproxyctl --json group list
```

```sh
cargo run --bin myproxyctl -- subscription add 'https://…' --name Example
cargo run --bin myproxyctl -- filter --set '(?i)(流量|剩余|到期|官网)'
cargo run --bin myproxyctl -- group add PROXY --all
cargo run --bin myproxyctl -- rule add --name GitHub --keyword github --via PROXY
cargo run --bin myproxyctl -- apply
cargo run --bin myproxyctl -- connect
```

Default Mixed port is **7890**.

## Signing

Local packaging uses a Developer ID identity from the login keychain when one exists; otherwise it ad-hoc signs. Both CI release channels require Developer ID signing, notarization, and Sparkle signatures:

| Secret | Purpose |
| --- | --- |
| `SPARKLE_ED_PRIVATE_KEY` | EdDSA seed for `generate_appcast` / `sign_update` (required for in-app updates) |
| `APPLE_ID` / `APPLE_TEAM_ID` / `APPLE_APP_SPECIFIC_PASSWORD` | Required notarization |
| `CSC_LINK` / `CSC_KEY_PASSWORD` | Base64 Developer ID certificate and its password |
| `MYPROXY_HOST_DEVID_PROFILE` / `MYPROXY_NETWORK_EXTENSION_DEVID_PROFILE` | Provisioning profiles needed for System Extension activation |

Do not commit `.env`, `.p12`, or the Sparkle private key.
