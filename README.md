# myproxy

GPUI controller for a bundled mihomo core. Strategy document is the source of truth: subscriptions, exclude filter, groups, mixed-port, and **规则**.

## Dev

```sh
scripts/fetch-mihomo.sh   # Apple Silicon mihomo
scripts/dev.sh            # local window + push debug binaries to macbook-air
MYPROXY_PAGE=rules scripts/dev.sh   # open 规则 page
scripts/install-macbook-air.sh      # release .app → macbook-air, mixed-port 7891
scripts/dev-air.sh                  # Air only: rebuild on save and relaunch
```

Rust 1.98+ (`rust-toolchain.toml`). GPUI has no in-process hot patch; the Air box cannot build GPUI, so `dev-air.sh` compiles here and copies `myproxy` / `myproxyctl` into `~/Applications/myproxy.app`. Air `strategy.json` is not overwritten.

On macOS the app keeps a menu-bar extra: left-click opens the window, right-click shows 打开窗口 / 连接 / 断开 / 更新配置 / 退出.

## CLI (agents)

```sh
cargo run --bin myproxyctl -- capabilities
cargo run --bin myproxyctl -- subscription add 'https://…' --name Neko
cargo run --bin myproxyctl -- filter --set '(?i)(流量|剩余|到期|官网)'
cargo run --bin myproxyctl -- group add Japan --contains jp --contains tokyo --contains 日
cargo run --bin myproxyctl -- group add KittyJP --source Kitty --contains 日 --contains jp
cargo run --bin myproxyctl -- group add PROXY --all
cargo run --bin myproxyctl -- group include Japan 'Kitty · JP-01'
cargo run --bin myproxyctl -- rule add --domain chatgpt.com --via PROXY
cargo run --bin myproxyctl -- rule add --app Arc --via Japan
cargo run --bin myproxyctl -- apply
cargo run --bin myproxyctl -- connect
```

Mixed port is HTTP proxy and SOCKS5 on the same loopback port (local default **17890**; Air install uses **7891**).
