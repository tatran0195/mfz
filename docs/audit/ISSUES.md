# Repository remediation issue inventory

Baseline: `668854b1c20b58e175046482481e44b326c96883`, 2026-10-15.
Status is deliberately not inferred from a proposed fix. `Confirmed (trace)` means
an execution path establishes the defect; `verified` additionally requires an
executed regression/check. No critical remote exploit is claimed from a pattern
alone. Likelihood is conditional on the stated scenario, not a population estimate.

## Current verified source checkpoint

Baseline behavior/evidence below refers to the **preserved delivered-v3 source**,
not a claim that it remains unfixed. Main workflows/callers were migrated.
Executed final local checks: **146 tests passed on three repeat runs**, **25 actual
application offline CLI checks**, **two actual Go-smux cases**, complete application
build, full-root Linux AND Windows-GNU all-target type/strict lint, expanded-harness
strict lint and stable fmt. See `VERIFICATION.md` for commands, logs and limitations.
Windows type-check is NOT Windows API execution; live Windows/AWS/guest and remote
CI gates remain open. No every-conceivable-defect or current-exfiltration claim.

**Important:** A26 records two potentially genuine committed MFA seeds (validity
unknown), also present in prior preserved ZIPs. No values appear in reports. A28
records a regression caught during intermediate remediation, not falsely attributed
to upstream's original direct writes.

## High priority

### MFZ-A01 — Secret removal does not remove persisted secrets
- Category/location: security, persistence; `src/config.rs::ConfigLoader::save`, `src/main.rs::cmd_config(Secure)`.
- Evidence/current behavior: Secure takes secret fields out of the model, then save only inserts keys returned by `to_map`; old INI keys are never removed. Reloading restores the plaintext secrets while the CLI prints “secrets removed.”
- Root cause/impact: patch semantics are used for a complete-model save. The same stale-field behavior prevents clearing other settings. A user believing MFA/AWS/RDP secrets were secured can retain/distribute plaintext.
- Severity high; likelihood high when Secure is used; confidence high. Affects config edits, credential migration and portable exports.
- Remediation: replace known section fields coherently, verify secure-store writes before removing plaintext, atomically persist under a mutation lock.
- Verification: `aud_001_removing_secret_fields_actually_removes_persisted_keys` plus migration/save-failure regressions.
- Status: Implemented and locally verified: full-field removal, secure readback/failure and locked persistence tests pass. Actual Windows secure-store/filesystem execution remains an external gate.

### MFZ-A02 — Ordinary exports silently copy plaintext secrets
- Category/location: security; `src/shell.rs::export_mfz_file`, `src/main.rs::cmd_export`, `src/cli.rs::ExportArgs`.
- Evidence: default standalone export serializes `EnvironmentConfig::to_map`, including AWS secret, TOTP seed and RDP password, without a warning/opt-in. Exported desktop/shared files can contain all authentication factors.
- Root cause: persistence and distributable export share an unfiltered secret-bearing representation.
- Severity high; likelihood medium (requires secrets in config/export); confidence high. Also affects export-name/path validation and output overwrites.
- Remediation: secret-free export by default, explicit secret opt-in/warning, validated names and no-clobber/explicit replacement policy.
- Verification: `aud_002`, `aud_003`, `aud_011`; restricted-file and round-trip checks.
- Status: Implemented and locally verified by helper and actual-CLI secret/no-clobber/explicit-policy checks; Windows output ACL runtime behavior remains an external gate.

### MFZ-A03 — RDP destination is not constrained to the authenticated tunnel
- Category/location: security, integration; `src/main.rs::cmd_launch`, `src/hosts.rs::{has_host,ensure_host}`, `src/rdp.rs::launch`.
- Evidence: any hosts-file alias is accepted irrespective of IP; hosts read/add failures are converted to false/ignored; `mstsc` receives the configurable hostname anyway. DNS or an incorrect existing mapping can direct RDP/cached credentials outside the native localhost tunnel.
- Root cause: alias existence is confused with the network endpoint invariant.
- Severity high; likelihood medium with wrong/untrusted mapping/profile; confidence high. Affects cached credentials, profile trust and hosts handling.
- Remediation: connect tunneled RDP to a numeric loopback endpoint, separate descriptive aliases/explicit hosts management, reject unsafe profile fields before side effects.
- Verification: generated-profile/launch-target regressions and Windows-target integration check; actual MSTSC requires Windows.
- Status: Implemented numeric owned-loopback policy through main/public launch; portable target tests and full Windows integration type/lint pass. Actual mstsc/server identity remains an external gate.

