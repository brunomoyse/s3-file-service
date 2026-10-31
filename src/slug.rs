//! Validated product slugs.

use std::error::Error;
use std::fmt;

const MAX_LENGTH: usize = 128;

/// A product slug that is safe to embed in an S3 key.
///
/// Lowercase ASCII letters and digits in groups joined by single hyphens, up to 128 bytes.
/// These are the rules of `validateSlug` in pili-api, the service's only client.
#[derive(Debug)]
pub(crate) struct Slug(String);

impl TryFrom<String> for Slug {
    type Error = InvalidSlug;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() <= MAX_LENGTH && value.split('-').all(is_valid_group) {
            Ok(Self(value))
        } else {
            Err(InvalidSlug)
        }
    }
}

fn is_valid_group(group: &str) -> bool {
    !group.is_empty()
        && group
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

impl fmt::Display for Slug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug)]
pub(crate) struct InvalidSlug;

impl fmt::Display for InvalidSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid product slug")
    }
}

impl Error for InvalidSlug {}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_valid(slug: &str) -> bool {
        Slug::try_from(slug.to_owned()).is_ok()
    }

    #[test]
    fn accepts_lowercase_groups_joined_by_hyphens() {
        for slug in ["a", "sushi-saumon", "california-roll-8", "2-for-1"] {
            assert!(is_valid(slug), "{slug}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        let slugs = [
            "",
            "-",
            "-sushi",
            "sushi-",
            "sushi--saumon",
            "Sushi",
            "sushi saumon",
            "sushi_saumon",
            "sushi.png",
            "../sushi",
            "sushi/saumon",
            "sushi%2Fsaumon",
            "saumón",
        ];
        for slug in slugs {
            assert!(!is_valid(slug), "{slug}");
        }
    }

    #[test]
    fn limits_the_length() {
        assert!(is_valid(&"a".repeat(MAX_LENGTH)));
        assert!(!is_valid(&"a".repeat(MAX_LENGTH + 1)));
    }

    #[test]
    fn displays_the_slug_unchanged() {
        let slug = Slug::try_from("sushi-saumon".to_owned()).unwrap();

        assert_eq!(slug.to_string(), "sushi-saumon");
    }
}
