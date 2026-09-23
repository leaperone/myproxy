# MyProxy mobile integration contract

The current implementation uses Expo presentation, native VPN ownership, shared existing Rust policy, and an embedded Xray executor. No local SDK/dependency installs or builds. Build on GitHub Actions. User explicitly postpones device testing; deliver implemented features and honest build results, never fake runtime state. Desktop application/releases/configuration stay intact.

## Ownership

- `crates/myproxy-core`: extracted existing Rust strategy/catalog/parser/node-renderer/policy. Desktop uses the same implementation. Platform storage/network adapters may differ.
- `crates/myproxy-mobile`: Rust native C/JNI facade and mobile document orchestration; delegates business decisions to shared core, never rewrites matching/fallback.
- `mobile/app`: Expo UI, package/config and TypeScript client.
- `mobile/app/modules/myproxy`: local Expo module with Swift/Kotlin control; no policy in JS/Swift/Kotlin.
- `mobile/ios-extension`: Packet Tunnel target sources, no Expo/React Native linked into this target.
- `mobile/network`: single Go runtime binding with Xray and a mature packet stack. Root owns this directory and build scripts/workflow.

## Control bridge

Native Expo module `MyProxy`, one async method `request(requestJSON: string): Promise<string>`. All responses are JSON envelopes:

```json
{"ok":true,"data":{}}
{"ok":false,"error":{"code":"invalid_config","message":"用户能理解的中文错误"}}
```

Native runtime commands: `snapshot`, `connect`, `disconnect`, `probe`, `refreshSource`, `addSource`. All other commands delegate to Rust. `snapshot` merges Rust view with actual native VPN/runtime observations. The UI must not simulate successful connection when native module is unavailable.

Public command examples:

```json
{"op":"snapshot"}
{"op":"connect"}
{"op":"disconnect"}
{"op":"import","text":"vless://...","name":"我的节点"}
{"op":"addSource","url":"https://...","name":"我的订阅"}
{"op":"refreshSource","id":"source-id"}
{"op":"removeSource","id":"source-id"}
{"op":"select","group":"节点选择","member":"美国优先"}
{"op":"select","group":"美国","member":"某个节点"}
{"op":"select","group":"美国","member":""}
{"op":"setMode","mode":"global"}
{"op":"setMode","mode":"rule"}
{"op":"setAutoConnect","enabled":true}
{"op":"setFallback","via":"节点选择"}
{"op":"saveRule","id":null,"name":"示例","kind":"domain-suffix","value":"example.com","via":"日本优先"}
{"op":"deleteRule","id":"rule-id"}
{"op":"probe"}
```

Native HTTP fetch uses explicit bounded time/size and no log of URLs/credentials. Once fetched, pass Rust `import` with `sourceId` for refresh, `sourceURL`, `name`, `text`. Invalid refresh preserves the old document. Native persists Rust `export` only after success, private/atomic. Persisted state is a document, not an Expo AsyncStorage replica. Selection changes while running update the provider/service Rust state and close old connections; changed nodes restart the engine transactionally or reconnect explicitly while retaining desired connection state.

## Snapshot

Every successful public mutation returns a new snapshot. Rust produces these fields; native adds `runtime`. All fields listed are required; unavailable values are null, empty collections remain empty.

```json
{
  "revision":1,"mode":"global","autoConnect":false,"selected":"节点选择","fallback":"节点选择",
  "capabilities":{"dynamicDirect":false,"appRouting":false},
  "sources":[{"id":"id","name":"订阅","url":"https://...","nodeCount":5}],
  "nodes":[{"name":"节点","protocol":"vless","source":"订阅","available":true,"error":null,"delayMs":null}],
  "groups":[{"id":"id","name":"节点选择","kind":"select","selected":"美国优先","resolved":"节点","members":[{"name":"美国优先","kind":"group","available":true,"delayMs":null}]}],
  "rules":[{"id":"id","name":"示例","kind":"domain-suffix","value":"example.com","via":"日本优先"}],
  "warnings":[],
  "runtime":{"phase":"disconnected","message":null,"connectedAt":null,"uploadBytes":0,"downloadBytes":0,"connections":[]}
}
```

Runtime phases: `disconnected`, `connecting`, `connected`, `disconnecting`, `error`. Flow rows: `{id, active, host, port, network, outbound, rule, chain: string[], uploadBytes, downloadBytes, startedAt}`. Times are Unix milliseconds. Engine returns bounded recent/active flow rows, newest first; active connections and ended history must be displayed distinctly. Display IP when hostname is unobserved; do not invent app attribution. Connection records cover intercepted traffic only.

