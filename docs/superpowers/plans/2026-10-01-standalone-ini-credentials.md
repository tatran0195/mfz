# Standalone `mfz.ini` Credentials Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable `mfz` to operate 100% standalone using direct AWS credentials (`aws_access_key_id` and `aws_secret_access_key`) in `mfz.ini` or Windows Keyring, with optional `region` and backwards-compatible profile fallback.

**Architecture:** Add `aws_access_key_id`, `aws_secret_access_key`, and `region` to `EnvironmentConfig`, with secret resolution falling back to Windows Keyring (`CredentialStore`). Introduce `AwsAuthSource::Direct` and `AwsAuthSource::Profile` in `mfa.rs` so that `MfaSessionManager` can authenticate directly against AWS STS with static credentials without querying `~/.aws/credentials`. Wire resolution and CLI display in `main.rs`.

**Tech Stack:** Rust (edition 2021), `aws-config`, `aws-sdk-sts`, `keyring`, `configparser`, `clap`.

## Global Constraints

- Standalone mode MUST NOT require or touch `~/.aws/credentials` or `~/.aws/config`.
- `profile` MUST remain supported as an optional fallback when `aws_access_key_id` is omitted.
- Missing both `aws_access_key_id` and `profile` MUST return an explicit, actionable validation error.
- Default region is `"ap-northeast-1"`, overridable by `mfz.ini` (`region`), overridable by CLI (`--region`).
- Follow established code formatting: `cargo +nightly fmt` or `cargo fmt`.

---

### Task 1: Extend `CredentialStore` for AWS Secret Key

**Files:**

- Modify: `src/credentials.rs:1-60`
- Test: `tests/credentials_test.rs` (or unit test in `src/credentials.rs`)

**Interfaces:**

- Consumes: `keyring::Entry`, `crate::error::{AppError, Result}`
- Produces:

  ```rust
  impl CredentialStore {
      pub fn set_aws_secret_key(env_name: &str, secret_key: &str) -> Result<()>;
      pub fn get_aws_secret_key(env_name: &str) -> Result<Option<String>>;
      pub fn delete_secrets(env_name: &str) -> Result<()>; // updated to also clear aws-secret
  }
  ```

- [ ] **Step 1: Write unit tests for AWS secret key in keyring**

Create or add to `tests/credentials_test.rs`:

```rust
use mfz::credentials::CredentialStore;

#[test]
fn test_aws_secret_keyring_roundtrip() {
    let test_env = "test_env_keyring_spec";
    let test_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    // Set
    let set_res = CredentialStore::set_aws_secret_key(test_env, test_key);
    assert!(set_res.is_ok());

    // Get
    let read_key = CredentialStore::get_aws_secret_key(test_env).unwrap();
    assert_eq!(read_key.as_deref(), Some(test_key));

    // Delete
    let del_res = CredentialStore::delete_secrets(test_env);
    assert!(del_res.is_ok());

    // Verify deleted
    let after_del = CredentialStore::get_aws_secret_key(test_env).unwrap();
    assert_eq!(after_del, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test credentials_test`
Expected: FAIL with `cannot find function set_aws_secret_key in CredentialStore`

- [ ] **Step 3: Implement `set_aws_secret_key`, `get_aws_secret_key`, and update `delete_secrets` in `src/credentials.rs`**

In `src/credentials.rs`:

```rust
    pub fn set_aws_secret_key(env_name: &str, secret_key: &str) -> Result<()> {
        let entry = Entry::new("mfz-cli", &format!("aws-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        entry
            .set_password(secret_key)
            .map_err(|e| AppError::Config(format!("Failed to save AWS secret key: {e}")))?;
        Ok(())
    }

    pub fn get_aws_secret_key(env_name: &str) -> Result<Option<String>> {
        let entry = Entry::new("mfz-cli", &format!("aws-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Config(format!("Failed to read AWS secret key: {e}"))),
        }
    }
```

And in `delete_secrets`:

