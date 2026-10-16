# Verification ledger — executed, not inferred

Audit date: 2026-10-15. Branch `fix/repository-remediation`; upstream HEAD remains
`0817c50175cc6c8714776fff9467684ecab98426`. No commits/pushes. Earlier reports and
benchmarks retain their own source/method provenance and are not relabeled here.

## Final executed local results

| Actual check | Result | Reproducible evidence |
|---|---|---|
| Expanded production-module/application/QR/IAM/SSM SDK suite | **146 passed, 0 failed, 3 intentionally ignored**; final three repeats all passed (2.47s / 2.40s / 2.39s execution; compilation additional) | `evidence/expanded-suite-final.log`, `expanded-repeat-{1,2,3}.log` |
| Actual complete Linux application executable build | passed; no stub main/EC2 replacement | `evidence/actual-application-build.log` |
| Actual application offline subprocess tests | **25 passed**; includes safe/explicit export, no overwrite, insert/edit, config origin, profile trust and 12 unique final-log flush events | `evidence/actual-cli-final.log`, `tools/audit_cli_test.py` |
| Strict expanded-harness all-target Clippy | passed, `-D warnings` | `evidence/expanded-harness-clippy.log` |
| Strict complete-root Linux all-target Clippy | passed, `-D warnings`, no six old category exemptions | `evidence/linux-root-clippy.log` |
| Complete-root Linux all-target type-check | passed | `evidence/linux-root-check.log` |
| Strict complete-root Windows-GNU all-target Clippy | **passed**, including cfg(windows) application/test code | `evidence/windows-root-clippy.log` |
| Complete-root Windows-GNU all-target type-check | **passed** | `evidence/windows-root-check.log` |
| Stable root/harness rustfmt and Git whitespace checks | passed after correcting an intermediate in-flight formatting failure | commands below; final check exit 0 |
| Actual pinned Go-smux peer build/vet, frozen source hashes and interoperability | **two cases passed**: slow first stream and large multistream duplex/FIN drain | `evidence/go-interoperability.log`, `vendor-hashes.log` |
| Release native writer benchmark | separately invoked ignored test passed; all 24 paired samples had 64 writes / zero duplicates | `writer-benchmark-paired.json`, `evidence/mfz-benchmark-*-paired.log` |
| Final lock alignment | all **416 shared registry version/checksum pairs** match; no extra conflicting shared-name version | executed Python/tomllib assertion; both locks |
| Exact OSV versions from both locks | **428 distinct versions; two maintenance-notice matches**, fxhash/paste; no prior security-advisory versions matched | `dependency-scan-remediated.json`, `evidence/dependency-rescan.log`, `DEPENDENCIES.md` |
| CI/scripts | Python AST parsing and CI YAML/triggers/all-target/credential/actual CLI commands validated | source scripts/workflow; remote CI NOT executed |
| Known legacy material exclusion and old archive integrity | no exact known-value match in current package sources; all three previous ZIP byte counts/SHA256 unchanged | `material-scan-final.json`, `preserved-artifacts.json`, MFZ-A26 |

Windows boundary-only checks also passed earlier; the complete Windows root checks
above supersede their compilation scope. **Neither cross-check executes Windows
APIs or links/runs a real Windows mstsc workflow.** The actual executable used for
offline CLI checks was the real Linux root application with the actual EC2/SDK
dependencies, not a reduced/stubbed dispatcher.

## Exact principal commands

Environment used for builds: one Cargo job, `CARGO_PROFILE_DEV_DEBUG=0`; Rust/Cargo
1.99.0, Clippy 0.1.99, rustfmt 1.10.0-stable; Go 1.27.2. MinGW/windows GNU Rust target
were installed; temporary sandbox swap was used for EC2 compilation, then **both
swap files were disabled and removed**. Full tool metadata is in `toolchains.json`.

```text
cargo test --manifest-path tools/transport-tests/Cargo.toml --locked --features session-sdk,enrollment-sdk
cargo clippy --manifest-path tools/transport-tests/Cargo.toml --locked --all-targets --features session-sdk,enrollment-sdk -- -D warnings
cargo build --locked --bin mfz
python3 tools/audit_cli_test.py --binary <actual target/debug/mfz>
cargo clippy --locked --all-targets -- -D warnings
cargo check --locked --all-targets
cargo clippy --locked --all-targets --target x86_64-pc-windows-gnu -- -D warnings
cargo check --locked --all-targets --target x86_64-pc-windows-gnu
cargo fmt --all -- --check
cargo fmt --manifest-path tools/transport-tests/Cargo.toml --all -- --check
git diff --check
cargo test --manifest-path tools/transport-tests/Cargo.toml --locked smux_go_interop -- --ignored --nocapture
python3 tools/audit_dependencies.py
```

No assertions or success criteria were weakened to obtain these results. Existing
narrow source attributes for an upstream-fixed WebSocket callback return type,
MSRV-compatible deprecated atomic alias and transport-only harness unused SDK entry
points are retained for their documented API/subset reasons; they are not the six
old complete-root lint-category command-line exemptions. No new blanket warning
suppression was added.

## Test authenticity and limits