### MFZ-A04 — Unvalidated strings cross configuration/hosts/RDP file boundaries
- Category/location: injection, integrity; `EnvironmentConfig`, `HostsManager::add_host`, `build_rdp_file_content`, `export_mfz_file`.
- Evidence: public functions interpolate hostname/user/drive/environment values into line-oriented files or paths without CR/LF/control/path checks. `hosts custom` can append additional host records via one hostname; environment names can escape an export directory.
- Root cause: CLI argument separation is mistaken for validation of downstream file grammars.
- Severity high for elevated hosts manipulation/misdirected credentials, medium for local profile corruption; likelihood medium; confidence high. All entry points, not just CLI parsing, need checks.
- Remediation: validate at the relevant public boundaries; preserve literal passwords using unambiguous serialization, not destructive sanitizing.
- Verification: `aud_003`, `aud_015`, RDP control-character and invalid-save regressions.
- Status: Implemented at config/hosts/export/RDP public boundaries; injection/quoting/serialization regressions pass. No destructive sanitizing of valid password bytes.

### MFZ-A05 — Security-sensitive Windows executables are found by ambient search
- Category/location: local execution/trust; `src/rdp.rs::launch` calls `Command::new("mstsc")`.
- Evidence: Windows executable search can select an executable in an untrusted working directory/PATH. Double-click profile operation can inherit unrelated current directories.
- Root cause: a Windows system component is invoked as an arbitrary search-path command.
- Severity high under planted-executable scenario; likelihood low/medium; confidence high on search behavior, Windows execution not available.
- Remediation: obtain the system directory through the Windows API and invoke the absolute executable; unsupported platforms must return an error instead of shell fallbacks.
- Verification: path construction unit checks, Windows cross-check and live Windows gate.
- Status: Implemented absolute system executable and unsupported-platform error. Complete Windows all-target type/lint pass; native mstsc execution not performed here.

### MFZ-A06 — Temporary RDP credentials overwrite/delete existing credentials
- Category/location: security, data integrity; `src/rdp.rs::launch`.
- Evidence: three target records, including a broad hostname without port, are written with per-user local-machine persistence (not a shared machine-wide vault); return values are ignored; all are deleted unconditionally afterwards. Existing user entries can be destroyed, failures are invisible and interrupted sessions can leave persistent credentials.
- Root cause: credential updates have no ownership/restoration/error model or per-target coordination.
- Severity high; likelihood medium when credentials already exist/concurrent sessions fail; confidence high. Windows-only behavior requires API execution to fully verify.
- Remediation: narrowly scoped targets, session persistence, owned guarded writes/restoration and explicit errors; no command-line password fallbacks.
- Verification: mockable lifecycle invariants and Windows-target type-check; native Credential Manager execution remains a live gate.
- Status: Implemented one exact-port session credential, refused overwrite, fresh ownership/readback checks and checked cleanup; no subprocess-argument password. Complete Windows type/lint pass, native credential/mstsc behavior remains unexecuted.

### MFZ-A07 — Region selection is inconsistent across authentication/discovery/session creation
- Category/location: correctness; `src/main.rs::cmd_launch`, `src/aws.rs::resolve_instance_by_tag`, `src/ssm.rs::SsmSession::start`.
- Evidence: launch computes CLI/config/fallback region for MFA, but passes only `args.region` to EC2 and SSM. Clients can therefore use the default SDK region rather than the environment's configured region. V3's correctly SDK-resolved WebSocket signing cannot fix choosing the wrong control-plane region.
- Root cause: duplicated selection code and loss of the resolved value at call boundaries.
- Severity high; likelihood high with per-environment regions and no override; confidence high.
- Remediation: propagate one explicit override consistently and otherwise honor the selected SDK profile/provider chain across services.
- Verification: actual SDK loopback signature/endpoint tests and launch-resolution regressions.
- Status: Implemented consistent override and cached selected-profile provider/region; actual SDK production-start/profile fixture and both-platform integration checks pass.

