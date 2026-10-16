# Native file-transfer fix: production completion

Date: 2026-10-15. MFZ base: `0817c50175cc6c8714776fff9467684ecab98426`.
Official reference clone: AWS Session Manager Plugin
`6430675a44032a551be1937fb27a2d3fdfeb27f7`.
Working branch: `fix/native-transfer-production`. No commit/push was made.

## Scope and acceptance

The implementation remains **native AWS Rust SDK + Rust WebSocket/SSM/smux**.
There is no AWS CLI, Session Manager Plugin or Go child process in MFZ's session
runtime. `mstsc.exe` remains the Windows RDP client, as before. The pinned Go smux
fixture is only an opt-in development interoperability peer.

Both clipboard file copy and redirected drives use the same RDP TCP stream.
They receive the same transport fixes; drive redirection is not a second tunnel.
The [performance report](native-transport-performance.md) describes lossless
bounded buffers, selective-ACK sequence-span credit, fair stream scheduling,
immediate ready batching, allocation retention control and adaptive exact-byte
retries. No fixed per-packet sleep or flush-per-packet bottleneck remains.

| Requirement | Implementation/verification |
|---|---|
| Native functionality | SDK StartSession/TerminateSession, SigV4 WebSocket upgrade, SSM handshake/ACK/retry and smux v1 in Rust; no external AWS/plugin runtime. |
| Learn from official plugin | Pinned streaming, config, multiplexed forwarding and actual unchanged Go smux reference; two executed cross-language tests. |
| Avoid freeze-related transport defects | No dropped acknowledged bytes, unbounded queues, ACK starvation, whole-window replay storms or detached relay children; bounded backpressure and explicit session cleanup. |
| Remove avoidable performance bottlenecks | No sleep-per-KiB, no batching linger, bounded fair batching, indexed ACK/timer state and shared immutable wire buffers. |
| Production integration/quality | Whole Windows-target app and test-target type-check, portable and actual-SDK loopback regressions, lint/format checks, deadline/cancellation ownership, CI jobs. |
| Prove the entire-VM freeze is resolved | **Not established offline.** Requires actual Windows/AWS/RDP/guest monitoring on the affected configuration. |
| Absolute maximum performance / zero bottlenecks | Not a meaningful universal guarantee. Network/SSM credit/TCP/RDP/guest CPU/disk remain physical constraints. |

## Application-level completion after performance-v2

1. **Resolved signing region.** WebSocket signing uses the actual SSM SDK client's
   resolved region, including the environment/profile/provider chain. The old
   `ap-northeast-1` fallback is removed. A real-SDK/local-WebSocket test verifies
   `eu-west-1` when the MFZ region override is absent.
2. **Validated StartSession response.** Session ID, stream URL and token must be
   present/nonempty. A cleanup owner is installed as soon as a real session ID
   exists, before subsequent fallible/awaited operations. A missing ID cannot be
   terminated because no ID exists; the error is explicit, never invented.
3. **Native remote-session termination.** Failed starts, failed readiness, retry,
   normal shutdown and unexpected native transport exit request the real SDK's
   `TerminateSession`. One cleanup actor per created session is bounded to 10 s,
   including SDK retries. Explicit application paths await it; Drop requests
   best-effort cleanup without blocking an executor thread.
4. **Cancellation-safe local shutdown.** The handle retains ownership of the
   parent while awaited. Cancelling the shutdown Future cannot detach its task.
   Normal shutdown waits for the parent and owned relay children to release;
   a 2 s abort fallback prevents hanging exit. Abort-before-first-poll also emits
   completion so remote cleanup is not missed.
5. **Real readiness, not heuristics.** Public readiness waits for authentication,
   the complete agent handshake and actual TCP bind, and reports the task error.
   A live task is not treated as a ready listener. No TCP readiness probe or
   polling interval opens an unwanted smux stream. Preflight checks bind locally
   rather than connecting to a possibly live forwarder; the real bind is still
   authoritative and handles the unavoidable preflight race.
6. **No artificial RDP startup delay.** The old 1.5 s stabilization sleep is gone;
   the app launches RDP only after verified readiness. RDP-launch failures still
   await session cleanup. SSM-only mode also exits on transport failure rather
   than waiting indefinitely for Ctrl+C.
7. **Retry policy remains bounded.** The SDK handles transient control-plane
   retries. Permission/MFA/config/API failures are surfaced, not blindly retried
   by another tight loop. Up to three native readiness attempts remain; each
   cleans up before creating another remote session.

