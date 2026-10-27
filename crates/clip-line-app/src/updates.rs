//! Optional, anonymous check of the project's latest GitHub release.
use semver::Version;
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/katsugtgz/clip-line/releases/latest";

/// Returns the newer release's tag, if there is one. This does not download or
/// install anything. Call from a worker thread, never the UI thread.
pub fn check() -> anyhow::Result<Option<String>> {
    let response = ureq::get(LATEST)
        .set(
            "User-Agent",
            concat!("clip-line/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .call()?;
    let release: Release = response.into_json()?;
    newer_tag(&release.tag_name, env!("CARGO_PKG_VERSION"))
}

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
}

fn newer_tag(tag: &str, current: &str) -> anyhow::Result<Option<String>> {
    let release = Version::parse(tag.strip_prefix('v').unwrap_or(tag))?;
    let current = Version::parse(current)?;
    Ok((release > current).then(|| tag.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compares_versions_numerically_and_ignores_older_releases() {
        assert_eq!(
            newer_tag("v0.10.0", "0.9.0").unwrap(),
            Some("v0.10.0".into())
        );
        assert_eq!(newer_tag("v0.1.0", "0.1.0").unwrap(), None);
        assert_eq!(newer_tag("v0.1.0-rc.1", "0.1.0").unwrap(), None);
        assert!(newer_tag("invalid", "0.1.0").is_err());
    }
}