### MFZ-A08 — Long-lived credentials are passed to SDK clients with an empty session token
- Category/location: correctness/integration; `src/mfa.rs::load_from_profile`, `src/aws.rs`, `src/ssm.rs`.
- Evidence: no-MFA direct credentials and static profiles model absence as `""`; EC2/SSM construct SDK credentials with `Some(creds.session_token)`. The token header should be absent, not an explicitly empty security token.
- Root cause: ad-hoc credential conversion is duplicated rather than preserving AWS token optionality.
- Severity high for the advertised standalone/no-MFA workflow; likelihood high for that mode; confidence high on invalid representation, service response requires live AWS.
- Remediation: one credential conversion that omits empty tokens, used by every SDK/signer consumer.
- Verification: local real-SDK HTTP and signer tests assert absence/presence of token headers.
- Status: Implemented optional token model and typed SDK credentials through native signer; actual HTTP/signature presence/absence tests pass.

### MFZ-A09 — Partial MFA configuration silently selects a non-MFA flow
- Category/location: authentication/reliability; `src/mfa.rs::MfaSessionManager::get_credentials`.
- Evidence: MFA is used only if both serial and secret are Some; serial-only/secret-only managers fall back to profile/direct credentials. Main normally resolves a missing secret first, but this public service entry point does not uphold the invariant.
- Root cause: missing MFA input is treated as a request for another authentication mode.
- Severity high as a fail-open API contract; actual AWS IAM policy may independently reject non-MFA requests (no IAM bypass is claimed); likelihood medium; confidence high.
- Remediation: reject incomplete/empty MFA configuration before credentials/provider calls.
- Verification: public-manager tests proving no fallback/network request.
- Status: Implemented fail-closed validation before provider access; no-fallback/no-network manager tests pass.

### MFZ-A10 — Tag discovery silently selects an arbitrary instance and ignores later pages
- Category/location: target integrity; `src/aws.rs::resolve_instance_by_tag`, tag parsing in `cmd_launch`.
- Evidence: first response/first instance is returned, no paginator or uniqueness check; CLI splits on every `=` although tag values can contain `=`. Shared Name tags can select the wrong (including production) VM.
- Root cause: an API collection is treated as a unique resource identifier.
- Severity high for target integrity; likelihood medium; confidence high.
- Remediation: split once, validate nonempty key/value, paginate and require exactly one distinct matching running instance; cap pathological pagination.
- Verification: deterministic selector/pagination contract tests and complete SDK type-check; live account IAM remains external.
- Status: Implemented split-once tag parsing and bounded unique paginated selection; actual selector regressions and complete EC2 SDK type/lint pass. Live account enumeration is an external gate.

### MFZ-A11 — INI defaults truncate passwords and drive lists
- Category/location: data correctness; every `Ini::new` use in `src/config.rs`.
- Evidence: configparser's default inline comment symbols are `#` and `;`. A password containing either or a semicolon-separated RDP drive list is truncated during load. Save/load is not a round trip.
- Root cause: generic INI defaults are incompatible with this application's value grammar.
- Severity high for silently changed credentials; likelihood medium/high; confidence high.
- Remediation: full-line comments only, shared parser settings, explicit quoting/escaping for values that cannot round-trip literally, retain secret whitespace where represented explicitly.
- Verification: `aud_006`, punctuation/quoted-value/reload regressions.
- Status: Implemented full-line comments and explicit quoted values; punctuation/control/whitespace and explicit-empty-password round trips pass.

### MFZ-A12 — Vulnerable/deprecated transitive stacks are enabled unnecessarily
- Category/location: dependencies; root/harness Cargo manifests and locks.
- Evidence: the saved OSV batch scan matched 13 package versions. Advisory documents/ranges are preserved under `docs/audit/advisories/`; aliases are not counted as separate vulnerabilities. SDK default `rustls` additionally enables the legacy Hyper 0.14/rustls 0.21 stack even though the modern default HTTPS client is enabled. rxing's image helper feature pulls imageproc/font code although MFZ already supplies luminance data directly.
- Root cause: broad/default features plus old locked transitive versions. Not every matched path is exploitable: imageproc transforms, CRL parsing, smithy generated servers, aarch64 cmov and Debug paths require separate reachability assessment. Unmaintained notices are not themselves exploits.
- Severity high for reachable TLS issues; variable for others; likelihood/reachability under review; confidence high on package matches.
- Remediation: remove unused feature stacks/direct crates; update affected compatible transitive versions rather than suppress advisories; re-scan and document residual reachability.
- Verification: dependency trees, compile/tests and final OSV query with saved evidence.
- Status: Known advisory versions patched/unused stacks removed. Exact OSV scan matches only fxhash/paste maintenance notices; effective-tree reachability and deliberately retained residuals documented in DEPENDENCIES.md. No suppression or permanent zero-risk claim.

