# Embedded mobile network runtime

One Go runtime contains unmodified Xray-core protocol packages and gVisor's existing TCP/IP stack. Both are pinned to the versions already used by the desktop Xray release. A native Swift/Kotlin policy callback delegates each connection to the shared Rust implementation.

The iOS packet interface uses public `NEPacketTunnelFlow` reads and writes. Android passes a duplicated `VpnService` TUN descriptor and protects all upstream sockets. No HTTP/SOCKS listener is created. Xray receives the selected outbound tag in its dispatch context, not MyProxy group or matching rules.

The engine bounds concurrent flows, packet queues, DNS correlation and recent records. Internal node probes are excluded from user connection records. Observed DNS names are not assigned to an IP when multiple names share it. DNS datagrams use TCP to the chosen resolver through the selected node. Direct traffic is enabled only by the Android adapter.

Build on GitHub Actions with the repository mobile scripts. A successful build is not a claim of device/VPN acceptance; that phase is intentionally deferred by the user.
