---
name: release
description: 发布 myproxy 的 Prod 或 Nightly 版本，或把 Developer ID 应用公证后再装到真机。用于 /release patch、/release minor、/release major、/release nightly、$release、公证、notarize、notarytool、System Extension 安装、Unnotarized Developer ID。
---

# myproxy release

入口：`/release <patch|minor|major|nightly>`；Codex 也可用 `$release patch` 等形式调用。只接受一个参数；缺失或未知参数时说明用法，不默认发布。创建、修改本技能或讨论发布方案不构成一次发布调用。

仓库固定为 `leaperone/myproxy`，源分支为 `main`。执行发布的调用授权本次对应通道的版本提交、PR、合并、tag、workflow dispatch 和 GitHub Release；沿用仓库规则及已有授权，不重复询问。

## 版本契约

正式版增量以 **最新已发布 Prod release** 为基线，不以已前移的 `Cargo.toml` 为基线：

| 参数 | 最新 Prod 为 v0.0.3 时 | 行为 |
| --- | --- | --- |
| patch | v0.0.4 | 补丁版本 +1 |
| minor | v0.1.0 | 次版本 +1，补丁归零 |
| major | v1.0.0 | 主版本 +1，其余归零 |
| nightly | v0.0.4-nightly.20260905.42.1 | main 的目标版本 + UTC 日期 + CI 流水号 + 重试次数 |

`Cargo.toml` 保存下一目标版本。Nightly 不消费正式版本号；具体版本/tag 由 [release.yml](../../../.github/workflows/release.yml) 生成，Agent 不另造时间戳。Git tag 是 `v` 加应用版本；Sparkle 用 `CFBundleVersion=run_number.run_attempt` 排序。徽章只反映构建：Dev、Nightly 显示，Prod 不显示。

## 公证

要装到 Mac 并激活 System Extension 的 Developer ID `.app`，必须先走仓库里这条公证链路。不要另写签名或 `notarytool` 流程。

正式 Prod 和 Nightly 由 [release.yml](../../../.github/workflows/release.yml) 调用 [release-macos.sh](../../../scripts/release-macos.sh)，脚本再调用 [notarize-macos-app.sh](../../../scripts/notarize-macos-app.sh)。CI 缺 `APPLE_ID`、`APPLE_APP_SPECIFIC_PASSWORD`、`APPLE_TEAM_ID` 或 Developer ID 证书时直接失败。Nightly 只构建 `main`。未合并分支不在 Nightly 里。

未合并分支或本地要在真机上跑新的 System Extension 时，用同一套脚本和同一组环境变量名。只检查变量是否存在，不打印值。

1. 确认 `APPLE_ID`、`APPLE_APP_SPECIFIC_PASSWORD`、`APPLE_TEAM_ID` 已设置。
2. 设置 `MYPROXY_HOST_DEVID_PROFILE_PATH` 和 `MYPROXY_NETWORK_EXTENSION_DEVID_PROFILE_PATH`。可从上一份已公证安装的 `myproxy.app` 复制 embedded profile。没有 profile 就不要装。
3. 钥匙串里有多把 Developer ID Application 时，把 `CODESIGN_IDENTITY` 设成其中一把的 40 位 hash。[select_developer_id_identity.py](../../../scripts/select_developer_id_identity.py) 在未指定时若看到多把会退出。
4. 运行 `scripts/package-macos-app.sh`。
5. 运行 `scripts/notarize-macos-app.sh target/release/myproxy.app`。Accepted 之后脚本会 staple。不要设 `SKIP_NOTARIZE=1`。
6. 用 `spctl -a -vv --type install target/release/myproxy.app` 确认是 Notarized Developer ID。不要用 `stapler validate` 当门禁。Command Line Tools 下它常失败，而 `xcrun stapler staple` 和 `spctl` 仍可用。
7. 装到设备并启动后，`systemextensionsctl list` 必须显示这次 `CFBundleVersion` 为 activated enabled。主程序版本新、扩展仍是旧版时，还没有跑到新扩展。

不要安装 Unnotarized Developer ID。macOS 会以 `OSSystemExtensionErrorDomain` 8 拒绝新扩展。不要只替换 host 二进制来代替新扩展。不要为了测未合并分支去 dispatch Nightly。

## 发布前