```rust
        let aws_entry = Entry::new("mfz-cli", &format!("aws-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        let _ = aws_entry.delete_credential();
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test credentials_test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/credentials.rs tests/credentials_test.rs
git commit -m "feat(credentials): add AWS secret key storage to Keyring"
```

---

### Task 2: Update `config.rs` Schema and Parsing for Standalone Keys & Region

**Files:**

- Modify: `src/config.rs:10-222`
- Test: `tests/config.rs`

**Interfaces:**

- Consumes: `crate::credentials::CredentialStore`, `configparser::ini::Ini`
- Produces:

  ```rust
  pub struct EnvironmentConfig {
      pub profile: Option<String>,
      pub aws_access_key_id: Option<String>,
      pub aws_secret_access_key: Option<String>,
      pub region: Option<String>,
      // ... existing fields
  }
  impl EnvironmentConfig {
      pub fn resolve_aws_secret_key(&self, env_name: &str) -> Result<Option<String>>;
  }
  ```

- [ ] **Step 1: Write failing tests in `tests/config.rs`**

Add tests to `tests/config.rs`:

```rust
#[test]
fn test_standalone_config_parsing() {
    let content = r#"
[stg_standalone]
aws_access_key_id=AKIAIOSFODNN7EXAMPLE
aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
region=ap-northeast-1
serial=arn:aws:iam::123:mfa/waka
instance=i-abc
remote_port=3389
local_port=13389
rdp_host=stg-rdp
"#;
    let file = create_temp_config(content);
    let loader = ConfigLoader::new(&file);
    let cfg = loader.load("stg_standalone").unwrap();

    assert_eq!(cfg.aws_access_key_id.as_deref(), Some("AKIAIOSFODNN7EXAMPLE"));
    assert_eq!(cfg.aws_secret_access_key.as_deref(), Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"));
    assert_eq!(cfg.region.as_deref(), Some("ap-northeast-1"));
    assert_eq!(cfg.profile, None);
    assert_eq!(cfg.resolve_aws_secret_key("stg_standalone").unwrap().as_deref(), Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"));
}

#[test]
fn test_missing_auth_error() {
    let content = r#"
[invalid_env]
instance=i-abc
remote_port=3389
local_port=13389
rdp_host=stg-rdp
"#;
    let file = create_temp_config(content);
    let loader = ConfigLoader::new(&file);
    let err = loader.load("invalid_env").unwrap_err();
    assert!(err.to_string().contains("must specify either `aws_access_key_id` or `profile`"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config test_standalone_config_parsing`
Expected: FAIL (missing fields / profile required)

- [ ] **Step 3: Update `src/config.rs`**

1. Change `REQUIRED_KEYS`:

   ```rust
   const REQUIRED_KEYS: &[&str] = &["remote_port", "local_port", "rdp_host"];
   ```

2. Update `EnvironmentConfig`:
   - Change `pub profile: Option<String>`.
   - Add `pub aws_access_key_id: Option<String>`.
   - Add `pub aws_secret_access_key: Option<String>`.
   - Add `pub region: Option<String>`.
3. In `from_section`:
   - Read `profile: get_opt("profile")`.
   - Read `aws_access_key_id: get_opt("aws_access_key_id")`.
   - Read `aws_secret_access_key: get_opt("aws_secret_access_key")`.
   - Read `region: get_opt("region")`.
   - Validate auth:

     ```rust
     if profile.is_none() && aws_access_key_id.is_none() {
         return Err(AppError::Config(format!(
             "[{name}] must specify either `aws_access_key_id` (standalone) or `profile` (AWS profile)"
         )));
     }
     ```

4. Implement `resolve_aws_secret_key`:

   ```rust
   pub fn resolve_aws_secret_key(&self, env_name: &str) -> Result<Option<String>> {
       if let Some(ref k) = self.aws_secret_access_key {
           return Ok(Some(k.clone()));
       }
       if self.aws_access_key_id.is_some() {
           let sec = CredentialStore::get_aws_secret_key(env_name)?.ok_or_else(|| {
               AppError::Config(format!(
                   "No AWS secret access key found for [{env_name}] in config or keyring"
               ))
           })?;
           return Ok(Some(sec));
       }
       Ok(None)
   }
   ```

