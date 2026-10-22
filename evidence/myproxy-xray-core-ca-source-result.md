# MyProxy Xray CA source bump (v26.10.21)

## Status

**Complete** — isolated source branch ready as draft stack PR on `fix/rust-ca-backport-xray-87f`. No Rust/Cargo/app/runtime changes.

## PR / branch

| Field | Value |
| --- | --- |
| Head | `source/xray-ca-bump-26.10.21` (from `1e1883e75a259a41b19793460302af60036445d3`) |
| Base (stack) | `fix/rust-ca-backport-xray-87f` |
| Parent task | PR115 / `fix/rust-ca-backport-xray-87f` (not modified) |

## Changed files

- `scripts/fetch-xray.sh` — default `v26.10.21`, GitHub-only download, SHA2-256 verify (pinned digests for 4 host assets; other `XRAY_VERSION` loads official `.dgst` from GitHub). Removed unverified `ghproxy` fallbacks.
- `ThirdParty/xray/NOTICE.md` — version/revision metadata for packaging copy in `package-xray-test.sh`.
- `scripts/check-xray-package.py` — NOTICE `v26.10.21` + bundled `xray` version marker.
- `scripts/test-fetch-xray-sha.py` — local HTTP fixture proves bad checksum rejects install.

## Official release evidence

- Release: [v26.10.21](https://github.com/XTLS/Xray-core/releases/tag/v26.10.21) published `2026-10-21T12:07:11Z` (emSign/emudhra root CA removal, Go 1.27.3; v26.10.10/17 withdrawn).
- Tag commit: `1b1647cd9ddf0abc34b981d4611fc03bcac1ca74`

### Pinned zip SHA2-256 (from release `.dgst`)

| Asset | SHA2-256 |
| --- | --- |
| `Xray-macos-arm64-v8a.zip` | `926cc55fda2a8757dabe4f22dc98772f218ace025f101bd0b68aee232e59d5dc` |
| `Xray-macos-64.zip` | `8e5169b429e76ce5f550c8294db465bc259f30c158ba900aa40802b76ad2f66e` |
| `Xray-linux-arm64-v8a.zip` | `d1304946c486d98e6b886e9b931eaaaebb23835b30e217638037b7f2721d4e9c` |
| `Xray-linux-64.zip` | `0eec3ed4163c273d8c8bf821f7d219e257921cf584aa3fb970715e458e631978` |

### Local binary proof (darwin/arm64, not committed)

```
scripts/fetch-xray.sh → exit 0
resources/xray/xray version → Xray 26.10.21 … 1b1647c (go1.27.3 darwin/arm64)
go version -m resources/xray/xray → go1.27.3, mod github.com/xtls/xray-core
```

## Commands (exit 0)

```text
python3 scripts/test-fetch-xray-sha.py
python3 scripts/test-xray-boundary.py   # 13 tests OK
python3 scripts/check-xray-boundary.py
```

## Tradeoffs

- `XRAY_VERSION` override remains supported but downloads must match SHA2-256 from the official GitHub `.dgst` (no mirror fallback without verification).
- Direct GitHub-only fetch may be slower in restricted networks; mirrors were removed to avoid silent unverified bytes.

## Blockers

None for source scope. Signed `check-xray-package.py` / notary validation awaits hosted CI after stack merge with PR115.

## Invariants

- Protected boundary files unchanged (`fetch-mihomo`, release workflows, Cargo graph, etc.).
- `Cargo` + Rust app sources byte-identical to `1e1883e`.
