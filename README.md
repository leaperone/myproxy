# myproxy

GPUI controller for a bundled mihomo core. Strategy document is the source of truth: subscriptions, exclude filter, groups, mixed-port, and **规则**.

## Dev

```sh
scripts/fetch-mihomo.sh   # Apple Silicon mihomo
scripts/dev.sh            # restart on .rs/.toml save (needs watchexec)
MYPROXY_PAGE=rules scripts/dev.sh   # open 规则 page
```

Rust 1.98+ (`rust-toolchain.toml`). GPUI has no in-process hot patch; `dev.sh` kills and relaunches the window.

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

Mixed port is HTTP proxy and SOCKS5 on the same loopback port (default **17890**, so it does not collide with Clash/MClash on 7890).
