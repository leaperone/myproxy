# MyProxy Xray mobile app

This directory contains the Expo presentation layer for the Xray mobile channel. It is intentionally thin: subscription parsing, rule matching, node selection and connection state belong to the shared Rust core and native runtime.

## Local development

The mobile app uses Expo SDK 57.0.24, React Native 0.87 and React 19.2. Expo's bundled native module map pins the companion packages in `package.json`. The repository does not commit a lockfile from a developer machine. CI installs the exact dependency graph and builds the iOS/Android artifacts remotely.

The app requires a custom development or release build because the local `MyProxy` Expo module owns the VPN bridge. Expo Go cannot provide that module. The native module exposes one method:

```ts
MyProxy.request(requestJSON: string): Promise<string>
```

The UI treats the native runtime as authoritative. If the module is missing or returns an incomplete snapshot, the app displays an actionable error and never presents a fake connected state.

## Screens

- **连接** shows the real VPN phase, selected route, byte counters and recent intercepted flows.
- **节点** expands each group inline, supports manual selection, fallback and latency based groups, and adds either a subscription URL, a share link, a bounded-size text/config file, or a QR code.
- **设置** switches global/rule mode, controls auto-connect and edits simple rules while retaining the Rust policy as the source of truth.

The UI is Chinese-first, follows the system light/dark appearance, supports text scaling, and keeps destructive source deletion behind a confirmation. File imports are bounded to 512 KiB and never log their contents. Device and network acceptance are separate from this source milestone; GitHub Actions owns builds to keep developer machines small.
