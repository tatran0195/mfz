# Repository investigation coverage

Started/updated: 2026-10-15. Branch `fix/repository-remediation`, upstream HEAD
`0817c50175cc6c8714776fff9467684ecab98426`; no commits or pushes. Preserve delivered-v3
source tree `668854b1c20b58e175046482481e44b326c96883` and prior artifacts. Evidence is
also retained outside generated/cache directories in `/home/user/mfz-audit-baseline/`.

## Actual application and boundaries

MFZ is a Windows desktop CLI. It reads INI/portable profiles, resolves Windows
keyring/AWS credentials and MFA, selects EC2 targets, creates a native SSM tunnel
and launches Windows Remote Desktop. It optionally manages hosts aliases,
per-user shell association and IAM virtual MFA/QR utilities. There is **no**
database, HTTP application server/frontend, tenant table or database migration
system. Those categories are not applicable, not silently omitted.

AWS SDK/WebSocket peers, local/profile files, Windows credentials/DOS mappings,
elevation, shell registration and remote RDP identity are the relevant boundaries.
IAM authorizes resource operations; a tag is not a unique identifier unless checked.
Loopback limits network exposure but is not per-user authentication: trust the local
machine and retain RDP/server authentication. Default config discovery assumes
trusted working/executable/user directories; use explicit private paths elsewhere.

## Examined and remediated coverage

| Area | Actual files/workflow examined | Evidence / current boundary |
|---|---|---|
| Intent/history/build/features | Cargo manifests/locks, build.rs, rustfmt.toml, README, original design plans | Actual Windows desktop/native intent traced. Old design plans are history, not proof of current behavior. Platform-only registry dependency, supported feature pruning and stable fmt corrected. |
| CLI/startup/dispatch | cli.rs, main.rs; config/export/launch/hosts/QR through all callers | Migrated actual callers; 25 real-application offline subprocess cases passed, including final log flushing and an independently reproduced explicit-config-origin regression. Final root lint/check ledger records remaining work. |
| Config/model/persistence | config.rs, storage.rs, validation.rs, original config tests/audit tests | All model fields/aliases, quoting, malformed sections, pointers, relative paths, locked full edits, verified migration and failures traced/tested. Explicit and omitted `--config` now preserve distinct origins. |
| Secure storage | credentials.rs, credential test | Windows backend contract inspected, unsupported mock persistence rejected, case/name normalization and fresh readback implemented. Portable tests passed; real Windows round trip is opt-in/disposable, not executed here. |
| AWS identity/MFA | mfa.rs, otp.rs, original/new MFA tests | Optional tokens, fail-closed incomplete MFA, profile region/provider/cache and refresh-capable non-MFA config traced; real SDK profile subprocess/HTTP/signing tests passed. No live account/login bootstrap claimed. |
| EC2 target selection | aws.rs, instance_selection.rs and callers | Full paginated running-instance path, split-once tag value, distinct-target and pathological pagination handling traced/tested. Complete SDK integration type-checked; actual EC2 account enumeration not run. |
| RDP outputs/lifecycle | rdp.rs, rdp_windows.rs, main launch and tests | Numeric endpoint, injection/drive validation, profile settings, canonical folder/exact target cleanup, owned async child and credential lifecycle reviewed. Portable tests passed; Windows boundary code/tests cross-compiled and strictly linted. Actual mstsc/LSASS/DOS behavior remains external. |
| Hosts/elevation | hosts.rs, CLI hidden snapshot apply, original/new tests | Comments/case/IP/conflicts/complete-batch preflight, bounded read, actual-inode append/rollback, UAC handle/wait/exit/postcondition and snapshot arguments traced. No elevated config reread; no implicit hosts edit during launch/add. Actual UAC not run. |
| File security/atomic recovery | storage_windows.rs and callers | Microsoft ReplaceFile contract and partial-error states inspected. Protected private creation, existing DACL retention and recovery preservation implemented. Windows test targets compile; DACL and filesystem behavior still need real Windows execution. |
| Association/export | shell.rs, CLI/main, original/new shell tests | No-clobber/secret-free default, explicit policies, exact source pointers, ownership/prior-default/unrelated-subkey preservation and orphan-default repair reviewed. Native registry operations not executed on Linux. |
| QR/local image parsing | qr.rs, otp.rs and tests | Actual PNG/URI round trip, URL encoding, Base-32, bounds, no-clobber writes and independent TOTP interpretation passed. Supported decoder features retained; unused AVIF encoder disabled (not a decoder regression). |
| IAM enrollment | enrollment.rs and main/CLI | Creation through pending ownership, presentation/input, enable, persistence and failure paths traced. Real SDK local XML peers cover cancellation, missing seed, PNG failure, activation/save failure; actual closed-input subprocess tested. Live IAM not run. |
| Errors/UI/logging | error.rs, ui.rs, main.rs and actual binary checks | No current whole-object secret logging established; necessary Debug redacted/removed. WorkerGuard owned/flushed; normal/error ExitCode flow verified with 12 unique final log events. Fixed spinner template unwrap is an invariant, not an invented vulnerability. |
| Native SSM reliability/writer/protocol | ssm*.rs, transport_buffer.rs and tests | Re-read full codec/window/writer, handshake/read/dispatch/close and cleanup ownership paths after changes. Bounded count/byte/gap reserve, selective ACK IDs, exact retransmission, fair event-driven batches, allocation retention and cancellation traced. No new confirmed packet hot-path defect found in this pass. Current native/SDK tests and two actual Go interoperability tests passed. |
| Smux | smux.rs and tests/pinned Go peer | Complete open/queue/schedule/FIN/read/write/drop/receive-credit paths reviewed, limits/lock ordering/wakers traced, actual slow-stream/duplex/FIN interoperability executed. Shared-budget head-of-line blocking at the *full session budget* is deliberate bounded backpressure, not an invented performance bug. |
| Legacy refs/provenance | old refs/script.py/environment.txt, caller search, source ZIP byte checks | Parsed old Python without executing imports; no current caller found. Removed obsolete alternate runtime/config; sanitized current template/provenance added. Two seed-like values were in all three historical ZIPs; validity unknown and rotation external (MFZ-A26). Never publish an unredacted deletion diff. |
| Dependencies | actual features, registry implementations/APIs used, exact OSV scans, both locks | Prior security-advisory versions patched/unused stacks removed. Full third-party source/formal audit is NOT claimed. Current remaining OSV matches are maintenance notices, with effective-path reachability documented separately. |
| CI/release/docs | native-transport.yml, README, existing reports and new audit documents | Expanded triggers/application/QR/SDK/all-target integration/real credential opt-in/actual CLI checks; CI file is provided, not remotely executed. Prior native performance results retain their historical source/method limits. |