5. Update `to_map()` to include `aws_access_key_id`, `aws_secret_access_key`, `region`, and conditional `profile`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test config`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config.rs
git commit -m "feat(config): support standalone aws credentials and optional region"
```

---

### Task 3: Update `mfa.rs` Authentication Engine for `AwsAuthSource`

**Files:**

- Modify: `src/mfa.rs:50-160`
- Test: `tests/mfa.rs`

**Interfaces:**

- Consumes: `aws_sdk_sts`, `aws_config`, `aws_credential_types`
- Produces:

  ```rust
  #[derive(Debug, Clone)]
  pub enum AwsAuthSource {
      Direct {
          access_key_id: String,
          secret_access_key: String,
      },
      Profile(String),
  }

  impl MfaSessionManager {
      pub fn new(
          auth_source: AwsAuthSource,
          region: Option<String>,
          serial: Option<String>,
          secret: Option<String>,
      ) -> Self;
  }
  ```

- [ ] **Step 1: Write test for `AwsAuthSource` in `tests/mfa.rs`**

In `tests/mfa.rs`:

```rust
use mfz::mfa::{AwsAuthSource, MfaSessionManager};

#[test]
fn test_mfa_manager_creation_with_direct_auth() {
    let auth = AwsAuthSource::Direct {
        access_key_id: "AKIAEXAMPLE".to_string(),
        secret_access_key: "SECRETEXAMPLE".to_string(),
    };
    let mgr = MfaSessionManager::new(
        auth,
        Some("ap-northeast-1".to_string()),
        Some("arn:aws:iam::123:mfa/user".to_string()),
        Some("BASE32SECRET".to_string()),
    );
    assert_eq!(mgr.region().as_deref(), Some("ap-northeast-1"));
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --test mfa`
Expected: FAIL (types / signature mismatch)

- [ ] **Step 3: Update `src/mfa.rs`**

1. Define `AwsAuthSource`:

   ```rust
   #[derive(Debug, Clone)]
   pub enum AwsAuthSource {
       Direct {
           access_key_id: String,
           secret_access_key: String,
       },
       Profile(String),
   }
   ```

2. Update `MfaSessionManager`:

   ```rust
   pub struct MfaSessionManager {
       auth_source: AwsAuthSource,
       region: Option<String>,
       serial: Option<String>,
       secret: Option<String>,
       cached: Option<MfaCredentials>,
   }
   ```

3. Update `acquire_session_token()`:

   ```rust
   let config = match &self.auth_source {
       AwsAuthSource::Direct { access_key_id, secret_access_key } => {
           let static_creds = aws_sdk_sts::config::Credentials::new(
               access_key_id.clone(),
               secret_access_key.clone(),
               None,
               None,
               "mfz-direct",
           );
           let mut loader = aws_config::defaults(BehaviorVersion::latest())
               .credentials_provider(static_creds);
           if let Some(ref r) = self.region {
               loader = loader.region(aws_config::Region::new(r.clone()));
           }
           loader.load().await
       }
       AwsAuthSource::Profile(profile) => {
           let mut loader = aws_config::defaults(BehaviorVersion::latest())
               .profile_name(profile);
           if let Some(ref r) = self.region {
               loader = loader.region(aws_config::Region::new(r.clone()));
           }
           loader.load().await
       }
   };
   let client = StsClient::new(&config);
   ```

4. Update `load_from_profile()` to handle both direct mode (returns credentials directly if not expired) and profile mode.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test mfa`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/mfa.rs tests/mfa.rs
git commit -m "feat(mfa): support direct static credentials via AwsAuthSource"
```

---

### Task 4: Wire Auth & Region Resolution in `main.rs`, `cli.rs`, and `shell.rs`

