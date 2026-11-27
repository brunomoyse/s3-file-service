//! CloudFront invalidations, so a replaced or deleted image stops being served from the CDN.

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use aws_sdk_cloudfront::Client;
use aws_sdk_cloudfront::config::{Credentials, Region};
use aws_sdk_cloudfront::types::{InvalidationBatch, Paths};
use tracing::{error, info};

use crate::slug::Slug;

/// Invalidates the CDN paths of a product's images on one CloudFront distribution.
#[derive(Debug)]
pub(crate) struct Invalidator {
    client: Client,
    distribution_id: String,
}

impl Invalidator {
    /// Builds an invalidator when `CLOUDFRONT_DISTRIBUTION_ID` is set, or returns `None`.
    /// It signs with the `S3_*` access key, which then needs `cloudfront:CreateInvalidation`.
    pub(crate) fn from_env() -> anyhow::Result<Option<Self>> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Self>> {
        let Some(distribution_id) =
            lookup("CLOUDFRONT_DISTRIBUTION_ID").filter(|id| !id.is_empty())
        else {
            return Ok(None);
        };
        let access_key = lookup("S3_ACCESS_KEY_ID").context("S3_ACCESS_KEY_ID must be set")?;
        let secret_key =
            lookup("S3_SECRET_ACCESS_KEY").context("S3_SECRET_ACCESS_KEY must be set")?;
        let config = aws_sdk_cloudfront::Config::builder()
            .credentials_provider(Credentials::new(access_key, secret_key, None, None, "env"))
            // CloudFront is a global service signed in us-east-1.
            .region(Region::new("us-east-1"))
            .behavior_version_latest()
            .build();
        Ok(Some(Self::new(Client::from_conf(config), distribution_id)))
    }

    pub(crate) fn new(client: Client, distribution_id: String) -> Self {
        Self {
            client,
            distribution_id,
        }
    }

    /// Invalidates every variant of `slug`. A failure is logged, not returned: the objects are
    /// already stored or deleted, and the CDN catches up when its cached copies expire.
    pub(crate) async fn invalidate_product(&self, slug: &Slug) {
        if let Err(cause) = self.create_invalidation(slug).await {
            error!(%slug, "CloudFront invalidation failed: {cause:#}");
        }
    }

    async fn create_invalidation(&self, slug: &Slug) -> anyhow::Result<()> {
        let paths = product_paths(slug);
        let quantity = i32::try_from(paths.len()).context("too many invalidation paths")?;
        let batch = InvalidationBatch::builder()
            .paths(
                Paths::builder()
                    .quantity(quantity)
                    .set_items(Some(paths))
                    .build()
                    .context("building the invalidation paths")?,
            )
            .caller_reference(caller_reference(slug))
            .build()
            .context("building the invalidation batch")?;

        let output = self
            .client
            .create_invalidation()
            .distribution_id(&self.distribution_id)
            .invalidation_batch(batch)
            .send()
            .await
            .context("creating the invalidation")?;

        let id = output
            .invalidation()
            .map_or("", |invalidation| invalidation.id());
        info!(%slug, invalidation_id = id, "invalidated CDN paths");
        Ok(())
    }
}

/// The CDN paths of every variant of `slug`. A trailing wildcard covers the three formats in
/// one path, and CloudFront bills an invalidation per path.
fn product_paths(slug: &Slug) -> Vec<String> {
    vec![
        format!("/images/{slug}.*"),
        format!("/images/thumbnails/{slug}.*"),
    ]
}

/// A reference unique to this request, so CloudFront never takes it for a retry of an earlier
/// invalidation of the same slug. The counter keeps two calls within one clock tick apart.
fn caller_reference(slug: &Slug) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{slug}-{nanos}-{sequence}")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn from_vars(vars: &[(&str, &str)]) -> anyhow::Result<Option<Invalidator>> {
        let vars: HashMap<&str, &str> = vars.iter().copied().collect();
        Invalidator::from_lookup(|name| vars.get(name).map(ToString::to_string))
    }

    #[test]
    fn no_distribution_means_no_invalidator() {
        let none = from_vars(&[
            ("S3_ACCESS_KEY_ID", "key"),
            ("S3_SECRET_ACCESS_KEY", "secret"),
        ]);
        assert!(none.unwrap().is_none(), "no distribution id");

        let empty = from_vars(&[("CLOUDFRONT_DISTRIBUTION_ID", "")]);
        assert!(empty.unwrap().is_none(), "empty distribution id");
    }

    #[test]
    fn a_distribution_builds_an_invalidator() {
        let invalidator = from_vars(&[
            ("CLOUDFRONT_DISTRIBUTION_ID", "E123"),
            ("S3_ACCESS_KEY_ID", "key"),
            ("S3_SECRET_ACCESS_KEY", "secret"),
        ])
        .unwrap()
        .expect("an invalidator");

        assert_eq!(invalidator.distribution_id, "E123");
    }

    #[test]
    fn a_distribution_without_credentials_names_the_variable() {
        let error = from_vars(&[("CLOUDFRONT_DISTRIBUTION_ID", "E123")]).expect_err("an error");

        assert!(error.to_string().contains("S3_ACCESS_KEY_ID"), "{error}");
    }

    #[test]
    fn paths_cover_every_variant_of_the_slug() {
        let slug = Slug::try_from("my-product".to_owned()).unwrap();

        assert_eq!(
            product_paths(&slug),
            ["/images/my-product.*", "/images/thumbnails/my-product.*"]
        );
    }

    #[test]
    fn caller_references_differ_between_calls() {
        let slug = Slug::try_from("my-product".to_owned()).unwrap();

        assert_ne!(
            caller_reference(&slug),
            caller_reference(&slug),
            "unique per call"
        );
    }
}