## Verification status and remaining uncertainty

The executed progress suite is now 146 passed / 0 failed / 3 intentionally ignored;
the two Go cases and the opt-in release writer benchmark separately passed (paired against preserved v3 on the same worker). Actual application build and 25 offline CLI
cases passed. Windows boundary all-target compile and strict lint passed. Complete full-root Linux AND Windows-GNU all-target type-check/strict lint also
passed. Exact execution is in `VERIFICATION.md`; native Windows API/cloud execution
is still not implied by compilation.

Partial/unavailable areas are explicit:

- Real Windows filesystem ACL/recovery failure behavior, registry reassignment,
  mstsc identity/credential selection, DOS-device races, Credential Manager
  persistence and UAC acceptance/cancellation/timeout.
- Live AWS permissions, account-specific SSO cache/login, EC2 eventual consistency,
  IAM/MFA activation and cleanup, service response/network ambiguity and expiry.
- The affected guest/clipboard/redirected-drive transfers and remote CI.
- Third-party dependency implementations outside the inspected APIs/features and
  advisory paths; current OSV knowledge is not a proof of absence of vulnerabilities.
- Sudden kill/power loss or noncooperating outside editors/processes cannot be made
  a cross-resource transaction; backups/recovery states are reported explicitly.

## Deliberate non-removals/trade-offs

- Required pinned AWS-vendored Go smux code/license/hash manifest is retained for
  genuine interoperability/provenance; unlike the unused Python launcher, it has
  concrete test callers. Native Rust transport remains the production runtime.
- SDK SSO and optional credential_process support remain; no required AWS CLI/plugin
  fallback was reintroduced. Initial interactive SSO bootstrap and KMS-encrypted
  handshake actions are not implemented; unsupported handshakes fail closed.
- Explicit plaintext config/QR/enrollment save remains an intentional supported
  workflow with warnings and verified migration; ordinary exports omit secrets.
- Existing output DACLs are preserved, not silently revoked; secure migration is
  not secure erasure of backups/history/previous source archives.
- No database/web/tenant requirements or speculative critical exploits are invented.
