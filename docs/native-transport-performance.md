# Native SSM transport: performance follow-up

> Performance-v2 benchmark record. The current source also includes the
> [production completion](native-production-completion.md): native remote cleanup,
> SDK-resolved signing region, awaited shutdown and a full Windows-target app check.
> Historical samples/source hashes below are retained unchanged.

## Status and scope

- MFZ base: `0817c50175cc6c8714776fff9467684ecab98426`.
- Local working branch: `fix/native-ssm-performance`. No commits or GitHub pushes were made.
- Official reference cloned to `session-manager-plugin/`, revision **`6430675a44032a551be1937fb27a2d3fdfeb27f7`** (2026-10-09).
- This replaces the stability-first candidate's artificial throughput limits while preserving its fixes for lost bytes, ACK/retry correctness, handshake readiness and task ownership.
- MFZ remains native Rust. **No AWS CLI, Session Manager Plugin or Go process is required at runtime.** Go is used only by opt-in development interoperability tests.
- The reported problem is an **entire VM becoming unresponsive**, not just an RDP window. Local transport tests cannot establish that OS-level cause. **A full Windows application build, live AWS interoperability, real TLS/network throughput, RDP virtual-channel behavior and the VM-wide freeze remain unverified here.** Use a non-production instance for integration validation.

## What we learned from the official plugin

All reference links below are pinned to the cloned revision:

