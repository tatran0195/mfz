//! Control-plane integration uses the real SDK's HTTP serializer/signer and the
//! production SsmSession. Only the remote HTTP/WebSocket endpoints are local.
use super::*;
use aws_config::BehaviorVersion;
use aws_sdk_sts::config::Credentials as StsCredentials;
use chrono::{TimeDelta, Utc};
use serde_json::{json, Value};
use tokio::{net::TcpListener, time::timeout};

use super::super::tests::peer::{HttpPeer, Response};

struct ApiPeer(HttpPeer);
impl ApiPeer {
    async fn new(responses: Vec<Value>) -> Self {
        Self(
            HttpPeer::new(
                responses
                    .into_iter()
                    .map(|value| {
                        Response::new(
                            "application/x-amz-json-1.1",
                            serde_json::to_vec(&value).unwrap(),
                        )
                    })
                    .collect(),
            )
            .await,
        )
    }
    async fn requests(self) -> Vec<(String, Value)> {
        self.0
            .requests()
            .await
            .into_iter()
            .map(|request| {
                (
                    request.headers,
                    serde_json::from_slice(&request.body).unwrap(),
                )
            })
            .collect()
    }
}
impl std::ops::Deref for ApiPeer {
    type Target = HttpPeer;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
fn credentials() -> MfaCredentials {
    MfaCredentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "dummy-for-local-tests".into(),
        session_token: Some("dummy-sts-token".into()),
        expiration: Utc::now() + TimeDelta::hours(1),
    }
}
fn client(endpoint: &str) -> SsmClient {
    SsmClient::from_conf(
        aws_sdk_ssm::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(aws_sdk_ssm::config::Region::new("eu-west-1"))
            .credentials_provider(StsCredentials::new(
                "AKIDEXAMPLE",
                "dummy-for-local-tests",
                Some("dummy-sts-token".into()),
                None,
                "mfz-local-test",
            ))
            .endpoint_url(endpoint)
            .retry_config(aws_sdk_ssm::config::retry::RetryConfig::disabled())
            .build(),
    )
}
fn unused_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn assert_requests(requests: Vec<(String, Value)>) {
    assert_eq!(requests.len(), 2);
    assert!(requests[0].0.contains("AmazonSSM.StartSession"));
    assert!(requests[0].0.contains("/eu-west-1/ssm/aws4_request"));
    assert_eq!(
        requests[0].1["DocumentName"],
        "AWS-StartPortForwardingSession"
    );
    assert_eq!(requests[0].1["Target"], "i-local-test");
    assert!(requests[1].0.contains("AmazonSSM.TerminateSession"));
    assert_eq!(requests[1].1, json!({"SessionId":"local-test-session"}));
}
#[tokio::test]
async fn missing_start_response_fields_terminate_the_created_remote_session() {
    let api = ApiPeer::new(vec![
        json!({"SessionId":"local-test-session","TokenValue":"dummy"}),
        json!({"SessionId":"local-test-session"}),
    ])
    .await;
    let mut session = SsmSession::new("i-local-test", 3389, unused_port(), None);
    let error = session
        .start_with_client(client(&api.url), credentials(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no stream URL or token"));
    assert!(!session.is_running());
    assert_requests(api.requests().await);
}
#[tokio::test]
async fn failed_native_start_uses_sdk_termination_before_retry_or_return() {
    let api=ApiPeer::new(vec![json!({"SessionId":"local-test-session","TokenValue":"dummy","StreamUrl":"not a websocket URL"}),json!({"SessionId":"local-test-session"})]).await;
    let mut session = SsmSession::new("i-local-test", 3389, unused_port(), None);
    session
        .start_with_client(client(&api.url), credentials(), None)
        .await
        .unwrap();
    assert!(!session.wait_for_port(None).await);
    session.shutdown().await.unwrap();
    assert_requests(api.requests().await);
}
// tungstenite fixes the callback's ErrorResponse type.
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn websocket_signing_uses_the_sdk_resolved_region_not_a_hardcoded_fallback() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let stream_url = format!(
        "ws://{}/v1/data-channel/local-test-session",
        listener.local_addr().unwrap()
    );
    let peer = tokio::spawn(async move {
        let (tcp, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut ws = tokio_tungstenite::accept_hdr_async(
            tcp,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
             response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let authorization = request.headers()["authorization"].to_str().unwrap();
                assert!(authorization.contains("/eu-west-1/ssmmessages/aws4_request"));
                assert_eq!(request.headers()["x-amz-security-token"], "dummy-sts-token");
                Ok(response)
            },
        )
        .await
        .unwrap();
        // Deliberate pre-handshake remote close also tests automatic termination.
        ws.close(None).await.unwrap();
    });
    let api = ApiPeer::new(vec![
        json!({"SessionId":"local-test-session","TokenValue":"dummy","StreamUrl":stream_url}),
        json!({"SessionId":"local-test-session"}),
    ])
    .await;
    let mut session = SsmSession::new("i-local-test", 3389, unused_port(), None);
    session
        .start_with_client(client(&api.url), credentials(), None)
        .await
        .unwrap();
    assert!(!session.wait_for_port(None).await);
    // No manual stop before observing TerminateSession: transport failure alone
    // must trigger the real SDK cleanup request.
    assert_requests(api.requests().await);
    session.shutdown().await.unwrap();
    timeout(Duration::from_secs(5), peer)
        .await
        .unwrap()
        .unwrap();
}

// Run this exact test in an isolated process so AWS environment/profile fixtures
// cannot race the rest of the suite or touch a developer's real AWS files.
#[tokio::test]
async fn selected_profile_region_and_long_lived_token_option_reach_production_start() {
    const CHILD: &str = "MFZ_AUDIT_PROFILE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let mut mfa = MfaSessionManager::new(
            crate::mfa::AwsAuthSource::Profile("selected".into()),
            None,
            None,
            None,
        );
        let base = mfa.aws_config().await;
        assert_eq!(base.region().unwrap().as_ref(), "eu-west-1");
        assert!(
            std::ptr::eq(base, mfa.aws_config().await),
            "SDK provider chain must be shared, not reloaded per client"
        );
        let credentials = mfa.get_credentials(None).await.unwrap();
        assert_eq!(credentials.access_key_id, "AKIDSELECTED");
        assert!(credentials.session_token.is_none());
        assert!(credentials.to_sdk_credentials().session_token().is_none());
        let mut session = SsmSession::new("i-local-test", 3389, unused_port(), None);
        session.start(&mut mfa, None).await.unwrap();
        assert!(!session.wait_for_port(None).await);
        session.shutdown().await.unwrap();
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    let credentials = directory.path().join("credentials");
    std::fs::write(
        &config,
        "[default]\nregion=us-east-1\n[profile selected]\nregion=eu-west-1\n",
    )
    .unwrap();
    std::fs::write(&credentials, "[default]\naws_access_key_id=AKIDWRONGDEFAULT\naws_secret_access_key=dummy-default\n[selected]\naws_access_key_id=AKIDSELECTED\naws_secret_access_key=dummy-for-local-tests\n").unwrap();
    let api = ApiPeer::new(vec![json!({"SessionId":"local-test-session","TokenValue":"dummy","StreamUrl":"invalid websocket URL"}), json!({"SessionId":"local-test-session"})]).await;
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.arg("--exact").arg("ssm::sdk_tests::selected_profile_region_and_long_lived_token_option_reach_production_start").arg("--nocapture");
    for name in [
        "AWS_PROFILE",
        "AWS_DEFAULT_PROFILE",
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_ENDPOINT_URL_SSM",
        "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
        "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    ] {
        command.env_remove(name);
    }
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        command
            .env(CHILD, "1")
            .env("AWS_CONFIG_FILE", &config)
            .env("AWS_SHARED_CREDENTIALS_FILE", &credentials)
            .env("AWS_EC2_METADATA_DISABLED", "true")
            .env("AWS_ENDPOINT_URL", &api.url)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "isolated profile fixture failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = api.requests().await;
    for (headers, _) in &requests {
        assert!(headers.contains("Credential=AKIDSELECTED/"));
        assert!(!headers
            .to_ascii_lowercase()
            .contains("x-amz-security-token:"));
    }
    assert_requests(requests);
}
