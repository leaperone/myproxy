# Mobile dependency references

| Component | Pin / license | Role |
| --- | --- | --- |
| [Xray-core](https://github.com/XTLS/Xray-core/tree/52a412d9e2f5c2a5142b1b4e2ab3771dacb8b120) | 26.9.9, `52a412d9e2f5c2a5142b1b4e2ab3771dacb8b120`, MPL-2.0 | Unmodified proxy protocol implementations, embedded in one Go runtime |
| [gVisor](https://github.com/google/gvisor/tree/89a5d21be8f0) | `v0.0.0-20260122175437-89a5d21be8f0`, Apache-2.0 | Existing userspace TCP/IP stack; also the version selected by Xray |
| [Go mobile](https://go.googlesource.com/mobile/+/8b95e45f8d3e) | `v0.0.0-20260908204917-8b95e45f8d3e`, BSD-3-Clause | Generated Java and Objective-C bindings |
| [Go](https://go.dev/) | 1.27.1, BSD-3-Clause | Runtime and compiler |
| [Expo](https://github.com/expo/expo) | SDK 57.0.24, MIT | Shared mobile interface and native module integration |
| [React Native](https://github.com/facebook/react-native/tree/v0.86.3) | 0.86.3, MIT | Native presentation runtime |
| [React](https://github.com/facebook/react/tree/v19.2.3) | 19.2.3, MIT | Interface state and rendering |

The exact dependency graph is recorded in `network/go.mod`, `network/go.sum`, `app/package-lock.json` and the workspace Cargo lockfile. Xray's license is included at `../ThirdParty/xray/LICENSE`.

The mobile bridge calls Xray's exported `core.StartInstance`, `core.Dial` and forced-outbound context APIs. It does not fork the proxy protocols. Android linker settings follow the [libXray Android build](https://github.com/XTLS/libXray/blob/5b7c5e07ef358b13acddc6f2b8e33949079f08e7/build/app/android.py), including its `anet` compatibility flag and 16 KiB page alignment; libXray itself is a build reference, not a separately loaded Go runtime.

v2rayNG and other clients were read as architecture references. Their source was not copied into this implementation. Public distribution should preserve the license notices for the complete bundled dependency graph.
