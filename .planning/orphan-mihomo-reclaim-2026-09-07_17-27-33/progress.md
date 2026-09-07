# 进度

## 当前

实现与单测已完成。Mini Mixed 已写成 `proxy` 并已发出重启。

## 已完成

1. `disconnect_inner` 改为 `reclaim_owned_mihomo`：按 exe 识别并 SIGTERM/SIGKILL 本 bundle 残留核心。
2. `DNS_LISTEN_PORT` 抽出，回收 Mixed / NE SOCKS / controller / 1053。
3. `cargo test --lib -- supervisor:: compile::`：22 passed。
4. Mini：`mixed-mode proxy` + apply；osascript restart 已发出。
