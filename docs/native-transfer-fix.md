# Native SSM transfer stability: investigation and patch

> **Historical stability-first report:** the queue sizes, fixed pacing and 29-test results below describe the previously delivered candidate. The current checkout is the [performance follow-up](native-transport-performance.md), which supersedes those limits and adds measured batching, adaptive reliability and actual Go smux interoperability. The original diagnosis and Windows/EC2 checklist below remain relevant.

## Status

- Repository: `https://github.com/tatran0195/mfz`
- Base: `0817c50175cc6c8714776fff9467684ecab98426` (`0817c50`)
- Local branch: `fix/native-ssm-transfer-backpressure`
- Reported symptom: the entire remote VM becomes unresponsive during both clipboard and redirected-drive/folder transfers.
- Result: confirmed native-client transport defects have been patched; **29 local transport tests pass, transport Clippy passes with warnings denied, and Windows-target transport compilation checks pass**.
- **Not yet verified:** a full Windows application build, live AWS/SSM integration, or whether this resolves the machine-wide freeze. No AWS credentials or remote-instance access were used. This is a candidate fix to validate on a non-production instance, not proof of an OS-level root cause.

## Confirmed defects

### 1. A full receive queue discarded RDP bytes and closed the stream

`src/smux.rs` used `try_send` to deliver inbound PSH frames. When its 256-entry queue filled, it removed the stream and sent `CMD_FIN`. That is not flow control: it loses part of an already-acknowledged byte stream and terminates the RDP connection.

A local reproduction against the original code queued 257 one-byte PSH frames before reading. It received only 256 bytes and then EOF:

```text
slow_reader_must_not_lose_the_257th_frame ... FAILED
left: 256
right: 257
```

The replacement waits asynchronously for queue capacity. A regression test now transfers 600 numbered frames through a deliberately backpressured stream and verifies every byte, with no synthetic FIN.

### 2. Sending had no effective application-level flow control

TCP writes fed an unbounded smux channel. The forwarding task then sent every 1 KiB SSM chunk as quickly as WebSocket writes allowed, ignored incoming input ACKs, and never retransmitted missing input messages.

A large file could therefore build an arbitrarily large local backlog. A missing SSM input sequence could also stall agent-side reassembly. High-rate traffic is a plausible contributor to remote CPU/memory pressure, but this has **not** been established as the cause of the reported VM-wide freeze.

The official AWS plugin retains unacknowledged input frames, selectively removes them on ACK, retransmits missing messages, and sleeps 1 ms after each multiplexed data send. MFZ previously lacked these behaviors.

### 3. Lifecycle and readiness problems

- A `start_publication` control could prematurely end the handshake. It is not equivalent to `HandshakeComplete`.
- Detached receive/send/connection tasks could outlive the tunnel handle, leaving sockets and buffers alive after stop/retry.
- Joining two copy futures waited for both even if one direction had already failed.

## Implemented changes

| Area | Change |
| --- | --- |
| smux receive | Bounded queue; asynchronous wait for capacity; no eviction or synthetic FIN on overflow. |
| smux send | Bounded FIFO using `PollSender`; normal shutdown queues FIN after data. |
| SSM input | At most 128 unacknowledged messages; matching message type, sequence and UUID are required to release credit. ACKs are selective, not cumulative. |
| Pacing | At least 1 ms between new data messages; no catch-up burst after a delayed write or ACK. |
| Retries | Oldest unacknowledged input is retried after 1 second using the exact original bytes/UUID/sequence; failure is surfaced after 60 retries. |
| WebSocket writes | One writer task owns the sink and prioritizes ACK/Pong/control processing even when the input window is full. Writes time out after 15 seconds. |
| WebSocket receive | Separate bounded reassembly and dispatcher stages. Full TCP queues do not prevent reading input ACKs or Ping/control frames. Overflow that cannot be retained is not ACKed, allowing agent retransmission. |
| Ordering | Duplicate suppression; bounded future-sequence buffer; one slot and byte credit reserved for the missing expected sequence. |
| Handshake | Bind only after the port-session agent handshake completes. Reject malformed/unsupported session actions instead of claiming success. Connect and handshake waits have deadlines. |
| Compatibility | Match AWS's strict version thresholds: multiplexing after 3.0.196.0; smux keepalive only through 3.1.1511.0. Accept service publication controls with unused metadata and space/NUL message-type padding. KMS-encrypted sessions remain unsupported and fail explicitly. |
| Validation | Check nonempty payload digests, framing bounds, and smux commands/control lengths. Keep SSM data payloads at or below 1024 bytes. |
| Cleanup | Owned `JoinSet` tasks are cancelled on stop/error/parent abort; bidirectional copying uses Tokio's standard error and half-close handling. Active connections are limited to 64. |
| Logging | Remove logging of the live SSM token JSON. |