- [streaming/reliability](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/datachannel/streaming.go): retain original encoded messages, match selective ACKs, measure RTT, estimate retransmission timeout with alpha=1/8 and beta=1/4.
- [configuration](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/config/config.go): 1024-byte SSM stream data and 10000-message buffers.
- [multiplexed forwarding](https://github.com/aws/session-manager-plugin/blob/6430675a44032a551be1937fb27a2d3fdfeb27f7/src/sessionmanagerplugin/session/portsession/muxportforwarding.go): smux client, version-dependent keepalive, and a fixed 1 ms sleep after each 1024-byte data read.
- [vendored smux](https://github.com/aws/session-manager-plugin/tree/6430675a44032a551be1937fb27a2d3fdfeb27f7/vendor/src/github.com/xtaci/smux): session-wide 4 MiB receive credits, 32 KiB write frames, scheduled writes across streams, and FIN that permits buffered receive data to drain.

We adopt the protocol, bounded session receive storage, fair scheduling and adaptive reliability—not every implementation detail. In particular:

1. **Do not copy the fixed sleep.** TCP already provides network congestion control. ACK credit and bounded storage provide application backpressure without a wall-clock bytes-per-second cap.
2. **Do not evict unacknowledged data.** The plugin's fixed outgoing buffer evicts the oldest at capacity. MFZ blocks its producer instead.
3. **Do not treat selective ACKs as contiguous delivery.** A future message can be ACKed because the agent retained it behind a gap. Limit the sequence span, not just the remaining pending-record count.
4. **Do not let a slow stream stop the whole dispatcher after 32 frames.** Share receive byte credit across the session; global exhaustion still intentionally applies backpressure.
5. **Do not scan every pending packet on every ACK/retry tick.** Index sequence numbers and deadlines. Do not blindly retransmit on a fixed timer independent of RTT. Like the plugin's oldest-gap recovery, use bounded gap probes; an ACK-silent agent must not receive a whole-window replay storm.
6. Service publication controls remain advisory, matching the plugin's streaming handler. We do not invent a pause-only flow-control protocol.

## Implemented transport changes

| Area | Previous stability candidate | Current implementation |
|---|---|---|
| New sends | 1 ms minimum per 1 KiB frame; `send()` flushes every frame | `feed()` ready messages, one immediate `flush()` per bounded batch; **no linger or fixed sleep** |
| Send credit | 128 pending records | Default **8192-message sequence-span** window, also bounded by retained wire bytes |
| Retries | Fixed 1 s timeout, 100 ms polling | RTT EWMA; 100 ms–5 s RTO after learning; indexed exact deadlines; Karn's rule; exponential backoff capped at 10 s; at most eight active gap probes |
| Retry lifetime | 60 attempts | At most 64 retries **and** 5-minute message age; failure closes the transport instead of hanging |
| Writer scheduling | Biased control-first select | Bounded per-turn ACK/control, retry and data quotas; cooperative yield per batch |
| Control backpressure | Shared ACK/control queue could block the reader | Separate scalar input-ACK inbox; if outbound ACK/Pong wire queue is full, retain data and withhold ordinary ACK without blocking incoming credit |
| Receive smux | 32-frame per-stream awaited queue | Shared **4 MiB session byte credit**, with metadata allowance and credit retained until a whole allocation is consumed |
| Send smux | Shared 64-frame FIFO | **Two-frame per-stream** bounded queues; round-robin at a maximum 32 KiB write frame |
| FIN/drop | Normal FIN after data; Drop could lose FIN on a full queue | Ordered FIN represented separately from data capacity; bounded active/closing stream entries; Drop does not overtake queued data |
| Allocations | Vector copies and formatted UUID strings on the hot path | Immutable `Bytes` wire buffers; cheap exact retry clones; slice smux output into SSM packets; typed UUID and message-type ACK matching |
| Retained receives | Logical length could miss large backing allocations after a Bytes upgrade | Normalize retained allocations: a tiny shared/tail slice cannot pin a huge buffer and bypass byte-credit accounting |
| WebSocket | 0.24, mostly default limits | 0.30 Bytes-backed messages, explicit message/frame/write-buffer limits, TCP_NODELAY |
| TCP copies | Default Tokio buffers | 32 KiB each direction; standard error/half-close behavior |
| Structure | One large SSM module | Separate wire codec, flow state, writer, allocation-retention helper, orchestration and regressions |

### Bounded defaults

These are **data-credit/storage limits, not an exact process-RSS limit**. SDK, TLS, OS socket buffers, allocator overhead and the RDP client add memory outside them.

- Smux: at most 64 active/closing streams; two queued send frames per stream (32 KiB data maximum), plus ordered FIN state; one coalesced keepalive.
- Smux receive: 4 MiB charged bytes including 128 bytes allowance per chunk. A partially read chunk keeps its entire credit until consumed; closed streams return it.
- SSM outgoing: 8192 outstanding sequence span and at most 8192 × (120 + 1024) = **8.9375 MiB** retained encoded wire bytes, plus indexed metadata. Selective future ACKs release buffers, but cannot move new sends arbitrarily far past an older gap.
- SSM producer: 256 packets; DATA stays at **1024 bytes or less**. A batch never combines several SSM messages into one WebSocket message.
- SSM reassembly: 8192 records / 4 MiB charged wire-size bytes; one record and a full maximum-size frame reserved for the missing expected sequence. 64 dispatcher slots; output payload at most 64 KiB.
- Ordinary ACK/Pong/control: 512 commands. Scalar input ACKs: window size + 512 retry slack. Exhausted ordinary ACK capacity never means acknowledged bytes were discarded.
- Writer batch: at most 64 new DATA packets by default, with bounded control/retry quotas. Idle writes flush immediately; buffers do not wait to fill.
- Connect/handshake: 30 s deadlines; stalled socket batch: 15 s deadline. Initial RTO is 1 s before a trustworthy RTT sample; no samples from retransmitted packets.

No transport can eliminate TCP ordering, the shared SSM sequence, guest CPU/disk limits or contention inside a single RDP connection. Fairness is between smux streams; it cannot reorder RDP virtual channels within one TCP stream. A slow stream can still exhaust the session-wide budget, at which point global backpressure is correct.

### Tuning and observability

The existing CLI uses validated defaults. The advanced Rust entry point `start_port_forward_tunnel_with_options` accepts `TunnelOptions { window_messages, write_batch_messages }`; invalid zero/excessive values fail before starting a task. Window range is 1..=65536, batch range 1..=256. Do not increase SSM DATA size.

Choose a window against bandwidth-delay product and the guest's capacity: 8192 × 1 KiB / 250 ms is a theoretical 32 MiB/s credit ceiling, **not a measured AWS rate**. Increasing it costs memory and can increase bursts toward a CPU-limited agent. Start smaller if testing shows guest pressure. Avoid layering another TCP-style congestion controller over TCP or reintroducing sleep-per-packet.

At debug level the writer reports frames, flushes, retries, peak retained messages/bytes and final RTO on exit. The live session token JSON is never logged. Use external guest/client CPU, memory, network and disk monitoring alongside these counters.

## Measured local performance

The reproducible opt-in benchmark compiles the **real production writer**, transports masked WebSocket messages over a bounded in-memory duplex, validates SSM frames and supplies delayed selective ACKs. It counts underlying transport `poll_write` calls. No TLS, real network, AWS gateway, SSM agent, Windows or RDP is in this benchmark.

**The baseline is our previously delivered stability patch—not the original unbounded MFZ implementation and not the official Go plugin.** A 4 MiB run sends 4096 × 1024-byte DATA frames. Timing includes the final ACK drain; small transfers are therefore RTT-dependent. Measurements vary by host. Raw samples and environment are in [the benchmark JSON](benchmarks/native-writer-2026-10-15.json).

The original baseline measured approximately 0.458 / 0.455 / 0.444 MiB/s at 0 / 50 / 250 ms RTT, and 0.244 MiB/s at 500 ms RTT. It made **4096 transport writes** for each 4 MiB run. The optimized writer normally uses **64 writes** for the same payload: **64× fewer calls**, with one separate WebSocket/SSM message per packet.

| Simulated ACK RTT | Previous MFZ (MiB/s) | Optimized MFZ median (MiB/s) | Speedup |
|---|---:|---:|---:|
| 0 ms | 0.458 | 256.69 | 560.0× |
| 50 ms | 0.455 | 60.82 | 133.6× |
| 250 ms | 0.444 | 15.04 | 33.9× |
| 500 ms | 0.244 | 7.76 | 31.8× |

Candidate values are medians of **three final-source runs** per RTT; the baseline
has one sample per RTT. Every 4 MiB run reports zero duplicates. These figures
demonstrate removal of client-side artificial limits; they are **not an AWS or
RDP throughput guarantee**.

A separate final-source **16 MiB** candidate run exceeds the default 8 MiB window
and exercises multiple credit rounds: 280.0 / 115.4 / 28.8 / 15.4 MiB/s at 0 /
50 / 250 / 500 ms RTT, with 256 writes and no duplicates. Do not compare its
larger-transfer rate directly with the 4 MiB baseline rate.

## Verification

The lightweight harness includes the actual production transport and error modules, excluding Windows orchestration and the large EC2 SDK:

```bash
cargo test --manifest-path tools/transport-tests/Cargo.toml --locked
cargo clippy --manifest-path tools/transport-tests/Cargo.toml --locked --all-targets -- -D warnings
rustup target add x86_64-pc-windows-gnu
cargo check --manifest-path tools/transport-tests/Cargo.toml --locked --target x86_64-pc-windows-gnu --all-targets
```

Local Linux x86_64 / Rust 1.99.0 results:

- **50 regression tests passed**, with benchmark and two Go tests intentionally opt-in.
- **Two real Go smux interoperability tests passed separately**: two concurrent 8 MiB upload/echo streams with byte equality and FIN drain, and a 2 MiB stalled first stream alongside a completed 64 KiB second stream. This is smux over local TCP, not live SSM interoperability.
- Strict transport Clippy and Windows-GNU **transport/test-target compilation** passed. Windows execution/full app compilation were not performed.
- Regressions include sequence-span protection after future ACKs, adaptive RTT/Karn/backoff, deadline-index cleanup, no batching linger, fair control bursts, idle final ACK cleanup, allocation amplification, queue saturation, cancellation and corrupted frames.
- The signed public-handle mock still validates dummy SigV4 headers, blocked **64 MiB** upload backpressure, socket/task stop and local port reuse. Two 1 MiB native TCP/WebSocket round trips survive a deliberately lost SSM packet.

[Harness README](../tools/transport-tests/README.md) contains Go fixture, benchmark, baseline-reproduction and Windows commands. The vendored development peer is unmodified MIT smux code from the pinned AWS tree, with its license and SHA-256 manifest included. New CI configuration runs transport regressions/lint and these interoperability tests on Linux and Windows; that workflow has not been run on GitHub from this workspace.

## Apply, build and validate on Windows/EC2

The new `mfz-native-performance.zip` is a source-only archive. The full `mfz-native-performance.patch` applies to clean base `0817c50` and includes the prior correctness fixes; **do not also apply the old stability patch**. The incremental `mfz-native-performance-from-stability.patch` is for the previously delivered stability source tree instead.

```bash
git switch -c fix/native-ssm-performance
git apply --check /path/to/mfz-native-performance.patch
git apply /path/to/mfz-native-performance.patch
```

Build the Windows application on Windows with current stable Rust:

```powershell
cargo build --release --locked
.\target\release\mfz.exe --help
```

The harness needs the locked AWS dependencies' Rust 1.91.1 minimum or newer; this work used 1.99.0. Existing application configuration and clipboard/drive-sharing settings remain unchanged.

Use the [Windows/EC2 integration checklist and independent VM diagnostics](native-transfer-fix.md) from the original investigation. Test both clipboard and redirected-drive transfers, hash files in both directions, keep an independent instance-health/console path, monitor SSM agent and RDP/client CPU/RAM/disk, and repeat disconnect/reconnect while checking for stale remote sessions. The faster sender can reveal real guest resource limits rather than masking them with a throttle.

**Known scope limits of performance-v2:** KMS sessions fail explicitly; automatic ResumeSession/reconnect and explicit TerminateSession API cleanup were not implemented in that candidate. The current production-completion source adds native SDK termination (see the linked report). Local cancellation and ordered stream FIN do not guarantee remote cleanup after an abrupt transport abort; service/agent/session timeout behavior still matters. The entire-VM freeze is not proven resolved. Do not deploy as a proven OS-level fix without live validation.