- 读取仓库规则、当前 [release workflow](../../../.github/workflows/release.yml)、[打包脚本](../../../scripts/release-macos.sh)、[公证脚本](../../../scripts/notarize-macos-app.sh) 与 [产物门禁](../../../scripts/check-release-artifacts.py)。确认 Git remote 指向本仓库、`gh` 可用；只检查 secret 名称/可用性，不输出值。
- Fetch `origin/main` 和 tags，读取 `gh api repos/leaperone/myproxy/releases/latest`，确认它是非 draft、非 prerelease 的三段 Prod 版本。读取发布 workflow 的 queued/in_progress run；已有发布在途时先核对，避免重复触发。
- 需要修改版本时遵循 `worktree` 技能，在任务专用 checkout 操作，保留其他 dirty 修改。版本文件只同步 `Cargo.toml`、`Cargo.lock` 中 myproxy 包、两个 `packaging/macos/**/Info.plist` 的版本字段。按仓库规则 commit、push、PR、`preflight`；最终未合并不得打正式 tag。
- 将主线已有的目标版本与算出的正式目标按数字元组比较。若正式目标低于主线目标，报告版本冲突并请用户选择发布级别，不静默降版或改选 minor/major。

## patch / minor / major

1. 根据最新 Prod 算出目标版本；检查本地/远程同名 tag、draft 和已发布 release。已发布的版本不可覆盖；已有 tag 或 draft 按下方恢复规则处理。
2. 若 `origin/main` 的版本已等于目标，无需空提交或空 PR；否则完成上述版本 PR。固定已合并、CI 通过且版本匹配的源 commit，发布前重新核对最新 Prod 未变化。
3. 给该 commit 创建注释 tag `v<version>`，只 push 这个 tag，不用 `--tags`、不 force。tag push 会触发 Prod workflow，不再同时 dispatch 第二次。记录目标 tag、SHA 和对应的 Release run ID。
4. 按“核验与恢复”确认 Prod 已公开且 feed 正确。随后通过独立小 PR 将 main 的开发基线推进到下一 patch 版本，供后续 Nightly 使用；若 main 已有更高目标版本则保留。版本前移未合并时如实报告，不改写已发布 tag。

## nightly

1. 从最新 `origin/main` 构建，不发布未合并的本地分支。目标版本必须高于最新 Prod；若 main 尚停在已发布版本，先按版本 PR 流程前移到下一 patch 目标。
2. 调用现有工作流：

   ```sh
   gh workflow run release.yml --repo leaperone/myproxy --ref main -f channel=nightly
   ```

3. 记录此次 dispatch 的 run ID，跟踪它生成的不可变版本 tag。Nightly release 必须是 prerelease；`nightly` 是固定 feed 指针，不能成为 latest Prod。产物源 SHA 从实际版本 tag 核对，不以 dispatch 时的本地 HEAD 代替。

## 核验与恢复

- 跟踪本次 Release run 到结果，等待期间保持简短状态更新。失败先检查对应步骤与必要日志；工作流已成功只证明 CI 已公证并发布，不证明设备上的 System Extension 已换成这次构建。不启动 UI 手动测试。本地真机核验按「公证」第 6、7 步。
- 核对 run success、版本 tag 的源 SHA、release 的 draft/prerelease/latest 状态和 zip/appcast 附件。下载本次公开 feed 与 archive 到临时目录，复用 `scripts/check-release-artifacts.py <channel> <version> <build_number> <tag> --dist <dir>`；它校验元数据和签名存在性，不代表密码学验签。
- Prod 核对 `releases/latest/download/appcast.xml` 指向本次版本。Nightly 核对 `releases/download/nightly/appcast.xml` 指向本次不可变 tag，并确认 latest Prod 未变化。
- 已有公开 release 时先核验是否已经完成，禁止覆盖资产或强推 tag。已有 tag/draft 时检查源 SHA、run 与资产；不能证明可安全续接就报告具体恢复条件，不删除或盲目重发。
- 未发布且没有 draft 时，瞬时失败最多自动新建一次 workflow run：Prod 用 `channel=prod` 和原有 tag；Nightly 用 `channel=nightly`。不重跑旧 run，以免 run_number 小于后续已发布构建。相同失败再次发生则保留日志并报告卡点，不循环发布。
- 最终报告实际通道、版本、源 SHA、run/release 链接、feed 核验结果；Prod 同时报告 main 版本前移状态。queued、pending、draft、失败均不能说已发布。
