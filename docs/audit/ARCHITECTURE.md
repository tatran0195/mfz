# Rust CLI Architecture Audit

**Audit date:** 2026-10-15. **Scope:** the root Rust package, its binary/library entry points, feature modules, tests, build configuration, and the existing offline application-test harness. This is an architecture assessment, not a second security audit; security and dependency evidence remains in [REPORT.md](REPORT.md), [ISSUES.md](ISSUES.md), [COVERAGE.md](COVERAGE.md), and [DEPENDENCIES.md](DEPENDENCIES.md).

## Assessment

MFZ is a Windows-oriented CLI with a real, nontrivial application core: configuration and credential policy, AWS identity/MFA and EC2 selection, an owned native SSM/WebSocket/smux transport, RDP lifecycle, shell integration, and QR/enrollment workflows. The existing single-crate design is appropriate. The library already has useful domain/OS boundaries, the transport has been decomposed into protocol, lifecycle, flow, writer, and buffer modules, and tests cover both library contracts and actual offline CLI behavior. A workspace or additional internal crates would add build/API complexity without an independently shipped component or meaningful dependency-isolation benefit.

The main architectural weakness is the binary composition boundary: `src/main.rs` is 1,034 lines and contains process initialization, top-level error/exit behavior, dispatch, and every command workflow. It has become the command application layer as well as the process entry point. This makes a command change compete in the same file with unrelated workflows, and hides the distinction between process policy and feature-specific presentation/orchestration.

The second hotspot is `src/config.rs` (941 lines). It combines the environment model and validation, INI value parsing/serialization, source and profile resolution, load/list/export-facing conversion, locked mutations, atomic persistence, and secure migration. Some coupling here is intentional: secret migration and config persistence must preserve a security-sensitive transaction boundary. The file is nevertheless large enough that model invariants and persistence/source behavior are difficult to navigate independently.

Other large files are not automatically god modules. `src/ssm_tunnel.rs` has dedicated protocol, lifecycle, flow, writer, and buffer peers, while `src/smux.rs`, `src/rdp.rs`, `src/shell.rs`, and `src/enrollment.rs` each implement cohesive domain lifecycles with adjacent focused tests. `src/cli.rs` is a cohesive Clap schema, not a command implementation module. Keep these boundaries unless a specific behavior demonstrates a better split.

## Findings

| Priority | Location | Finding and impact | Correction and risk |
| --- | --- | --- | --- |
| High | `src/main.rs` | Process startup, dispatch, configuration, export, launch, host management, shell registration, and QR workflows share one module. This is the clearest god-file boundary and increases unrelated merge/review surface. | Keep startup, top-level dispatch, and exit/error policy in `main.rs`; move workflows to private binary-only command modules grouped by actual command responsibility. Preserve Clap types, exit behavior, and user-facing output. Main risks are visibility/import mistakes and accidental CLI output/ordering changes; guard with the full Rust tests and the actual-binary offline checks. |
| Medium | `src/config.rs` | Model/invariants, parsing, source selection, persistence, and secret migration are interleaved. The size obscures distinct read/model/write responsibilities, although mutation and secure migration must remain coordinated. | Extract model/value handling and file-source/persistence implementation behind `ConfigLoader`, keeping one public `mfz::config` API and the locked migration boundary. Move only after identifying exact private cross-dependencies; do not split by line count or create a generic repository framework. Regression risk is config precedence, quoted-value round trips, source provenance, atomic replacement, and keyring authority. |
| Medium | `src/main.rs` / `src/cli.rs` / feature modules | CLI definitions are cleanly separate, but command dispatch currently reaches across all features from the entry point. The result is an unclear ownership boundary, not evidence that all workflows need identical handler abstractions. | Make the binary command layer the only place that maps Clap variants to workflow modules. Simple list/hosts/shell commands can remain direct; launch, config mutation, export, and QR/enrollment stay in their cohesive feature modules. |
| Low | `src/lib.rs` | The package exposes many modules publicly even though MFZ is principally an executable. However, integration tests and the portable transport/application harness consume the library; reducing visibility would be a compatibility change without a demonstrated benefit. | Preserve current public exports during this refactor. Revisit API narrowing only with a documented consumer/migration inventory. |
| Not a finding | `Cargo.toml`, transport modules, dependencies | The package is one binary/library crate with feature-oriented modules and explicit dependency declarations. Existing remediation has already assessed and changed dependency features/versions; no architecture evidence supports another crate or dependency change here. | Keep one crate and avoid dependency churn. The dependency inventory and residual advisory reachability remain in `DEPENDENCIES.md`. |

## Target Structure (Implemented)