The harness imports actual production `.rs` modules by path and includes the
original config/hosts/RDP/shell/error/MFA integration suites plus the new audit/QR/
SDK tests. It does not copy substitute implementations. Real AWS SDK serializers,
signing, provider/profile loading and production session/enrollment callers talk
to bounded local HTTP/WebSocket peers. OS/cloud boundaries are isolated only where
explicitly stated; no live AWS credentials or endpoints were used.

The suite covers secret removal/readback/write-failure retention, mutation locks,
strict parser/source selection, file/RDP/hosts injection, actual QR PNG and independent
TOTP interpretation, image bounds, optional token headers, selected-profile region/
identity, no partial-MFA fallback, unique/bounded EC2 selection, IAM pending cleanup,
activation/save failure and actual closed-input behavior.

Default ignored tests are two Go interoperability cases and one opt-in writer
microbenchmark; the Go cases AND writer benchmark were separately executed above (all three default-ignored cases). A real Windows
Credential Manager round trip is cfg(windows), explicitly opt-in and random-
identity/disposable; it did not run here. Windows DACL test targets compile but
are not represented as runtime passes.

## Preserved red/unchanged baselines and intermediate checks

- Delivered-v3 unchanged native baseline: **61 passed / 0 failed / 3 ignored**.
- Initial application regressions against actual production modules: **0 passed /
  16 failed**, exit 101. Sources/log retained; no false initial-green claim.
- First config-only progress: **8 passed / 8 failed**, not an overall pass.
- Subsequent original audit regressions: **16 passed / 0 failed**.
- Expanded progress: **139 passed / 0 failed / 3 ignored**, then final **146** after
  neighboring EOF/URI/empty-password/ownership/batch/config-origin cases.
- Actual earlier binary explicitly reproduced wrong source selection for
  `--config mfz.ini` (new case FAILED before preserving option origin). Same
  actual-CLI case now passes after the correction.
- Initial dependency update hit a minimum smithy-runtime/types constraint; minimum
  compatible versions were then selected, not an unrelated broad SDK upgrade.
- Parallel complete Linux/Windows EC2 metadata checks nearly exhausted sandbox
  memory; Windows was deliberately stopped and rerun **serially to success**.
  First actual full binary code generation took 17m41s, decoder-feature graph
  rebuild 16m52s; final cached rebuild passed in 21.72s. Build-resource cost is not
  presented as an application/guest throughput measurement.
- One formatting check caught a late Windows orphan-association edit; formatting
  was corrected and complete final checks rerun. No unresolved failure is hidden.

Selected durable logs are included under `evidence/`; larger unchanged historical
baseline/production artifacts remain in `/home/user/mfz-audit-baseline/` and the
preserved earlier delivery files. New reports never repeat old seed values.

## Performance scope and paired check

An initial single 4 MiB release-writer sample differed from historical numbers,
so it was not silently labeled a regression or a gain. The exact preserved-v3
writer and current writer were then run **alternately on this same worker**, three
runs per RTT, same 4 MiB payload. All raw initial/paired samples are retained.

| Simulated RTT | Preserved-v3 median MiB/s | Audit median MiB/s | Audit / v3 |
|---|---:|---:|---:|
| 0 ms | 176.967 | 174.635 | 0.9868 |
| 50 ms | 55.745 | 55.761 | 1.0003 |
| 250 ms | 14.888 | 14.854 | 0.9978 |
| 500 ms | 7.681 | 7.694 | 1.0018 |

Every paired sample recorded 64 transport writes and zero duplicates. This small
same-worker check found no large drift in the writer scope; three samples are
**not** statistical equivalence, a new speedup claim or proof about TLS/AWS/RDP/
guest performance. Six packet/cleanup hot modules remain byte-identical to v3.
The historical baseline source was compiled read-only (`--locked`, external target
directory) for this comparison, not deployed or treated as a sanitized release.

```text
MFZ_BENCH_MIB=4 MFZ_BENCH_RTTS=0,50,250,500 cargo test --manifest-path tools/transport-tests/Cargo.toml --locked --release writer_throughput_matrix -- --ignored --nocapture
```

## Unavailable live gates / remaining uncertainty

- Actual Windows filesystem DACL/ReplaceFile recovery states, LSASS/Credential
  Manager durability, DOS mapping races, Known Folders, registry reassignment,
  UAC cancellation/timeout and mstsc credential-target/server-identity semantics.
- Live AWS IAM/session-policy/SSO cache/bootstrap, EC2 eventual consistency and
  real MFA activation/cleanup under ambiguous network results or credential expiry.
- Affected guest clipboard/redirected-drive transfers, whole-VM responsiveness
  and remote CI. Local/Go/writer checks do not prove the guest symptom resolved.
- Process kill/runtime destruction, power failure and noncooperating outside edits
  cannot be made a cross-resource transaction; any recovery copy/cleanup uncertainty
  is reported. Old backups/history may retain secret material.
- Prior archived MFA seed-like values may be genuine or revoked; no local source
  change determines/revokes them. Replace corresponding devices if genuine.
- Known advisory data/features were inspected, not every line of third-party
  dependencies or every future vulnerability. Residual maintenance notices remain
  visible, with no permanent zero-vulnerability certification.

Precise external checks/expected outcomes: `WINDOWS_ACCEPTANCE.md`. There is no
database, frontend/HTTP application route or tenant schema to test.
