# MyProxy PR115 boundary CA policy result

## Task status

Complete locally: tempfile lock drift removed, exact CA allowlist in `scripts/check-xray-boundary.py`, expanded `scripts/test-xray-boundary.py`, commit pushed to PR branch pending CI.

## Head / base

- Branch: `fix/rust-ca-backport-xray-87f`
- Base (xray installed): `87f1f87c775d8899878d87ae8ff7656009151b15`
- Prior HEAD: `b56415e461f271decae27a4ccaf8ccb2abd86493`

## Diff vs `87f1f87` (non-runtime)

| Path | Change |
| --- | --- |
| `Cargo.lock` | Only approved CA bumps + restored `tempfile` → `getrandom 0.4.3` (removed erroneous `0.3.4` edge) |
| `scripts/check-xray-boundary.py` | `normalize_approved_ca_updates()` for three reviewed registry packages; `webpki-roots 1.0.10` dep refs when roots package bumps |
| `scripts/test-xray-boundary.py` | Real-lock tests: accept baseline & exact CA patch; reject tempfile edge, bad checksum/source/version, extra CA deps |

Unchanged vs `87f1f87`: `src/**/*.rs`, `packaging/**`, `.github/workflows/**` (protected set), `Cargo.toml`, compiler/backend/feed guards in checker `main()`.

## Approved CA records (only)

- `rustls-webpki` 0.103.15 → 0.103.16, checksum `4443b1b6…`
- `webpki-roots` 1.0.9 → 1.0.10, checksum `1c1c8e20…`
- `webpki-root-certs` 1.0.9 → 1.0.10, checksum `58ec80e7…`
- Allowed dep refs: `webpki-roots 1.0.10` (e.g. `gpui-pre-http-client-tls`, `webpki-roots` 0.26.11 wrapper)

## Local evidence

| Command | Exit |
| --- | ---: |
| `python3 scripts/test-xray-boundary.py` | 0 |
| `MYPROXY_BOUNDARY_BASE=87f1f87… python3 -c '…verify_version_metadata…'` (lock only) | 0 |
| `cargo check --locked` | skipped (`cargo` not on host; `RUSTUP_AUTO_INSTALL=0`) |

## CI / gates not run here

- Full Xray `workflow_dispatch` / macOS `cargo test --locked` / signed package publish
- Default `MYPROXY_BOUNDARY_BASE=67dd…` full `check-xray-boundary.py` `main()` (67dd not in shallow worktree; compile strip guard unchanged)

## Runtime / infra

No merge, tag, publish, install, proxy, System Extension, or credential changes.
