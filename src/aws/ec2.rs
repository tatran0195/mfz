use aws_sdk_ec2::{types::Filter, Client as Ec2Client};

use super::instance_selection::UniqueInstanceSelection;
use crate::{
    auth::MfaSessionManager,
    error::{AppError, Result},
    ui::Spinner,
};

pub async fn resolve_instance_by_tag(
    mfa: &mut MfaSessionManager,
    tag_key: &str,
    tag_value: &str,
    region: Option<&str>,
    spinner: Option<&Spinner>,
) -> Result<String> {
    if tag_key.trim().is_empty() || tag_value.is_empty() {
        return Err(AppError::Config(
            "Instance tag key/value must not be empty".into(),
        ));
    }
    if let Some(spinner) = spinner {
        spinner.set_message(format!(
            "Searching for a unique running instance tagged {tag_key}={tag_value}…"
        ));
    }
    let credentials = mfa.get_credentials(spinner).await?;
    let config = mfa.service_config(&credentials).await;
    let mut builder = aws_sdk_ec2::config::Builder::from(&config);
    if let Some(region) = region {
        builder = builder.region(aws_config::Region::new(region.to_owned()));
    }
    let client = Ec2Client::from_conf(builder.build());
    let tag = Filter::builder()
        .name(format!("tag:{tag_key}"))
        .values(tag_value)
        .build();
    let state = Filter::builder()
        .name("instance-state-name")
        .values("running")
        .build();
    let mut selection = UniqueInstanceSelection::default();
    let mut next_token = None;
    loop {
        let page = client
            .describe_instances()
            .filters(tag.clone())
            .filters(state.clone())
            .max_results(100)
            .set_next_token(next_token)
            .send()
            .await
            .map_err(|error| AppError::Aws(format!("DescribeInstances failed: {error}")))?;
        for instance in page
            .reservations()
            .iter()
            .flat_map(|reservation| reservation.instances())
        {
            let id = instance.instance_id().ok_or_else(|| {
                AppError::Aws("EC2 returned a running instance without an ID".into())
            })?;
            selection.observe(id)?;
        }
        next_token = selection.next_page(page.next_token())?;
        if next_token.is_none() {
            break;
        }
    }
    selection.finish()
}
