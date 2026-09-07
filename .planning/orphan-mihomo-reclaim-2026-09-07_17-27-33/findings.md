# 调研记录

## 根因

Mini 早上 pid `94687`（`mihomo -f runtime.yaml`，无 `-d`）占着 7891/1053。后续 connect 再起的进程绑不上口。`disconnect_inner` 在没有 Child handle 时只读 pid 文件，且仅当该 pid 对应 mixed-port 正在 listen 才 SIGTERM；否则打 `ignoring stale mihomo pid without a listening port` 并删 pid 文件，真正的 listener 留下。

## 调用链

- `connect` / `apply` reconnect / `disconnect` / `shutdown` → `disconnect_inner`
- `start_with_catalog` 在 disconnect 之后 spawn；3–8s 等 `127.0.0.1:{mixed}`，超时 kill 新进程并报 port taken
- `is_running`：Child 或 `pid_file_alive` 或 `running_mixed_port` listen
- DNS listen 写死在 `compile.rs` 的 `127.0.0.1:1053`
- controller = mixed+107；NE SOCKS = mixed+1

## 识别边界

只杀名为 `mihomo` 且 exe 等于 `paths::bundled_mihomo()` 或路径含 `myproxy.app` 的进程。
