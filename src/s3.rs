//! S3 client configuration and uploads.

use std::env;

use anyhow::Context;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use tracing::info;

/// Connection settings read from the `S3_*` environment variables.
pub(crate) struct Settings {
    access_key: String,
    secret_key: String,
    region: String,
    endpoint: Option<String>,
    force_path_style: bool,
}

impl Settings {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    /// Reads the settings through `lookup`. `S3_PROVIDER=ovh` selects the OVH defaults, and
    /// any other value selects AWS.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let access_key = lookup("S3_ACCESS_KEY_ID").context("S3_ACCESS_KEY_ID must be set")?;
        let secret_key =
            lookup("S3_SECRET_ACCESS_KEY").context("S3_SECRET_ACCESS_KEY must be set")?;
        let force_path_style = lookup("S3_FORCE_PATH_STYLE").map(|value| value == "true");
        let endpoint = lookup("S3_ENDPOINT_URL");

        let settings = if lookup("S3_PROVIDER").as_deref() == Some("ovh") {
            let region = lookup("S3_REGION").unwrap_or_else(|| "gra".to_owned());
            Self {
                access_key,
                secret_key,
                endpoint: Some(
                    endpoint.unwrap_or_else(|| format!("https://s3.{region}.io.cloud.ovh.net")),
                ),
                region,
                force_path_style: force_path_style.unwrap_or(true),
            }
        } else {
            Self {
                access_key,
                secret_key,
                endpoint,
                region: lookup("S3_REGION").unwrap_or_else(|| "eu-west-3".to_owned()),
                force_path_style: force_path_style.unwrap_or(false),
            }
        };
        Ok(settings)
    }

    pub(crate) fn into_client(self) -> Client {
        let mut config = aws_sdk_s3::Config::builder()
            .credentials_provider(Credentials::new(
                self.access_key,
                self.secret_key,
                None,
                None,
                "env",
            ))
            .region(Region::new(self.region))
            .force_path_style(self.force_path_style)
            .behavior_version_latest();
        config.set_endpoint_url(self.endpoint);
        Client::from_conf(config.build())
    }
}

pub(crate) async fn upload_file(
    client: &Client,
    bucket: &str,
    key: &str,
    data: Vec<u8>,
) -> anyhow::Result<()> {
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(data))
        .send()
        .await
        .with_context(|| format!("uploading {key}"))?;

    info!(key, "uploaded object");
    Ok(())
}

/// Deletes an object. S3 reports success whether or not the object existed.
pub(crate) async fn delete_file(client: &Client, bucket: &str, key: &str) -> anyhow::Result<()> {
    client
        .delete_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .with_context(|| format!("deleting {key}"))?;

    info!(key, "deleted object");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn settings_from(vars: &[(&str, &str)]) -> anyhow::Result<Settings> {
        let vars: HashMap<&str, &str> = vars.iter().copied().collect();
        Settings::from_lookup(|name| vars.get(name).map(ToString::to_string))
    }

    const CREDENTIALS: [(&str, &str); 2] = [
        ("S3_ACCESS_KEY_ID", "key"),
        ("S3_SECRET_ACCESS_KEY", "secret"),
    ];

    #[test]
    fn aws_is_the_default_provider() {
        let settings = settings_from(&CREDENTIALS).unwrap();

        assert_eq!(settings.region, "eu-west-3");
        assert_eq!(settings.endpoint, None);
        assert!(!settings.force_path_style);
    }

    #[test]
    fn an_unknown_provider_falls_back_to_aws() {
        let settings =
            settings_from(&[CREDENTIALS[0], CREDENTIALS[1], ("S3_PROVIDER", "minio")]).unwrap();

        assert_eq!(settings.region, "eu-west-3");
        assert_eq!(settings.endpoint, None);
    }

    #[test]
    fn ovh_derives_its_endpoint_from_the_region_and_forces_path_style() {
        let settings = settings_from(&[
            CREDENTIALS[0],
            CREDENTIALS[1],
            ("S3_PROVIDER", "ovh"),
            ("S3_REGION", "sbg"),
        ])
        .unwrap();

        assert_eq!(settings.region, "sbg");
        assert_eq!(
            settings.endpoint.as_deref(),
            Some("https://s3.sbg.io.cloud.ovh.net")
        );
        assert!(settings.force_path_style);
    }

    #[test]
    fn ovh_honours_explicit_overrides() {
        let settings = settings_from(&[
            CREDENTIALS[0],
            CREDENTIALS[1],
            ("S3_PROVIDER", "ovh"),
            ("S3_ENDPOINT_URL", "https://custom.example"),
            ("S3_FORCE_PATH_STYLE", "false"),
        ])
        .unwrap();

        assert_eq!(settings.region, "gra");
        assert_eq!(settings.endpoint.as_deref(), Some("https://custom.example"));
        assert!(!settings.force_path_style);
    }

    #[test]
    fn aws_honours_explicit_overrides() {
        let settings = settings_from(&[
            CREDENTIALS[0],
            CREDENTIALS[1],
            ("S3_REGION", "us-east-1"),
            ("S3_ENDPOINT_URL", "http://localhost:9000"),
            ("S3_FORCE_PATH_STYLE", "true"),
        ])
        .unwrap();

        assert_eq!(settings.region, "us-east-1");
        assert_eq!(settings.endpoint.as_deref(), Some("http://localhost:9000"));
        assert!(settings.force_path_style);
    }

    #[test]
    fn missing_credentials_name_the_variable() {
        let error = settings_from(&[CREDENTIALS[0]]).err().unwrap();

        assert!(
            error.to_string().contains("S3_SECRET_ACCESS_KEY"),
            "{error}"
        );
    }
}