**Files:**

- Modify: `src/main.rs:115-130, 210-290, 500-580`
- Modify: `src/cli.rs` (credential management subcommands if needed)
- Modify: `src/shell.rs:133-155`

**Interfaces:**

- Consumes: `AwsAuthSource`, `EnvironmentConfig`, `CredentialStore`
- Produces: Working CLI with dynamic auth source detection, region resolution, and keyring helpers.

- [ ] **Step 1: Update session launch and auth resolution in `src/main.rs`**

In `cmd_launch` (and interactive run):

1. Resolve region:

   ```rust
   let resolved_region = args
       .region
       .clone()
       .or_else(|| env.region.clone())
       .unwrap_or_else(|| "ap-northeast-1".to_string());
   ```

2. Resolve auth source:

   ```rust
   let auth_source = if let Some(ref key_id) = env.aws_access_key_id {
       let secret_key = env.resolve_aws_secret_key(&env_name)?;
       let secret_key = secret_key.ok_or_else(|| {
           AppError::Config(format!(
               "Missing AWS secret access key for environment `{env_name}`"
           ))
       })?;
       AwsAuthSource::Direct {
           access_key_id: key_id.clone(),
           secret_access_key,
       }
   } else if let Some(ref p) = env.profile {
       AwsAuthSource::Profile(p.clone())
   } else {
       return Err(AppError::Config(format!(
           "Environment `{env_name}` must configure either `aws_access_key_id` or `profile`"
       )));
   };
   ```

3. Initialize `MfaSessionManager`:

   ```rust
   let mut mfa = MfaSessionManager::new(
       auth_source.clone(),
       Some(resolved_region.clone()),
       env.serial.clone(),
       totp_secret,
   );
   ```

4. Update UI summary print:

   ```rust
   match &auth_source {
       AwsAuthSource::Direct { access_key_id, .. } => {
           print_kv("AWS Auth", &format!("Direct Key ({})", access_key_id));
       }
       AwsAuthSource::Profile(p) => {
           print_kv("AWS Profile", p);
       }
   }
   print_kv("Region", &resolved_region);
   ```

5. Update `env.profile` references in `cmd_list`, `cmd_env_add`, etc. to handle `Option<String>` safely.

- [ ] **Step 2: Add `set-aws-secret` to CLI Creds command in `src/cli.rs` and `src/main.rs`**

Support `mfz creds set-aws-secret <env>` to save key to keyring:

```rust
CredsCommand::SetAwsSecret { env } => {
    let secret = inquire::Password::new("Enter AWS Secret Access Key:")
        .without_confirmation()
        .prompt()
        .map_err(|e| AppError::Config(e.to_string()))?;
    CredentialStore::set_aws_secret_key(&env, &secret)?;
    println!("AWS secret access key saved for `{env}`.");
}
```

- [ ] **Step 3: Update `export_mfz_file` in `src/shell.rs`**

Ensure standalone export serializes `aws_access_key_id`, `aws_secret_access_key` (if present), and `region`.

- [ ] **Step 4: Verify build and test**

Run: `cargo test` and `cargo check`
Expected: Zero compilation errors, all tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/main.rs src/cli.rs src/shell.rs
git commit -m "feat: wire standalone auth and region resolution into CLI and shell export"
```

---

### Task 5: End-to-End Verification and Documentation

**Files:**

- Modify: `README.md`, `README.dev.md`
- Verify: Full test suite

- [ ] **Step 1: Update documentation and example in `README.md` and `README.dev.md`**

Show example `mfz.ini` with both standalone mode (`aws_access_key_id`) and legacy mode (`profile`).

- [ ] **Step 2: Run formatting and all tests**

Run: `cargo fmt`
Run: `cargo test`
Expected: All tests pass, zero warnings.

- [ ] **Step 3: Commit**

```bash
git add README.md README.dev.md
git commit -m "docs: document standalone mfz.ini credentials and configuration"
```
