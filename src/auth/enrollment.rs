//! IAM virtual-MFA enrollment: pending resources are owned, failures are not
//! reported as success, and a confirmed ACTIVE MFA device is never auto-deleted.
use std::{path::Path, time::Duration};

use aws_config::BehaviorVersion;
use aws_sdk_iam::{error::ProvideErrorMetadata, Client as IamClient};
use tracing::warn;

use super::qr::{self, GeneratedQr};
use crate::{
    config::{AuthType, ConfigLoader},
    error::{AppError, Result},
    ui::{print_info, print_kv, print_section, print_success, print_warning},
    validation,
};

pub struct RegistrationOptions<'a> {
    pub profile: &'a str,
    pub username: &'a str,
    pub account: &'a str,
    pub issuer: &'a str,
    pub out_png: Option<&'a str>,
    pub overwrite_png: bool,
    pub region: Option<&'a str>,
    pub save_env: Option<&'a str>,
    pub config: &'a Path,
}

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

async fn delete_pending(iam: &IamClient, serial: &str) -> Result<()> {
    match tokio::time::timeout(CLEANUP_TIMEOUT, iam.delete_virtual_mfa_device().serial_number(serial).send()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) if error.as_service_error().is_some_and(|error| error.code() == Some("NoSuchEntity")) => Ok(()),
        Ok(Err(error)) => Err(AppError::Iam(format!("Pending-device cleanup failed for {serial}: {error}; inspect IAM before retrying (it may already be assigned)"))),
        Err(_) => Err(AppError::Iam(format!("Pending-device cleanup timed out for {serial}; remote outcome is unknown, inspect IAM before retrying"))),
    }
}

struct PendingDevice {
    iam: IamClient,
    serial: String,
    armed: bool,
}
impl PendingDevice {
    async fn cleanup(&mut self) -> Result<()> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        delete_pending(&self.iam, &self.serial).await
    }
}
impl Drop for PendingDevice {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let iam = self.iam.clone();
        let serial = self.serial.clone();
        // Cancellation fallback, not a guarantee on process kill/runtime exit.
        // DeleteVirtualMFADevice refuses an assigned device; never deactivate it.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = delete_pending(&iam, &serial).await {
                    warn!("{error}");
                }
            });
        } else {
            warn!("Pending MFA device {serial} requires manual inspection; no runtime for cleanup");
        }
    }
}

pub async fn register(options: RegistrationOptions<'_>) -> Result<()> {
    let mut loader = aws_config::defaults(BehaviorVersion::latest()).profile_name(options.profile);
    if let Some(region) = options.region {
        loader = loader.region(aws_config::Region::new(region.to_owned()));
    }
    let iam = IamClient::new(&loader.load().await);
    register_with_client(&iam, &options, |generated, serial| {
        print_section("Scan the new authenticator");
        qr::print_terminal(generated);
        print_kv("Secret (Base-32)", &generated.secret_b32);
        print_kv("Serial", serial);
        if let Some(path) = options.out_png { qr::save_png_with_overwrite(generated, path, options.overwrite_png)?; }
        print_info("Scan the QR code. Enter two consecutive six-digit codes; wait for the code to change before entering the second.");
        Ok(())
    }, prompt_totp).await?;
    print_success("MFA device enabled. Keep the authenticator/recovery material private.");
    Ok(())
}

