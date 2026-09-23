# MyProxy Xray mobile

The iOS and Android applications use one Expo interface and the existing MyProxy Rust policy implementation. Xray executes the selected outbound; the VPN service runs independently of JavaScript.

## Source layout

| Directory | Responsibility |
| --- | --- |
| `app` | Expo interface: connection, expandable node groups, imports and rules |
| `app/modules/myproxy` | Native control module, private storage, Android VPN service |
| `ios-extension` | iOS Packet Tunnel, public packet-flow API and background lifecycle |
| `network` | Embedded Xray and gVisor packet adapter, socket protection and traffic accounting |
| `../crates/myproxy-core` | The same strategy, catalog, parsers, defaults and policy files compiled by desktop |
| `../crates/myproxy-mobile` | Validated mobile documents, desired/applied snapshots, C/JNI interface |

See [CONTRACT.md](CONTRACT.md) for the native commands and data shapes. A configuration change is validated before it replaces saved state. The runtime activates a matching policy revision after preparing its Xray engine, so edits cannot redirect the old running backend through a different policy.

## Implemented workflows

- Import a subscription, supported share link, QR code or file.
- Select a node inside a group, restore automatic selection, or use US/Japan/Hong Kong priority groups.
- Use global routing or simple domain/IP rules, with an explicit unmatched destination.
- Inspect active connections separately from bounded recent history, including the chosen rule, group chain and node.
- Start and stop the platform VPN, handle its permission request, and restore connection on app launch when enabled and authorized.

Unsupported imported rules produce diagnostics rather than being silently widened. iOS does not offer desktop-style application rules or dynamic Direct after packet capture; local networks use static system exclusions. Android direct sockets are protected from re-entering the VPN. Process attribution, IPv6-only networks, network handovers and device energy behavior still require device acceptance.

## Remote builds

The `Xray mobile` GitHub Actions workflow builds pushes to `feat/xray-mobile`. It compiles Rust, exports both JavaScript bundles, generates the two Xray native libraries and builds the applications. It does not publish or change desktop releases, tags or Sparkle feeds.

Build outputs stay in Actions artifacts:

- Android: ARM64 APK with its JavaScript and native libraries bundled. Initial development builds use the template's development signing identity; permanent distribution signing must be configured before public distribution.
- iOS: ARM64 simulator application and unsigned device archive, including the Packet Tunnel extension. The archive is not an installable signed IPA. Device distribution requires the iOS app/extension/App Group signing configuration.

No simulator, SDK or dependency installation is needed to edit this checkout. `scripts/mobile/build-*.sh` run in CI. Package and core lockfiles are committed. Compilation and packaging are separate from device/network testing, which the user explicitly deferred for this implementation phase.

The application name remains MyProxy and the interface identifies the Xray channel. The base version follows the desktop Cargo package. iOS build numbers use the workflow run and attempt; Android uses a monotonically increasing integer derived from them.

## References

Pinned network dependencies and their licenses are recorded in [THIRD_PARTY.md](THIRD_PARTY.md). The Expo native module uses [Expo Modules](https://docs.expo.dev/modules/overview/); platform integration follows [Apple Packet Tunnel](https://developer.apple.com/documentation/networkextension/nepackettunnelprovider) and [Android VpnService](https://developer.android.com/develop/connectivity/vpn).