This follows the official plugin's important lifecycle principle too: its
[resend-timeout handler](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/sessionmanagerplugin/session/session.go#L108-L126)
terminates an unhealthy session, and its
[SDK termination path](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/sessionmanagerplugin/session/sessionhandler.go#L226-L257)
explicitly calls SSM TerminateSession. MFZ implements that principle natively,
with finite deadlines and owned/cancellation-safe cleanup rather than copying
polling goroutines or using the plugin executable.

## Permissions and cleanup limitations

In addition to existing session-start permissions, the MFA session identity must
have **`ssm:TerminateSession` on its own SSM session resource**. Use your account's
least-privilege session policy; do not grant unrestricted session termination.
Expired credentials, IAM denial, lost network or runtime/process termination can
prevent the API call. Cleanup errors are logged and surfaced by explicit shutdown;
there is no claim that Drop, process kill or abrupt OS exit guarantees remote
termination. Check service-side session/agent-worker cleanup during live tests.
Automatic ResumeSession reconnect and KMS-encrypted sessions remain outside this
port-forwarding fix. Unsupported KMS fails explicitly.

## Local validation and reproducibility

- Default portable suite: **61 passed**, three opt-in tests ignored.
- With `session-sdk`: **64 passed**, including three additional real SDK/local
  HTTP/WebSocket tests. The actual production `ssm.rs`, `mfa.rs`, `ui.rs` and
  cleanup module are included, not duplicate method implementations. No AWS
  account or credentials are used.
- Actual AWS-vendored Go smux: two opt-in interoperability tests.
- Strict portable and SDK-enabled Clippy without lint overrides; Rust formatting.
  Whole-repo `-D warnings` additionally encounters existing out-of-scope lint
  findings in CLI/hosts/RDP/shell/config/error tests. Those files remain unchanged;
  transport/session quality checks are not weakened to hide them. A separate full
  app/all-targets lint passes when only the six pre-existing lint categories
  are allowed; that is not represented as an unconditional strict-root pass.
- Full root `cargo check --locked --target x86_64-pc-windows-gnu --all-targets`:
  **passed**, including the application, native SDK wiring and test targets.
  This is a type-check, **not** Windows execution or a linked app release.
- Harness dependency package versions/checksums match the root lockfile; vendor
  file hashes and patch/source package integrity are checked separately.
- CI includes Linux/Windows transport and SDK loopback jobs, plus a full Windows
  app check/test job. The workflow is supplied, not claimed remotely executed.

```bash
cargo test --manifest-path tools/transport-tests/Cargo.toml --locked
cargo test --manifest-path tools/transport-tests/Cargo.toml --locked --features session-sdk
cargo clippy --manifest-path tools/transport-tests/Cargo.toml --locked --all-targets --features session-sdk -- -D warnings
cargo fmt --all --check
# On Windows with a supported Rust toolchain and Windows SDK:
cargo check --locked --all-targets
cargo test --locked --lib --bin mfz
```

The full cross-check used Rust 1.99.0, one compiler job and no debug info. The
large generated EC2 SDK exhausted the 2 GiB sandbox before the source was
checked; adding 4 GiB temporary build-only swap allowed the full check to pass.
Cross-tool prerequisites were MinGW GCC/binutils, CMake/NASM and resource-tool
PATH aliases; no application build-script changes or SDK upgrades were needed.

## Completion-candidate benchmark rerun

The actual release writer benchmark was rerun with the final source and pinned
lockfile. Three 4 MiB samples per RTT have these median rates:

| Simulated RTT | Production-v3 MiB/s | Earlier stability candidate MiB/s |
|---|---:|---:|
| 0 ms | 208.99 | 0.458 |
| 50 ms | 57.40 | 0.455 |
| 250 ms | **15.04** | **0.444** |
| 500 ms | 7.75 | 0.244 |

All 4 MiB runs: **64 transport write calls, zero duplicates**; the earlier
stability candidate used 4096 calls. At 250 ms the equal-size comparison is
**33.88×**. These are in-memory masked-WebSocket/local-ACK measurements, not
TLS/network/AWS/RDP rates or an official-plugin comparison. The historical
baseline has one sample per RTT; the completion candidate has three.

Extra 16 MiB and 64 MiB runs exercise many 8 MiB credit rounds. At 250 ms they
measured 29.53 and 31.07 MiB/s respectively, with zero duplicates. GNU time's peak
RSS for the **entire benchmark + mock-peer process across all four RTTs** was
17,676 KiB for the 16 MiB matrix and 19,632 KiB for the 64 MiB matrix. User/system
CPU totals were 0.26/0.01 s and 0.99/0.11 s. This is a small process-level resource
sample, not an allocation profile, hard memory bound, full app or guest profile.

All new samples, process-resource observations and final source hashes are in
[`native-writer-production-2026-10-15.json`](benchmarks/native-writer-production-2026-10-15.json).
The [performance-v2 report](native-transport-performance.md) retains its original
measurements/source hashes unchanged; the writer hot-path source is unchanged.

## Required live acceptance gate

Use a non-production instance first. Keep a second connection/management channel
and external guest telemetry independent of MFZ. Compare the original native
build, this build and the official plugin path under identical file sizes and
RTTs, upload/download directions and clipboard/drive redirection. Verify file
hashes, responsive RDP and **other VM services**, guest/client CPU/RSS, disk I/O,
SSM-agent logs, retransmissions and session cleanup after error/stop/retry.

Do not call this a proven whole-OS freeze fix until that gate passes. If guest-wide
unresponsiveness persists with bounded native transport, investigate agent/OS/RDP
virtual-channel, antivirus/disk and driver behavior using the
[original diagnostic checklist](native-transfer-fix.md).