```text
src/
├── main.rs                 # CLI entrypoint, console encoding, setup_logging, dispatch
├── lib.rs                  # Library crate facade, domain modules, backward-compatible re-exports
├── cli.rs                  # Clap CLI schema and argument definitions
├── error.rs                # Typed AppError and contractual CLI exit codes
├── ui.rs                   # Terminal formatting, banners, spinners, colorized output
├── validation.rs           # Line/path sanitization, safe profile filenames, CRT quoting
│
├── auth/                   # Authentication & Credentials Domain
│   ├── mod.rs              # Facade exporting CredentialStore, MfaSessionManager, etc.
│   ├── credentials.rs      # Windows Keyring/Credential Manager secure storage
│   ├── enrollment.rs       # IAM virtual-MFA device creation, activation, rollback
│   ├── mfa.rs              # STS session token negotiation, TOTP generation
│   ├── otp.rs              # RFC 4648 Base-32 decoding & normalization
│   └── qr.rs               # TOTP provisioning QR generation, PNG export, image decoding
│
├── aws/                    # AWS Cloud Infrastructure Domain
│   ├── mod.rs              # Facade exporting instance resolution
│   ├── ec2.rs              # AWS EC2 SDK client, tag-based instance query
│   └── instance_selection.rs # Pagination and unique instance selection rules
│
├── commands/               # CLI Command Execution & Workflow Layer
│   ├── mod.rs              # Command submodules
│   ├── config.rs           # `config` subcommands (list, set, get, delete, keyring migration)
│   ├── export.rs           # `export` profile workflow (desktop shortcut / standalone INI)
│   ├── hosts.rs            # `hosts` management (add, apply elevated, custom)
│   ├── launch.rs           # `launch` orchestration (MFA → SSM tunnel → RDP launcher)
│   ├── list.rs             # `list` configured environments
│   ├── qr.rs               # `qr` subcommands (decode, generate)
│   └── shell.rs            # `register`/`unregister` Windows file associations
│
├── config/                 # Configuration & Profile Domain
│   ├── mod.rs              # Facade, INI serialization/deserialization, mutation lock
│   ├── loader.rs           # ConfigLoader: profile resolution, multi-env INI loading, migrations
│   └── model.rs            # EnvironmentConfig, AuthType, and domain invariants
│
├── rdp/                    # Remote Desktop Domain
│   ├── mod.rs              # RDP config generator, OptimizationSettings, LaunchOptions
│   └── platform.rs         # Windows mstsc.exe launcher & process spawning
│
├── sys/                    # Operating System & Platform Domain
│   ├── mod.rs              # Facade for hosts, shell, storage, windows
│   ├── hosts.rs            # Hosts file parsing, elevated UAC helper, hosts modifications
│   ├── shell.rs            # Windows Registry HKCU file associations, Known Folders
│   ├── storage/            # Atomic filesystem operations, mutation locks, DACL security
│   │   ├── mod.rs          # Cross-platform atomic file persistence and locks
│   │   └── windows.rs      # Windows-specific security descriptors, DACL inheritance
│   └── windows.rs          # Windows Console & Known Folder directory resolution
│
└── transport/              # Network Transport & Multiplexing Domain
    ├── mod.rs              # Facade for smux, ssm, buffer
    ├── buffer.rs           # Buffer retention & memory credit management
    ├── smux/               # SMUX v1 multiplexing protocol implementation & tests
    │   ├── mod.rs
    │   └── tests.rs
    └── ssm/                # Native AWS SSM WebSocket port-forwarding protocol stack
        ├── mod.rs          # SsmSession, SsmTunnelHandle, tunnel starters
        ├── client.rs       # SSM session retry orchestrator and connection manager
        ├── flow.rs         # Sliding-window flow control, sequence gaps, reassembly
        ├── lifecycle.rs    # SSM session token exchange and port readiness checks
        ├── protocol.rs     # SSM binary wire frame parser & serializer
        ├── tunnel.rs       # WebSocket listener & TCP relay tunnel loop
        ├── writer.rs       # Non-blocking batch WebSocket frame writer
        └── tests/          # SSM protocol, flow, writer, and SDK peer tests
            ├── mod.rs
            ├── peer.rs
            ├── flow_tests.rs
            ├── sdk_tests.rs
            ├── tunnel_tests.rs
            ├── writer_bench.rs
            └── writer_tests.rs
```

## Dependency Rules

- **Strict Dependency Direction:** 
  - `commands` depends on domain modules (`auth`, `aws`, `config`, `rdp`, `sys`, `transport`), `cli`, `ui`, `error`.
  - Domain modules (`auth`, `aws`, `config`, `rdp`, `sys`, `transport`) NEVER depend on `commands`.
  - `sys` and `transport` have zero dependencies on upper business domains.
  - `main.rs` contains only process entry, console initialization, logging setup, and delegating dispatch to `commands::*`.
- **Zero Global State:** All services and session managers are instantiated with explicit parameters.
- **Contractual Error Boundaries:** `AppError` provides deterministic exit codes (1 through 10) enforced by integration tests.
- **Backward Compatibility:** All legacy paths (`mfz::credentials`, `mfz::mfa`, `mfz::ssm`, `mfz::storage`, etc.) are re-exported in `src/lib.rs` to ensure existing external tests and consumers compile without breakage.

## Migration Order and Checks

1. Record this assessment and target rules before code movement.
2. Extract command handlers into private binary modules in small groups, keeping `main` dispatch and the actual Clap variants stable. After each extraction, run `cargo check --bin mfz` and the narrowest relevant tests; after the command layer is complete, run the existing actual-binary offline CLI suite.
3. Split `config.rs` only at proven seams, beginning with environment model/value logic while keeping the public facade and transaction boundary unchanged. Run config and audit regression tests after each slice.
4. Run `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, `cargo test --workspace`, and strict Clippy where the installed targets support them. Windows API execution and live AWS/RDP acceptance remain separate gates and cannot be inferred from cross-compilation.

## Compatibility and Limits

No documented command, option, default, config precedence, output stream, exit code, or integration-test import is intended to change. Existing security remediation findings are outside the scope of this structural pass and must not regress. The workspace currently presents all repository files as staged additions; do not stage, reset, or commit as part of this work. This audit intentionally identifies where architectural work is justified; it does not claim that a future comprehensive audit can prove the absence of every defect.