## Medium priority

### MFZ-A13 — Malformed profiles can silently select another configuration
- Location: `ConfigLoader::load_from_file_or_name`.
- Evidence/root cause: pointer keys use starts-with matching; relative pointer configs use process CWD; parse/validation errors are ignored and fall through to an existing environment named after the file; selection of the first valid section depends on HashMap iteration. Empty-file stem shorthand is intentional, malformed-file fallback is not.
- Impact: wrong environment launch, confusing diagnostics. Medium/high target integrity; likelihood medium; confidence high. Also platform case sensitivity and explicit-path resolution need consistency.
- Remediation: exact pointer grammar, pointer-relative paths, deterministic single-profile selection, empty-only fallback and source metadata for launch trust/keyring decisions.
- Verification: `aud_007`, `aud_008`, `aud_009`, ambiguous/missing profiles and trust-boundary tests.
- Additional reproduced evidence: the actual earlier application binary silently selected USERPROFILE/.mfz/mfz.ini for an explicit `--config mfz.ini`; the new subprocess case failed before the fix. CLI config fields now preserve omission vs explicit path, and programmatic constructors use exact paths.
- Status: Implemented deterministic/exact parser, pointer-relative provenance, explicit-vs-omitted config origin and two-stage profile authority. Original regressions and independently reproduced actual-CLI explicit-path regression pass.

### MFZ-A14 — Invalid configurations are accepted or silently hidden
- Location: `EnvironmentConfig::from_section`, `ConfigLoader::{get_all,save}`, CLI add/edit.
- Evidence/root cause: empty option strings count as present; zero ports parse successfully; malformed booleans become None; unknown optimization maps to None; get_all drops every invalid section; programmatic saves bypass validation. Config/hosts/export can claim success on a partial set.
- Impact: ignored controls, later failure, incorrect target/default behavior. Severity medium; likelihood high for typos; confidence high.
- Remediation: coherent model validation reused at load/save and public output boundaries; actionable errors for every invalid section; sorted deterministic listings.
- Verification: `aud_004`, `aud_005`, `aud_010`, blank/invalid inputs and programmatic-save tests.
- Status: Implemented coherent model/output/CLI validation, strict invalid-section errors and accurate non-MFA provider labeling; regressions pass.

### MFZ-A15 — Config mutations can truncate files and lose concurrent updates
- Location: `ConfigLoader::{save,delete}`, `cmd_config(Edit,Secure)`.
- Evidence/root cause: direct fs::write truncates the destination; read/modify/write has no lock; edit reads outside save so even a save lock alone would lose edits. Non-UTF8 paths silently become an empty filename through unwrap_or_default.
- Impact: partial config loss on failure/crash, unrelated edits overwritten. Severity medium/high; likelihood medium; confidence high.
- Remediation: same-directory private temporary file + atomic replacement, cross-process mutation lock, closure-based locked edit/migration; operate on Paths without lossy conversion.
- Verification: read-only/write-failure/no-truncation, concurrent edits, Unicode/path and full replacement regressions.
- Status: Implemented stable read-through-update locks/atomic writes and migrated all callers; local read-only/no-loss/concurrent-lock tests pass. Complete Windows DACL/recovery implementation/test targets type/lint pass; actual Windows filesystem behavior remains an external gate.

### MFZ-A16 — Credential deletion errors are discarded; unsupported backends can lie
- Location: `CredentialStore::delete_secrets`, `cmd_config(Remove)`, non-Windows credential backend.
- Evidence: each delete result and then the entire operation result are ignored. keyring has a mock default on unsupported target configurations; successful volatile writes must not be represented as durable secure migration.
- Impact: stale secrets or credential loss with a false success message. Severity medium; likelihood medium; confidence high on swallowed errors, backend test pending.
- Remediation: distinguish NoEntry from real errors, attempt all deletions/report failures; fail unsupported writes explicitly; isolated backend tests and opt-in real Windows test.
- Status: Implemented unsupported-mock errors, normalized identities, fresh readback and checked deletion. Portable tests pass; explicit disposable real-Windows round trip is provided but not executed here.