async fn register_with_client(
    iam: &IamClient,
    options: &RegistrationOptions<'_>,
    present: impl FnOnce(&GeneratedQr, &str) -> Result<()>,
    mut code: impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    for (label, value) in [
        ("Profile", options.profile),
        ("IAM username", options.username),
        ("Account", options.account),
        ("Issuer", options.issuer),
    ] {
        validation::single_line(value, label)?;
        if value.trim().is_empty() {
            return Err(AppError::Iam(format!("{label} must not be empty")));
        }
    }
    let config_loader = ConfigLoader::new(options.config);
    // Fail BEFORE remote mutation for an invalid/missing destination. Never turn
    // a read/parse/permission error into a fabricated REPLACE_ME environment.
    let original = options
        .save_env
        .map(|name| config_loader.load(name))
        .transpose()?;
    if let Some(path) = options.out_png {
        if !options.overwrite_png && Path::new(path).exists() {
            return Err(AppError::QrCode(
                "Output PNG already exists; choose another path or explicitly use --force".into(),
            ));
        }
    }
    let name = format!("{}-mfz-{}", options.username, uuid::Uuid::new_v4());
    let created = iam
        .create_virtual_mfa_device()
        .virtual_mfa_device_name(&name)
        .send()
        .await
        .map_err(|error| AppError::Iam(format!("CreateVirtualMFADevice failed: {error}")))?;
    let device = created.virtual_mfa_device().ok_or_else(|| {
        AppError::Iam(format!(
            "CreateVirtualMFADevice returned no device; inspect IAM for `{name}` before retrying"
        ))
    })?;
    let mut pending = PendingDevice {
        iam: iam.clone(),
        serial: device.serial_number().to_owned(),
        armed: true,
    };
    let enrollment = async {
        let seed = device.base32_string_seed().ok_or_else(|| AppError::Iam("New MFA device response contains no seed".into()))?;
        let seed = std::str::from_utf8(seed.as_ref()).map_err(|_| AppError::Iam("New MFA device seed is not valid UTF-8 Base-32".into()))?;
        let generated = qr::generate(Some(seed), options.account, options.issuer)?;
        present(&generated, &pending.serial)?;
        let first = code("Enter TOTP code 1")?;
        let second = code("Wait for the next interval, then enter TOTP code 2")?;
        if !valid_code(&first) || !valid_code(&second) {
            return Err(AppError::Iam("Both TOTP codes must contain exactly six ASCII digits".into()));
        }
        iam.enable_mfa_device().user_name(options.username).serial_number(&pending.serial)
            .authentication_code1(first).authentication_code2(second).send().await
            .map_err(|error| AppError::Iam(format!("EnableMFADevice failed: {error}")))?;
        // Success is irreversible from this helper's point of view. Do not
        // delete/deactivate an active device if local persistence subsequently fails.
        pending.armed = false;
        if let Some(target) = options.save_env {
            print_warning("--save-env explicitly saves the MFA seed as plaintext in the selected config; run `mfz config secure` to migrate it to Windows Credential Manager.");
            config_loader.update(target, |environment| {
                let original = original.as_ref().ok_or_else(|| AppError::Config("Missing enrollment save preflight".into()))?;
                if environment.serial != original.serial || environment.secret != original.secret
                    || environment.profile != original.profile || environment.aws_access_key_id != original.aws_access_key_id {
                    return Err(AppError::Config("Environment authentication changed during enrollment; refusing to overwrite concurrent edits".into()));
                }
                environment.serial = Some(pending.serial.clone());
                environment.secret = Some(generated.secret_b32.clone());
                environment.auth_type = AuthType::Mfa;
                Ok(())
            }).map_err(|error| AppError::Iam(format!("MFA device {} IS ENABLED, but saving configuration failed: {error}. Keep the displayed seed and update the environment manually; the active device was not deleted", pending.serial)))?;
        }
        Ok(())
    }.await;
    if let Err(error) = enrollment {
        if let Err(cleanup) = pending.cleanup().await {
            return Err(AppError::Iam(format!(
                "Enrollment failed: {error}. {cleanup}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn valid_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())
}

fn prompt_totp(prompt: &str) -> Result<String> {
    loop {
        // inquire handles EOF/non-TTY/escape/Ctrl+C as errors rather than an
        // unbounded read_line(0) retry loop. Do not log authentication codes.
        let code = inquire::Password::new(prompt)
            .without_confirmation()
            .prompt()
            .map_err(|_| AppError::Iam("MFA enrollment input cancelled/unavailable".into()))?;
        if valid_code(&code) {
            return Ok(code);
        }
        print_warning("Code must contain exactly six ASCII digits.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ssm::tests::peer::{HttpPeer, Response};
    use std::collections::BTreeMap;

    const SERIAL: &str = "arn:aws:iam::123456789012:mfa/mfz-local-test";
    const BASIC: &str = "[test]\nprofile=selected\ninstance=i-test\nremote_port=3389\nlocal_port=13389\nrdp_host=test-rdp\n";
    fn created(seed: bool) -> Response {
        let seed = if seed {
            "<Base32StringSeed>SkJTV1kzRFBFSFBLM1BYUA==</Base32StringSeed>"
        } else {
            ""
        };
        Response::new("text/xml", format!("<CreateVirtualMFADeviceResponse xmlns=\"https://iam.amazonaws.com/doc/2010-05-08/\"><CreateVirtualMFADeviceResult><VirtualMFADevice><SerialNumber>{SERIAL}</SerialNumber>{seed}</VirtualMFADevice></CreateVirtualMFADeviceResult><ResponseMetadata><RequestId>mfz-local-test</RequestId></ResponseMetadata></CreateVirtualMFADeviceResponse>").into_bytes())
    }
    fn completed(operation: &str) -> Response {
        Response::new("text/xml", format!("<{operation}Response xmlns=\"https://iam.amazonaws.com/doc/2010-05-08/\"><ResponseMetadata><RequestId>mfz-local-test</RequestId></ResponseMetadata></{operation}Response>").into_bytes())
    }
    fn client(endpoint: &str) -> IamClient {
        IamClient::from_conf(
            aws_sdk_iam::config::Builder::new()
                .behavior_version(BehaviorVersion::latest())
                .region(aws_config::Region::new("us-east-1"))
                .credentials_provider(aws_credential_types::Credentials::new(
                    "AKIDEXAMPLE",
                    "dummy-local-secret",
                    None,
                    None,
                    "mfz-local-test",
                ))
                .endpoint_url(endpoint)
                .retry_config(aws_sdk_iam::config::retry::RetryConfig::disabled())
                .build(),
        )
    }
    fn options(path: &Path) -> RegistrationOptions<'_> {
        RegistrationOptions {
            profile: "selected",
            username: "local-test-user",
            account: "local-test-user",
            issuer: "AWS",
            out_png: None,
            overwrite_png: false,
            region: Some("us-east-1"),
            save_env: None,
            config: path,
        }
    }
    async fn actions(peer: HttpPeer) -> Vec<BTreeMap<String, String>> {
        peer.requests()
            .await
            .into_iter()
            .map(|request| {
                assert!(request.headers.contains("/us-east-1/iam/aws4_request"));
                assert!(!request
                    .headers
                    .to_lowercase()
                    .contains("x-amz-security-token:"));
                url::form_urlencoded::parse(&request.body)
                    .into_owned()
                    .collect()
            })
            .collect()
    }

    #[tokio::test]
    async fn cancelled_input_deletes_only_the_new_pending_device_through_the_real_sdk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        let peer = HttpPeer::new(vec![created(true), completed("DeleteVirtualMFADevice")]).await;
        let error = register_with_client(
            &client(&peer.url),
            &options(&path),
            |_, _| Ok(()),
            |_| Err(AppError::Iam("Input cancelled".into())),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Input cancelled"));
        let requests = actions(peer).await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["Action"], "CreateVirtualMFADevice");
        assert_eq!(requests[1]["Action"], "DeleteVirtualMFADevice");
        assert_eq!(requests[1]["SerialNumber"], SERIAL);
    }

    #[tokio::test]
    async fn missing_seed_and_failed_png_persistence_cleanup_pending_devices() {
        for missing_seed in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.ini");
            let png = directory.path().join("missing-parent").join("qr.png");
            let peer = HttpPeer::new(vec![
                created(!missing_seed),
                completed("DeleteVirtualMFADevice"),
            ])
            .await;
            let result = register_with_client(
                &client(&peer.url),
                &options(&path),
                |qr, _| qr::save_png(qr, png.to_str().unwrap()),
                |_| panic!("No OTP prompt is permitted after a seed/PNG failure"),
            )
            .await;
            assert!(result.is_err());
            let requests = actions(peer).await;
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[1]["Action"], "DeleteVirtualMFADevice");
            assert_eq!(requests[1]["SerialNumber"], SERIAL);
        }
    }

    #[tokio::test]
    async fn successful_activation_updates_the_existing_real_config_and_auth_type() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        std::fs::write(&path, BASIC).unwrap();
        let mut options = options(&path);
        options.save_env = Some("test");
        let mut codes = ["123456", "654321"].into_iter();
        let peer = HttpPeer::new(vec![created(true), completed("EnableMFADevice")]).await;
        register_with_client(
            &client(&peer.url),
            &options,
            |_, _| Ok(()),
            |_| Ok(codes.next().unwrap().into()),
        )
        .await
        .unwrap();
        let loaded = ConfigLoader::new(&path).load("test").unwrap();
        assert_eq!(loaded.serial.as_deref(), Some(SERIAL));
        assert_eq!(loaded.secret.as_deref(), Some("JBSWY3DPEHPK3PXP"));
        assert_eq!(loaded.auth_type, AuthType::Mfa);
        assert_eq!(loaded.instance.as_deref(), Some("i-test"));
        let requests = actions(peer).await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["Action"], "EnableMFADevice");
        assert_eq!(requests[1]["UserName"], "local-test-user");
        assert_eq!(requests[1]["AuthenticationCode1"], "123456");
        assert_eq!(requests[1]["AuthenticationCode2"], "654321");
    }

    #[tokio::test]
    async fn save_failure_after_activation_is_not_success_or_active_device_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        std::fs::write(&path, BASIC).unwrap();
        let mut options = options(&path);
        options.save_env = Some("test");
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut codes = ["123456", "654321"].into_iter();
        let peer = HttpPeer::new(vec![created(true), completed("EnableMFADevice")]).await;
        let error = register_with_client(
            &client(&peer.url),
            &options,
            |_, _| {
                let mut readonly = permissions.clone();
                readonly.set_readonly(true);
                std::fs::set_permissions(&path, readonly)?;
                Ok(())
            },
            |_| Ok(codes.next().unwrap().into()),
        )
        .await
        .unwrap_err();
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(error.to_string().contains("IS ENABLED"));
        assert!(error.to_string().contains("saving configuration failed"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), BASIC);
        let requests = actions(peer).await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["Action"], "EnableMFADevice");
    }

    #[tokio::test]
    async fn invalid_save_environment_fails_before_any_remote_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid.ini");
        std::fs::write(&path, "[test]\nprofile=selected\n").unwrap();
        let mut options = options(&path);
        options.save_env = Some("test");
        let peer = HttpPeer::new(vec![]).await;
        let error = register_with_client(
            &client(&peer.url),
            &options,
            |_, _| panic!("Invalid save destination must fail before presentation"),
            |_| panic!("No prompt expected"),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, AppError::Config(_)));
        assert!(actions(peer).await.is_empty());
    }
}

#[cfg(test)]
mod input_tests {
    #[tokio::test]
    async fn eof_prompt_returns_error_without_spinning() {
        const CHILD: &str = "MFZ_AUDIT_EOF_CHILD";
        if std::env::var_os(CHILD).is_some() {
            assert!(super::prompt_totp("MFA code (closed input fixture)").is_err());
            return;
        }
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("enrollment::input_tests::eof_prompt_returns_error_without_spinning")
            .arg("--nocapture")
            .env(CHILD, "1")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
            .await
            .expect("EOF must not retry forever")
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
