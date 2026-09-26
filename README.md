# myproxy

macOS menu-bar controller ([GPUI](https://github.com/zed-industries/zed)) for a bundled [mihomo](https://github.com/MetaCubeX/mihomo) core.

Strategy JSON under `~/Library/Application Support/myproxy/` is the source of truth: subscriptions, node groups, rules, and Mixed port (HTTP + SOCKS5 on one loopback port). The UI is Chinese: **总览**, **连接**, **订阅**, **节点组**, **规则**, **设置**.

## Install

Download `myproxy-*.sparkle.zip` from [Releases](https://github.com/leaperone/myproxy/releases), unzip, and move `myproxy.app` into Applications.

If the build is not Developer ID–notarized, macOS Gatekeeper may require a right-click → Open the first time.

## Updates

Choose **正式版（Prod）** or **Nightly** under **设置 → 更新 → 更新通道**, then use **检查更新** or the menu-bar extra. The choice is saved in `strategy.json`; older configurations default to the installed build's channel. Switching back to Prod receives the next newer Prod build and does not downgrade the installed app.

| Channel | Sparkle feed | CI/CD |
| --- | --- | --- |
| Prod | `https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml` | Push a `vMAJOR.MINOR.PATCH` tag, or run Release with channel `prod` and that existing tag. |
| Nightly | `https://github.com/leaperone/myproxy/releases/download/nightly/appcast.xml` | Builds `main` daily at 18:00 UTC, or run Release with channel `nightly`. |
| Xray | `https://github.com/leaperone/myproxy/releases/download/xray/appcast.xml` | Build `xray` on push or run its workflow on that branch; publish only after validation. |

Nightly builds are GitHub prereleases with immutable build tags. The `nightly` prerelease points to the latest Nightly feed. Both channels generate Sparkle deltas from recent same-channel archives; Nightly never replaces GitHub's latest stable release. Existing published Prod tags cannot be overwritten by the workflow. While the core is connected, Check for Updates and archive/delta downloads use Mixed as an HTTP proxy.

`Cargo.toml` holds the next target release version. Since `v0.0.3` is already released, `main` now targets `0.0.4`: Prod uses `v0.0.4`, while Nightly uses `v0.0.4-nightly.20260905.42.1` (UTC date, Release run number, attempt). The app displays the same version without the `v` prefix; the source commit is recorded in release notes. When a Prod release is ready, tag its matching commit, then advance `main` to the next target version. Nightly tags do not trigger the Prod workflow.

Both channels use the Release workflow's run number and attempt as `CFBundleVersion`, so Sparkle can compare builds across channels. The display version, build number, feed, and channel are validated against the packaged app before publication. A manually dispatched Prod release checks out the requested tag; its version must match `Cargo.toml` at that commit.

The title shows **Dev** for development builds and **Nightly** for Nightly builds; Prod has no badge. This identifies the installed build, independently of the selected update channel or developer logging. Local debug builds default to Dev and release builds to Prod; `MYPROXY_BUILD_CHANNEL=dev|prod|nightly|xray` overrides this at build time.

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

Rust 1.98+ (`rust-toolchain.toml`). Info / Warning / Error always go to `myproxy.log`; 设置 → 日志 shows the tail. Subscription URLs are never written.

## CLI

`myproxyctl` is included in `myproxy.app/Contents/MacOS/` and updated with the app. A first launch that does not yet have the PATH link opens a one-screen prompt to create `~/.cargo/bin/myproxyctl` and copy the official Agent skill. Skipping or installing writes `onboard.json` next to `strategy.json`, so the prompt does not return. The MacBook Air install script creates the same link automatically.

The app's **设置 → 命令行工具** panel can create or update that link later, and copies the official Agent skill from `.agents/skills/agent/SKILL.md`. It refuses to replace an existing regular file. If a terminal cannot find `myproxyctl`, add `~/.cargo/bin` to `PATH`. You can still invoke `/Applications/myproxy.app/Contents/MacOS/myproxyctl` directly.

All commands accept `--json` for one machine-readable success result on stdout. Runtime errors return a JSON `error` on stderr and a non-zero exit. The official [Agent CLI skill](.agents/skills/agent/SKILL.md) explains how to inspect and configure myproxy.

On macOS, the bundled CLI sends `apply`, `subscription refresh`, `connect`, `disconnect`, and runtime status queries to the signed application. Commands that change runtime state open that same app in the background if needed, without triggering its launch-time connection setting. `status` does not launch the app; if the Host is unavailable, System Extension and DNS are reported as unobserved. Pending authorization stays with the application after the CLI exits. If a command loses its reply, check `status` before retrying.

```sh
myproxyctl --json capabilities
myproxyctl --json status
myproxyctl --json group list
myproxyctl --json export
myproxyctl --json import ~/Downloads/myproxy-strategy-2026-09-10.json
```

`export` writes the current `strategy.json` (default `~/Downloads/myproxy-strategy-YYYY-MM-DD.json`). `import` replaces the live file after writing `strategy.json.bak-import-*` and does not apply; run `apply` if the core should pick it up. The **设置 → 配置** panel does the same with a file dialog.

```sh
cargo run --bin myproxyctl -- subscription add 'https://…' --name Example
cargo run --bin myproxyctl -- filter --set '(?i)(流量|剩余|到期|官网)'
cargo run --bin myproxyctl -- group add PROXY --all
cargo run --bin myproxyctl -- rule add --name GitHub --keyword github --via PROXY
cargo run --bin myproxyctl -- apply
cargo run --bin myproxyctl -- connect
```

Default Mixed port is **7890**.

## Xray channel

Xray is a separate release channel alongside Prod and Nightly. It uses the same
MyProxy application identity and the existing signed Network Extension. The title
shows an Xray badge. Its Sparkle feed is
`https://github.com/leaperone/myproxy/releases/download/xray/appcast.xml`; Xray
releases never update the Prod or Nightly feeds or GitHub's latest stable release.
The application stores Xray settings under
`~/Library/Application Support/myproxy-xray/`. Existing configurations imported
into earlier Xray builds remain there. `MYPROXY_XRAY_DATA_DIR` overrides this
location for isolated checks; the original backend still uses `MYPROXY_DATA_DIR`.

The app owns routing, manual node selection, ordered fallback, latency selection,
and connection accounting. HTTP and SOCKS share one public loopback entrance,
default **40808**. The System Extension sends connection metadata to the
application before accepting a connection. The application evaluates every
rule and returns Direct, Proxy, or Reject. The extension receives no routing
rules, domain lists, groups, or node configuration. Direct TCP and initial UDP
decisions return the original connection to macOS. Proxy decisions use a
short-lived authenticated relay credential bound to the selected route; the
application forwards those bytes to Xray's private per-node entrances. A signed
Xray host and core bypass their own capture path to avoid proxy and DNS recursion.

macOS cannot return an already-owned UDP destination or a DNS proxy flow to
the original system path. For these cases, an application Direct decision uses
the extension's direct transport, without sending payload through the app or
Xray. Failure to contact the application rejects the captured request; it does
not silently bypass the configured policy.
Captured DNS requests routed through a proxy use TCP to the app's public DNS
resolver through the selected node. The original system resolver can be local
to the user's network and unreachable from that node. Explicit SOCKS UDP DNS
requests retain their requested resolver. Other UDP traffic still requires UDP
support from the selected node.
Direct DNS also uses TCP in the extension. Before enabling capture, the app
queries the configured system resolvers and chooses one that returns a DNS
answer. Startup waits for the current DNS provider to be ready before testing
the system resolver. If DNS activation fails, it disables both DNS and
transparent capture; the local HTTP/SOCKS entrance remains available.

New configurations start in global mode through 节点选择, with 美国优先,
日本优先, 香港优先, and a direct choice. Groups expand to show their nodes;
automatic groups support a manual pin and a return to automatic selection.
Region groups aggregate matching nodes from every subscription and select the
lowest-latency available node. Priority groups reference those region groups in
order: 美国优先 uses 美国 → 日本 → 香港, 日本优先 uses 日本 → 香港 → 美国,
and 香港优先 uses 香港 → 美国 → 日本. A priority group tries the next region
when the current region has no available node; it never falls back to Direct.
These references stay current when subscriptions refresh. The group editor
supports adding, removing, and reordering child groups; the CLI accepts repeated
`--group-ref` arguments. Renaming a group updates its references, and a referenced
group cannot be deleted until those references are removed.
Routing changes close old flows without restarting Xray. Disconnect confirms
both capture and DNS are disabled before stopping their relay or core.

Rule mode supports application rules for captured traffic, exact/suffix/keyword/
wildcard domains, CIDRs, optional TCP/UDP qualifiers, and imported geographic or
domain categories. Domain and IP data are evaluated in the app. The local
`rulesets/mclash-geodata.json` snapshot preserves the imported categories;
missing or invalid data rejects activation. Explicit proxy requests do not carry
a trustworthy process identity, so application rules require system capture.
TUN is not used in this channel. Plain HTTP chunked uploads remain unsupported;
HTTPS uses CONNECT.

`scripts/export-mclash-geodata.py` copies the selected installed routing databases
into the private Xray configuration directory.
`scripts/import-mclash-rules.py` prepares a private migration candidate and receipt,
preserving current ports, modes, subscriptions, node selections and unrelated rules.
`--apply` imports through the bundled CLI after the channel supports the required
matchers. Imported user-ID and destination-port conditions remain joint
constraints on their original destination rules; they are not widened into
standalone user or network rules.

All three channels use the base version in `Cargo.toml`. Prod publishes
`vMAJOR.MINOR.PATCH`; Nightly uses
`vMAJOR.MINOR.PATCH-nightly.YYYYMMDD.RUN.ATTEMPT`; Xray uses
`vMAJOR.MINOR.PATCH-xray.YYYYMMDD.RUN.ATTEMPT`. For example, with base version
`0.0.10`, the Xray archive is `myproxy-0.0.10-xray.20260922.71.1.sparkle.zip`.
Nightly and Xray are channel releases on GitHub and never replace the latest
Prod release. Their separate feed pointers are `nightly` and `xray`.

The Xray workflow tests feature-branch pushes. A commit marked `[xray-package]`
also produces a signed, notarized candidate artifact for local acceptance, without
publishing. A push to `xray`, or a manual run on that branch, creates a version
Release and Tag only after testing, signing, and package validation pass. It then
updates the `xray` feed pointer. Failed builds create no version tags. The app
and extension use the numeric `RUN.ATTEMPT` build number for update ordering;
this also upgrades the earlier incorrectly named `1.6.x` packages. Distribution requires Developer ID
signing, the host and extension provisioning profiles, Apple notarization,
stapling, Gatekeeper acceptance, and a signed Xray appcast. There is no ad-hoc
fallback. Original Prod/Nightly release scripts and workflow remain unchanged.

For a local signed upgrade, run `python3 scripts/install-xray-channel.py ARCHIVE`.
The installer verifies the package before stopping the app, backs up the current
configuration and bundle, and restores an existing connection after replacement.
It checks HTTP and SOCKS access on the original port before reporting success.

## Routing and runtime state

The iOS and Android Xray applications are developed separately under [mobile](mobile/README.md). They share the desktop Rust policy sources, use Expo for the mobile interface, and use native VPN services with an embedded Xray runtime. Their GitHub Actions builds do not update the desktop channels.

`strategy.json` stores the saved configuration. Saving, applying, Mihomo readiness,
System Extension activation, and DNS readiness are separate states. Apply validates
the candidate before replacing a running configuration; a failed activation reports
the failure and attempts to restore the previous working configuration.

The protected `runtime-state.json` keeps the applied YAML and matching System
Extension request together. An older running core without this snapshot requires
one disconnect and reconnect before applying changes. `MYPROXY_DATA_DIR` overrides
the data directory for isolated command checks; normal launches keep the standard
application support directory.

- Groups resolve pins in order, then automatic matches. Exact exclusions apply to
  both; name exclusions apply only to automatic matches. Empty groups reject traffic
  and show as unavailable instead of silently routing it directly.
- Matchers inside one rule are OR conditions. Rules are evaluated in their displayed
  order, after built-in local-network bypasses, and only after traffic enters Mixed,
  System Extension, or TUN. Mixed and System Extension each have their own mode; TUN
  follows the profile rules and is mutually exclusive with System Extension.
- `gfw:<group>` is evaluated by mihomo (`RULE-SET,gfw` then DIRECT on a miss).
  System Extension still captures the user's own process and domain/suffix/keyword/cidr
  matchers, including pins whose via is `gfw:<group>`. It does not embed the GFW
  domain list.
- The connections page shows connections recorded by Mihomo. System Extension
  traffic passed directly to macOS or rejected before Mihomo is outside that list;
  **显示直连** reveals only DIRECT connections recorded by the core.
- System Extension DNS interception stays coupled to the core: disconnect waits
  until capture and DNS are down before stopping Mihomo. If NEDNSProxy cannot be
  disabled, the core is kept so system resolution is not blackholed. When the
  private SOCKS backend is down, the DNS provider relays queries directly.
- macOS hands the DNS provider the queried name as the flow endpoint, not the
  resolver address, so the provider answers those queries through the resolver list
  the strategy already uses (`dns.nameserver`). DNS readiness reported in the app
  proves the private SOCKS relay is reachable, not that every lookup resolves.

Source checks and builds do not verify macOS approval, DNS forwarding, or actual
traffic. Those require acceptance on the signed installed application.

## Signing

Local packaging uses a Developer ID identity from the login keychain when one exists; otherwise it ad-hoc signs. Both CI release channels require Developer ID signing, notarization, and Sparkle signatures:

| Secret | Purpose |
| --- | --- |
| `SPARKLE_ED_PRIVATE_KEY` | EdDSA seed for `generate_appcast` / `sign_update` (required for in-app updates) |
| `APPLE_ID` / `APPLE_TEAM_ID` / `APPLE_APP_SPECIFIC_PASSWORD` | Required notarization |
| `CSC_LINK` / `CSC_KEY_PASSWORD` | Base64 Developer ID certificate and its password |
| `MYPROXY_HOST_DEVID_PROFILE` / `MYPROXY_NETWORK_EXTENSION_DEVID_PROFILE` | Provisioning profiles needed for System Extension activation |

Do not commit `.env`, `.p12`, or the Sparkle private key.