### MFZ-A17 — QR URI processing is not a URI contract
- Location: `qr::{generate,extract_secret_from_uri,decode,save_png}`.
- Evidence: labels/issuer are interpolated unescaped into path/query; regex accepts secret-looking substrings of arbitrary URIs and does not validate type/duplicates/Base32. Image decoding/output have no app-level size/privacy/no-clobber policy.
- Impact: wrong secret/issuer/OTP parameters, malformed QR, local resource exhaustion or accidental secret-file overwrite. Severity medium; likelihood medium; confidence high on URI bugs; size risk not benchmarked.
- Remediation: existing URL/TOTP primitives, consistent Base32 parser, explicit supported algorithm contract, bounded image decoding and safe output.
- Verification: independent TOTP parser, special-character round trips, malformed URI/large image cases.
- Status: Implemented URL/Base32/image/output contracts; actual QR PNG/independent TOTP, malformed input, bounds and no-clobber regressions pass.

### MFZ-A18 — IAM enrollment leaves orphan devices and conceals persistence failure
- Location: `qr::register`, `prompt_totp`.
- Evidence: after CreateVirtualMFADevice, every error/cancellation exits without deleting an unused device. Existing-config load errors become a fabricated REPLACE_ME profile; saving errors are printed then overall success is returned. EOF is retried forever by prompt_totp.
- Impact: unrecoverable enrollment retry/name conflicts, missing saved MFA seed with false success, tight EOF loop. Severity medium/high; likelihood medium; confidence high.
- Remediation: validate save target first, owned bounded cleanup until activation, preserve original failures, return save errors and stop at EOF.
- Verification: real-SDK loopback failure/cancellation fixtures where practical, input EOF unit test, Windows/AWS live gate.
- Status: Implemented owned pending cleanup, truthful activation/save state, existing-save preflight and EOF/cancel errors. Real SDK local XML lifecycle and actual closed-input subprocess tests pass; live IAM remains external.

### MFZ-A19 — RDP/drive process and mapping teardown lacks ownership checks
- Location: `rdp::{launch,FolderDriveGuard,is_drive_letter_in_use,resolve_drives_to_redirect}`.
- Evidence: nonzero mstsc exit still returns success; credential/DefineDosDevice deletion results are ignored; mapping removal has no exact target; an ordinary file passes folder exists(); non-ASCII char is narrowed before validation; comma drive lists documented in CLI are not normalized for MSTSC. Temporary profile names are predictable/contain host text and use truncating writes.
- Impact: misleading success, stale/wrong mappings, profile races and failed sharing. Severity medium; likelihood medium/high by subcase; confidence high.
- Remediation: validated/normalized inputs, canonical directory and exact owned mapping cleanup, secure temp profile RAII, status propagation and explicit unsupported-platform errors.
- Verification: `aud_012`, `aud_013`, profile/guard/status tests; actual Windows mapping gate remains external.
- Status: Implemented normalized/checked profiles, canonical folder/exact mapping teardown, secure temp file, async owned child and status propagation; portable regressions plus complete Windows type/lint pass. Actual mstsc/DOS execution remains external.

### MFZ-A20 — Hosts parsing/elevation can report work that did not happen
- Location: `HostsManager`, Windows `shell_execute_runas`.
- Evidence: comment words count as aliases; case/IP ignored; ensure_hosts swallows reads; UAC launch success is reported as update success before child exit; relative config args can resolve differently after elevation; Windows path is hardcoded.
- Impact: incomplete/wrong mappings and false success. Severity medium; likelihood medium; confidence high.
- Remediation: validated exact host/IP parser, errors propagated, elevation awaited/status checked with correctly quoted absolute arguments, system-derived path.
- Verification: `aud_014`, `aud_015`, conflict/comment/case/error tests, Windows API type-check and external UAC gate.
- Status: Implemented exact parser/IP/conflict/batch preflight, bounded locked append/rollback, checked UAC wait/exit/postcondition and validated snapshot arguments. Local file/argument tests and complete Windows type/lint pass; actual elevation remains external.

