use aws_config::BehaviorVersion;
use aws_sdk_ec2::{types::Filter, Client as Ec2Client};

use crate::{
    error::{AppError, Result},
    mfa::MfaSessionManager,
    ui::Spinner,
};

pub async fn resolve_instance_by_tag(
    mfa: &mut MfaSessionManager,
    tag_key: &str,
    tag_value: &str,
    region: Option<&str>,
    spinner: Option<&Spinner>,
) -> Result<String> {
    if let Some(s) = spinner {
        s.set_message(format!(
            "Searching for instance with tag {}={}...",
            tag_key, tag_value
        ));
    }

    let creds = mfa.get_credentials(spinner).await?;

    let mut config_loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(r) = region {
        config_loader = config_loader.region(aws_config::Region::new(r.to_string()));
    }

    let config = config_loader.load().await;

    // We need to pass the credentials from MFA session to the EC2 client
    let ec2_config = aws_sdk_ec2::config::Builder::from(&config)
        .credentials_provider(aws_sdk_sts::config::Credentials::new(
            creds.access_key_id,
            creds.secret_access_key,
            Some(creds.session_token),
            Some(std::time::SystemTime::from(creds.expiration)),
            "mfz-ec2-resolution",
        ))
        .build();

    let client = Ec2Client::from_conf(ec2_config);

    let filter = Filter::builder()
        .name(format!("tag:{}", tag_key))
        .values(tag_value)
        .build();

    let state_filter = Filter::builder()
        .name("instance-state-name")
        .values("running")
        .build();

    let response = client
        .describe_instances()
        .filters(filter)
        .filters(state_filter)
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("DescribeInstances failed: {e}")))?;

    let instance_id = response
        .reservations()
        .iter()
        .flat_map(|r| r.instances())
        .next()
        .and_then(|i| i.instance_id())
        .ok_or_else(|| {
            AppError::Aws(format!(
                "No running instance found with tag {}={}",
                tag_key, tag_value
            ))
        })?;

    Ok(instance_id.to_string())
}