Queue defaults are conservative stability limits, not advertised throughput guarantees:

- Outbound smux FIFO: 64 frames, each at most 32 KiB plus header.
- Per-stream inbound queue: 32 frames, each at most 32 KiB.
- SSM input queue: 128 packets; data packets are at most 1 KiB.
- SSM receive reassembly: at most 512 frames and 1 MiB, plus a 64-frame dispatch queue; each accepted output payload is at most 64 KiB.
- ACK/Pong/control queue: 256 commands.

With 1 KiB chunks and a minimum 1 ms interval, the theoretical new-data ceiling is about 1 MiB/s before scheduling, protocol and network overhead. High latency and the ACK window may lower it further. Stability is the priority; measure before tuning these defaults.

Automatic `ResumeSession` reconnect, KMS support, and explicit `TerminateSession` API cleanup are outside this change. Abrupt stop cancels the local transport but does not guarantee delivery of a remote smux FIN; remote session/worker cleanup still depends on service/agent behavior and session timeouts. Check for stale remote sessions/workers during validation.

No AWS CLI or `session-manager-plugin` process has been added. IAM/STS/EC2/SSM API access still uses the existing Rust SDK; the remote SSM agent is still required. RDP remains `mstsc.exe`. Configuration and RDP sharing settings were not changed.

## Local verification

Environment: Linux x86_64, Rust `1.99.0`; production `src/smux.rs`, `src/ssm_tunnel.rs`, and `src/error.rs` compiled by the lightweight harness.

```bash
cargo test --manifest-path tools/transport-tests/Cargo.toml --locked
cargo clippy --manifest-path tools/transport-tests/Cargo.toml --locked --all-targets -- -D warnings
```

Results:

- **29 passed, 0 failed**; doc tests also passed.
- **Clippy passed** with warnings denied.
- **Windows cross-target compilation check passed** for the transport library and test targets (`x86_64-pc-windows-gnu`). This is a type/compilation check, not Windows test execution or a full application build.
- Changed Rust files passed formatting checks; `git diff --check` passed.
- The tests cover slow consumers, outbound backpressure, FIN ordering, cancellation, input-window saturation, selective ACK matching, exact-byte retransmission, sequence gaps, bounded memory, malformed frames, and handshake/control compatibility.
- Two concurrent clients each upload and download 1 MiB through the real TCP/native relay, with one SSM input frame deliberately dropped. The final streams are byte-for-byte identical (2 MiB in each direction).
- A local TCP/WebSocket mock also exercises the public `start_port_forward_tunnel`/`stop` API, SigV4 upgrade headers with dummy credentials, and a blocked 16 MiB upload. Stop closes all child tasks/sockets and releases the port.

The cross-target check can be repeated with:

```bash
rustup target add x86_64-pc-windows-gnu
cargo check --manifest-path tools/transport-tests/Cargo.toml --locked --target x86_64-pc-windows-gnu --all-targets
```

This suite does **not** emulate the AWS gateway completely or exercise Windows RDP virtual channels. It does not replace the integration check below. The full Windows app/test suite was not run in this Linux sandbox.