### MFZ-A21 — Conflicting/ignored CLI choices and multi-export overwrite
- Location: `ExportArgs`, `cmd_export`, `cmd_list`.
- Evidence: standalone flag is ignored; --all and environment are accepted together; --all with a single output filename overwrites each previous profile; pointer exports omit the selected config path; help suggests nonexistent --fullscreen.
- Impact: user requests ignored, lost exports, wrong profile source. Severity medium; likelihood medium; confidence high.
- Remediation: Clap conflict rules, validate plan before writes, self-contained config pointer, accurate help.
- Verification: `aud_016`, CLI parse/plan/export-load regressions.
- Status: Implemented conflict/plan/source-pointer/export policy and corrected help/README commands/defaults; original parser regressions and 25 actual offline CLI checks pass.

### MFZ-A22 — Logging worker is deliberately leaked rather than flushed
- Location: `src/main.rs::setup_logging`, main exit path.
- Evidence: WorkerGuard is mem::forget'd and process::exit is used on error. No guard drop can drain queued logs at normal/error exit.
- Impact: final diagnostic events lost; no clean resource shutdown. Severity medium; likelihood medium for fast failure; confidence high.
- Remediation: return/retain guard, use ExitCode return so owned resources drain, keep logging initialization failures recoverable.
- Verification: subprocess short-run/log-flush test where possible and full binary check.
- Status: Implemented retained/flushed guard, fallible init and ExitCode ownership. Actual application subprocesses verified 12 distinct final diagnostic events persisted before process exit.

## Quality and verification gaps (medium/low as specified)

### MFZ-A23 — Tests encode developer-machine state and bypass important suites
- Location: `tests/{shell,rdp,credentials_test,config}.rs`, `.github/workflows/native-transport.yml`.
- Evidence: tests require a Desktop, C: and D: and no E:, use fixed temp/keyring names, touch a real secure store by default; Windows workflow runs --lib --bin only (omits tests/). No QR utility contract tests existed.
- Impact: false failures or destructive/nonisolated tests; meaningful failures never exercised by CI. Severity medium verification gap; likelihood high across machines; confidence high.
- Remediation: isolated fixtures/assertions against OS discovery rather than assumptions, opt-in real-store integration, portable actual-module coverage, run integration tests in CI.
- Verification: complete portable application-core suite plus Windows all-target compile; CI supplied, not claimed remotely executed.
- Status: Actual-module harness includes original application/QR/SDK suites; isolated fixtures and meaningful OS-boundary assertions pass. CI now includes integration/explicit credential/actual CLI steps; remote CI and native Windows execution are not claimed.

### MFZ-A24 — Existing strict-lint failures were previously outside the transport scope
- Location: `cli::TopCommand`, hosts SW_SHOW cast, RDP ASCII check, shell unwrap, tests/config and tests/error.
- Evidence: previous full-root lint required six category overrides. This mission now includes those areas; overrides are not an acceptable final verification shortcut.
- Impact: noisy/broken strict quality gate, minor enum/layout/maintainability costs. Severity low, not counted as security defects; confidence high.
- Remediation: coherent local fixes, not blanket allow attributes or weakened assertions.
- Verification: full root all-target Clippy with -D warnings and no category exemptions.
- Status: Strict complete-root all-target Clippy passes on Linux AND Windows GNU with -D warnings and without the six old category exemptions; expanded-harness strict lint and stable fmt pass.

### MFZ-A25 — Secret-bearing types derive unrestricted Debug
- Location: EnvironmentConfig, MfaCredentials, AwsAuthSource, QR structs, RDP launch options and CLI args.
- Evidence: Debug implementations can print passwords/seeds/tokens. No current production call logging these entire objects has been established; this is defense-in-depth/future-change risk, not a proven current exfiltration exploit.
- Severity low/medium; likelihood low today; confidence high on representation.
- Remediation: remove unused Debug derives or implement redacted Debug at necessary public boundaries; retain useful error diagnostics without secret material.
- Verification: sentinel-secret formatting tests/static call trace.
- Status: Necessary Debug output redacted/unused derives removed; sentinel formatting tests and call tracing pass. This remains defense-in-depth, not a claim of an observed original production leak.

## Additional confirmed discoveries / remediation regressions