## Rust native ABI and private commands

`const char *myproxy_mobile_call(const char *request_json); void myproxy_mobile_free(const char *response);` C strings are allocated/freed in Rust. Catch errors/panics at FFI boundary, no global CWD or env changes. C header at `crates/myproxy-mobile/include/myproxy_mobile.h`. On Android Java `one.leaper.myproxy.core.NativeCore.call(String): String` and library `myproxy_mobile` implement the equivalent JNI entrypoint in Rust. One runtime instance per process, guarded atomically; packet routing must see one coherent policy generation.

Private requests:

- `init {platform: "ios"|"android"}` initializes default groups and capability limits.
- `load {document: string, platform: ...}` loads an exported document into this process after complete validation. Returns core snapshot.
- `export` returns a serialized document string as data.
- `render` returns `{config: string, nodes: [{name,tag}], revision}`. Xray JSON contains outbounds, no listening inbounds and no business routing. Stable node tags `node-<stable id or hash>` map back to node names. No disabled/invalid outbound is rendered.
- `activate {revision}` publishes the prepared Rust policy only after the matching Xray engine is constructed and before it accepts packets. Configuration edits do not alter routing on the old running engine. A mismatched revision rejects activation.
- `route {host, port, network: "tcp"|"udp"}` returns `{action: "proxy"|"direct"|"reject", tag: string|null, node: string|null, rule: string, chain: string[], revision}` using existing policy. Dynamic Direct is an Android-only capability; invalid iOS profile/direct rule must not silently broaden.
- `health {node, delayMs: number|null, failed: boolean}` updates transient node health without persisting runtime metrics as configuration.

Import supports existing share-link/base64/YAML parsing and mobile exported document. Existing desktop strategy imports must explicitly report unsupported conditions; never silently omit an enabled rule or turn it into a broader match. Empty profiles can be edited but cannot connect. Unsupported nodes become individually unavailable with diagnostics. Full imported complex rule sets may be read-only until an editor can preserve all conditions.

## Go mobile engine API (root implementation)

Go module package `mobile`, bound with gomobile to `MyProxyNetwork.xcframework` and `myproxy-network.aar`. Android Java package `one.leaper.myproxy.network.mobile` via `-javapkg one.leaper.myproxy.network`.

```go
type Policy interface {
    Decide(requestJSON string) string // Rust route envelope
    Health(node string, delayMs int64, failed bool)
}
type Protector interface { Protect(fd int64) bool }
type PacketWriter interface { WritePacket(packet []byte) bool }
func NewEngine(renderJSON string, policy Policy, protector Protector, allowDirect bool) (*Engine, error)
func (e *Engine) Start(writer PacketWriter) error // iOS public packetFlow adapter
func (e *Engine) StartTun(fd int64) error         // Android TUN, engine duplicates fd
func (e *Engine) WritePacket(packet []byte) error // iOS readPackets -> stack
func (e *Engine) Close()
func (e *Engine) CloseConnections()
func (e *Engine) Snapshot() string              // runtime object, not envelope
func (e *Engine) Probe()                        // bounded asynchronous probes + Policy.Health
```

One Go runtime per VPN process. Android every Rust/Go upstream socket needs protection or platform bypass; native subscription download occurs outside the VPN service packet loop. iOS no public/private SOCKS listeners: packetFlow -> existing userspace stack -> per-flow Rust decision -> Xray core.Dial forced tag. DNS resolver configured for the tunnel travels over the selected proxy route; do not implement a whole-system DNS interception product. Dynamic direct is absent on iOS. Static local network exclusions use documented system routes.

## Product and build

Brand MyProxy, channel Xray; Chinese default, system light/dark, ordinary-user language. Three main tabs: 连接, 节点, 设置; recent connection details from connection screen. Region groups and direct node selection must expand inline. No fake/demo nodes or misleading ready state.

Expo dependency versions must be pinned to a coherent SDK; root generates lockfiles remotely. Do not install npm/cargo/Go/iOS/Android SDK dependencies locally. iOS native project and extension creation can be an explicit reproducible generation script, no hand edits lost by prebuild. Builds are GitHub workflow artifacts, not new desktop releases/tags. Android initial CI APK development-signed; iOS simulator app and unsigned device archive until iOS distribution credentials exist. This is an implementation/build milestone, not device/network/store acceptance.