## Applying the patch

The workspace contains the cloned, patched `mfz/` checkout on the local branch above. Nothing has been pushed to GitHub.

- `mfz-native-transfer-fix.zip`: patched source tree, including tests and this guide; excludes Git history and generated dependencies/build outputs. Extract it and build from the `mfz` directory.
- `mfz-native-transfer-fix.patch`: Git patch against base `0817c50`, including new test/guide files. In an existing clean checkout at that base, copy the patch outside the repo and run:

  ```bash
  git switch -c fix/native-ssm-transfer-backpressure
  git apply --check ../mfz-native-transfer-fix.patch
  git apply ../mfz-native-transfer-fix.patch
  ```

If your checkout has other changes or a different base, inspect/merge the patch rather than overwriting your work. Do not replace your existing credentials/config with examples from the repository.

## Windows / EC2 validation

Use a non-production VM and keep a recovery/access method available independently of this RDP tunnel. Keep the existing binary for rollback.

1. Install a current stable Rust toolchain and Windows build tools. The locked AWS transport dependencies alone require Rust 1.91.1 or newer; current stable is recommended for the full application.
2. From the patched repository, run:

   ```powershell
   cargo test --manifest-path tools/transport-tests/Cargo.toml --locked
   cargo test --locked
   cargo build --release --locked
   ```

3. Launch using your existing configuration, with debug file logging:

   ```powershell
   $env:RUST_LOG = "mfz=debug"
   .\target\release\mfz.exe --log launch my-server --config .\mfz.ini --share-folder "C:\Transfer"
   ```

   Replace the environment/config/folder with your own values. The rolling log is written as `mfz.log*` in the working directory. Logs may contain instance/session identifiers and should be redacted before sharing; do not share credentials or live tokens.

4. Test both directions, first with a small file, then progressively larger files through `\\tsclient`, then through clipboard copy/paste. Record elapsed time, CPU/memory/disk activity, and the last successful byte count. Compare source/destination hashes with `Get-FileHash`.
5. Repeat the same file and sharing settings with the known-good official-plugin setup as a diagnostic baseline. It is not a runtime dependency of this patch.
6. Stop/reconnect several times, including during a transfer. Confirm the port is reusable and no old local tunnel remains connected.

### If the whole VM still freezes

A tunnel bug can explain a frozen/disconnected RDP session; it is insufficient evidence of an operating-system hang. Establish health through an **independent** path, not another session over the same stalled connection:

- EC2 status checks and CPU/network/disk metrics at the freeze timestamp.
- Guest memory/commit, pagefile and disk latency via PerfMon or preconfigured guest metrics (EC2 default metrics do not report guest memory).
- CPU and private bytes for `amazon-ssm-agent`, `ssm-session-worker`, RDP-related processes, and any security/antivirus scanner.
- `C:\ProgramData\Amazon\SSM\Logs\amazon-ssm-agent.log`, `errors.log`, and session-worker logs if present.
- Windows System/Application event logs; unexpected restarts, resource exhaustion, disk/storage errors and driver failures.
- SSM agent version, Windows version, instance type, free RAM/disk space, copy direction, file size and whether the same transfer freezes using the official plugin.

Capture guest metrics **before** starting the copy, since logging may stop once the VM is truly hung. If both implementations reproduce the same whole-VM problem, investigate guest resource/storage/RDP/driver behavior rather than assuming another client transport rewrite will solve it.

## Source references

Comparison was against AWS Session Manager Plugin commit `6430675a44032a551be1937fb27a2d3fdfeb27f7`:

- [SSM ACK retention/retransmission and receive ordering](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/datachannel/streaming.go)
- [Multiplexed port forwarding and 1 ms send pacing](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/sessionmanagerplugin/session/portsession/muxportforwarding.go#L203-L225)
- [Wire validation and publication-control exemptions](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/message/messageparser.go)
- [Agent version thresholds](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/config/config.go) and [strict version comparison](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/version/versionvalidator.go)