### MFZ-A26 — Potentially genuine MFA seeds in tracked legacy references and prior ZIPs
- Category/location: sensitive material/provenance; baseline `refs/environment.txt` lines 15 and 26, original reference configuration and earlier delivery archives.
- Evidence: two non-placeholder-looking 64-character Base-32 values paired with MFA device ARNs. A byte comparison confirmed the same values in `mfz/refs/environment.txt` in the v1 transfer-fix, v2 performance and v3 production ZIPs. Values are NOT reproduced in reports/new deliverables; `legacy-material-exposure.json` records only metadata.
- Root cause/impact: a supposedly harmless reference config carried authentication-factor material into source packages. A TOTP seed is not sufficient alone to authenticate to AWS, but compromises the MFA factor if genuine. Active/revoked status and actual compromise are unknown; no credential probing was performed.
- Severity high conditional on validity; likelihood unknown; confidence high on presence/archive replication, unknown on live validity. Affects documentation/examples, package generation, Git/history and the corresponding MFA devices.
- Remediation: remove unused legacy Python/config artifacts after confirming no callers; provide sanitized current `mfz.ini.example` and provenance note. Preserve earlier artifacts per instruction, explicitly flag them as NOT sanitized. Never generate an unredacted deletion patch for the old config. If genuine, replace the corresponding devices; source removal cannot revoke credentials or erase Git/upstream history.
- Verification: parsed legacy Python without executing imports, caller search, seed-shape classification and byte comparison of all three ZIPs; current files/reports contain no values. Final sanitized-package check pending.
- Status: Removed active legacy artifacts after caller/provenance review; current package files have no exact known-value match. Earlier archives/history unchanged and explicitly flagged; genuine-device replacement/revocation is UNKNOWN and external.

### MFZ-A27 — Unregister deletes another application's extension data
- Category/location: local data ownership; baseline `shell::unregister_file_association` recursively removes both `.mfz` and `mfz.session` without checking current command/default ownership.
- Evidence/root cause: after another app or another MFZ installation takes ownership, unregister still deletes its registration subtree. Registration also changes the extension before the open command is complete, so partial failure can leave a broken default.
- Impact: lost per-user extension/OpenWith configuration and dangling defaults. Severity medium; likelihood low/medium conditional on reassignment/partial failure; confidence high from direct source/caller trace. Affects per-user HKCU shell settings, not an elevation bypass.
- Remediation: complete ProgID before changing extension, retain prior default, preserve unrelated subkeys, verify this executable's command before uninstall and notify Explorer. Do not bypass protected Windows UserChoice defaults.
- Verification: portable ownership/quoting policy tests added; actual registry behavior requires Windows-target check and isolated Windows execution (not yet done).
- Status: Implemented command ownership/prior-default/unrelated-subkey/orphan-default handling. Portable policy tests and complete Windows type/lint pass; actual registry/UserChoice behavior remains an external gate.

### MFZ-A28 — Intermediate atomic-save implementation could broaden Windows ACLs
- Category/location: remediation regression/security metadata; intermediate `storage::atomic_write` used a destination-directory-inherited tempfile and MoveFileEx replacement.
- Evidence/root cause: a destination file may have a tighter DACL than its parent; inherited temporary-file security is not equivalent, and tightening only after writing would leave a secret-read window. This risk was introduced while correcting upstream's truncating writes, not attributed to the original direct-write implementation.
- Impact: local secret exposure under a broader parent ACL; severity high conditional on permissions, likelihood environment-dependent, confidence high on Windows file semantics. Affects config, explicit secret exports and QR saves.
- Remediation: private-at-creation descriptor granting the process user/SYSTEM for secret-bearing temp files; preserve an existing destination DACL with checked ReplaceFileW. Handle documented partial-failure states with a preserved recovery copy/no-clobber restoration. Do not use unsupported WRITE_THROUGH or ignore ACL errors. Existing destination grants are preserved, not promised to be revoked; protect output file ACLs too.
- Verification: official Microsoft API contract inspected; protected-DACL-before-write and DACL-preserving replacement tests authored. Windows implementation/test target has NOT yet been checked or executed at this checkpoint.
- Status: Intermediate ACL regression corrected with private creation/DACL preservation/recovery retention; complete Windows code and native-test targets type/lint pass. Real Windows ACL/partial-failure/cleanup execution remains an external gate.

## Additional discoveries / residuals

This is a living inventory, not a claim that investigation is finished. Native
transport re-examination, reference-file secret/provenance checking, full dependency
reachability and post-fix neighboring-code review remain on COVERAGE.md. No
hypothetical database/HTTP/tenant findings are invented for components absent here.
