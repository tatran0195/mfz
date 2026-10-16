# External Windows/AWS acceptance gates

**Not executed by the Linux audit worker.** Use a disposable Windows user/VM,
nonproduction AWS instance, least-privilege account and dummy/disposable RDP
credentials. Do not experiment with another user's saved credentials, production
hosts aliases or real data. Cross-checks/SDK loopbacks do not replace these gates.

## 1. Windows build and isolated tests

```powershell
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --test credentials_test -- --ignored --nocapture
cargo build --locked --bin mfz
python tools/audit_cli_test.py --binary target/debug/mfz.exe
```

Default tests use private temporary files/local SDK peers, not live AWS/system
hosts modifications. The explicit credential test writes/deletes only a random
`mfz_test_*` identity. Record the actual Windows version, compiler and exit codes.

## 2. File security and recovery

- Run the protected-DACL-before-write and DACL-preserving-overwrite tests on NTFS.
  Confirm no `.mfz-*` recovery artifact remains after an ordinary successful write.
- Compare original/output DACLs for an existing file tighter than its directory.
  New secret-bearing files must initially grant only the process user/SYSTEM.
- Test read-only/denied writes and a held conflicting file handle. No original data
  may be silently truncated. If ReplaceFile reports partial failure, inspect the
  explicitly reported preserved/recovery paths before retrying; a backup may
  contain previous secrets. Do not test destructive power failure without a VM snapshot.
- Check redirected/localized Desktop and a long Unicode path; no guessed OneDrive
  directory or lossy/empty path substitution is allowed.

## 3. Credentials, mstsc and DOS mapping ownership

- With a disposable exact-port `TERMSRV/127.0.0.1:<port>` entry already present,
  automatic password launch must fail without overwriting/deleting it.
- Test an owned new entry, normal child exit, nonzero exit, Ctrl+C and native tunnel
  failure. Verify the app only attempts removal when marker/username/blob match.
  Check both exact-port and pre-existing host-only credential lookup behavior on
  supported mstsc versions; it must not silently use an unrelated password.
- Change the disposable credential while a session is active; cleanup must preserve
  observed unowned/changed state. Windows has no compare-and-delete transaction
  against arbitrary noncooperating applications; record concurrent-edit limits.
- Redirect a canonical directory with occupied drive letters, then close/cancel.
  Only the exact owned DOS target may be removed. Test an ordinary file/nonexistent
  directory failure and a replaced mapping using a disposable letter/session only.
- Confirm NLA and server-identity warnings remain; do not disable certificate/NLA
  policy or clipboard/drive support just to make the test pass.

## 4. Explicit elevation and shell registration

- On a disposable VM, snapshot the hosts file. Test existing matching mapping,
  conflicting address, invalid complete batch, UAC cancellation, elevated child
  failure and success. No success may be printed merely because UAC launched.
  Timeout must state uncertainty; do not kill an unrelated elevated process.
- Confirm the elevated command applies validated arguments and does not reread a
  mutable user config. Config add/launch must not implicitly elevate/edit hosts.
- Back up the disposable user's association state. Register, change default to
  another application/installation, then unregister. Preserve unrelated extension
  subkeys and another executable's command; restore prior defaults where owned.
  Check partial/orphan install cleanup and Windows UserChoice behavior via Settings.

## 5. Live cloud and guest workload

- Independently authorize the intended instance/region/profile and unique tag.
  Test no match, multiple matches and a match on a later EC2 page. IAM remains the
  real authorization boundary; no IAM-bypass claim was made by local validation.
- Test direct static keys without a token, IAM TOTP, role/cached SSO and an explicit
  region override. Initial SSO bootstrap is external to MFZ; unsupported KMS
  handshake actions must fail closed, never silently downgrade/fall back to CLI.
- Verify normal and failed native starts, Ctrl+C, expired/denied/network cleanup
  outcomes and bounded `TerminateSession`. No access to unrelated sessions/processes.
- Enroll a disposable virtual MFA device; cancel/input EOF/save failure and inspect
  pending cleanup. Once enable is acknowledged, local save failure must report
  **active device / save failed**, never silently deactivate it or say full success.
- Clipboard and redirected-drive transfers share one native RDP tunnel. Exercise
  small and large files in both directions, duplex traffic, reconnect/half-close,
  idle/slow consumer and realistic RTT/loss while checking other guest connections,
  CPU/commit/disk/DPC/network counters. Local writer/Go results do not prove the
  original **whole-VM** symptom fixed; preserve independent guest diagnostics.

## 6. Historical material

If the two MFA seeds in the old reference config were genuine, replace the
corresponding devices and handle old archives/history under your credential policy.
Current source removal is not revocation or secure erasure. Never paste the old
seed values into test logs, issues, patches or the acceptance report.
