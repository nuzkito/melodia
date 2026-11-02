//! New versions of the app: the manifest that the update server publishes (`web/version.json`:
//! the latest version and its download page), and the comparison of versions. Pure logic:
//! `backend/updates.rs` downloads it.

use std::cmp::Ordering;

use serde::Deserialize;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the update server publishes: the latest version and its download location.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// The download page, which «Download Melodia X.Y.Z» opens in the browser. It must be a page
    /// that [`is_trusted_download`] accepts.
    pub url: String,
}

/// A version newer than the version that runs, with its download location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
    pub url: String,
}

/// A version as `MAJOR.MINOR.PATCH` (missing numbers are 0), with an optional `v` at the start and
/// an optional pre-release (`1.2.0-beta.1`), which comes before the release. The parser ignores
/// build metadata (`+…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
    pre: Option<String>,
}

impl Version {
    /// `None` if `text` is not a version.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(version, _)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_owned())),
            Some(_) => return None,
            None => (text, None),
        };
        let mut numbers = [0; 3];
        let mut parts = core.split('.');
        for (i, part) in parts.by_ref().take(3).enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            numbers[i] = part.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Self { numbers, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_pre(a, b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The comparison of pre-releases goes dot by dot: numbers as numbers, and before words.
fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        let ordering = match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            },
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

/// The update that `manifest` tells about, if it is newer than `current`. If the version cannot be
/// read, or the download page is not a [trusted page](is_trusted_download), there is no update.
pub fn newer_than(manifest: Manifest, current: &str) -> Option<AvailableUpdate> {
    let latest = Version::parse(&manifest.version)?;
    let current = Version::parse(current)?;
    let url = manifest.url.trim();
    (latest > current && is_trusted_download(url)).then(|| AvailableUpdate {
        version: manifest.version.trim().to_owned(),
        url: url.to_owned(),
    })
}

/// The domain of the app's site. The download page can be on it or on one of its subdomains.
const SITE_DOMAIN: &str = "nuzkito.es";
/// The GitHub repository of the app, where the releases are.
const REPOSITORY_PATH: &str = "/nuzkito/melodia";

/// True if `url` is an `https` page of the app's site or of its GitHub repository. The app opens
/// the page without changes. Thus a manifest that an attacker changed cannot send the user to a
/// different site.
pub fn is_trusted_download(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    if url.scheme() != "https" || url.port().is_some() || !url.username().is_empty() {
        return false;
    }
    match url.host_str() {
        Some("github.com") => {
            let path = url.path().to_ascii_lowercase();
            path == REPOSITORY_PATH || path.starts_with(&format!("{REPOSITORY_PATH}/"))
        }
        Some(host) => {
            host == SITE_DOMAIN
                || host
                    .strip_suffix(SITE_DOMAIN)
                    .is_some_and(|sub| sub.ends_with('.'))
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} is a version"))
    }

    fn manifest(version: &str, url: &str) -> Manifest {
        Manifest {
            version: version.into(),
            url: url.into(),
        }
    }

    const URL: &str = "https://github.com/nuzkito/melodia/releases";

    #[test]
    fn versions_compare_number_by_number() {
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("1.2"), v("1.2.0"));
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3+build.7"), v("1.2.3"));
    }

    #[test]
    fn a_pre_release_goes_before_its_release() {
        assert!(v("1.0.0-beta") < v("1.0.0"));
        assert!(v("1.0.0-beta.2") < v("1.0.0-beta.10"));
        assert!(v("1.0.0-alpha") < v("1.0.0-beta"));
        assert!(v("1.0.0-beta") < v("1.0.0-beta.1"));
        assert!(v("1.0.0-rc.1") > v("0.9.0"));
    }

    #[test]
    fn text_that_is_not_a_version_is_rejected() {
        for text in [
            "", "latest", "1..2", "1.2.3.4", "1.x", "1.2-", "-1.2", "1.2 .3",
        ] {
            assert_eq!(Version::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn only_a_newer_version_is_announced() {
        assert_eq!(
            newer_than(manifest(" 0.2.0 ", &format!(" {URL} ")), "0.1.0"),
            Some(AvailableUpdate {
                version: "0.2.0".into(),
                url: URL.into(),
            })
        );
        assert_eq!(newer_than(manifest("0.1.0", URL), "0.1.0"), None);
        assert_eq!(newer_than(manifest("0.0.9", URL), "0.1.0"), None);
        assert_eq!(newer_than(manifest("soon", URL), "0.1.0"), None);
    }

    #[test]
    fn a_download_url_that_is_not_a_web_page_announces_nothing() {
        for url in [
            "",
            "  ",
            "example.com",
            "file:///C:/Windows/notepad.exe",
            "ms-settings:",
        ] {
            assert_eq!(newer_than(manifest("0.2.0", url), "0.1.0"), None, "{url:?}");
        }
        let local = manifest("0.2.0", "https://melodia.nuzkito.es/download");
        assert!(newer_than(local, "0.1.0").is_some());
    }

    #[test]
    fn only_pages_of_the_site_or_the_repository_are_trusted() {
        for url in [
            "https://nuzkito.es/",
            "https://melodia.nuzkito.es/download",
            "https://github.com/nuzkito/melodia",
            "https://github.com/nuzkito/melodia/releases/tag/v0.2.0",
            "https://github.com/Nuzkito/Melodia/releases",
        ] {
            assert!(is_trusted_download(url), "{url}");
        }
        for url in [
            "http://melodia.nuzkito.es/download",
            "https://melodia.nuzkito.es:8443/download",
            "https://evilnuzkito.es/",
            "https://nuzkito.es.example.com/",
            "https://user@melodia.nuzkito.es/",
            "https://github.com/nuzkito/melodia-fake/releases",
            "https://github.com/pablo/melodia/releases",
            "https://example.com/melodia",
        ] {
            assert!(!is_trusted_download(url), "{url}");
        }
    }

    #[test]
    fn the_manifest_needs_a_download_url() {
        assert!(serde_json::from_str::<Manifest>(r#"{"version": "0.2.0"}"#).is_err());
    }

    #[test]
    fn the_published_manifest_is_valid() {
        let manifest: Manifest = serde_json::from_str(include_str!("../web/version.json"))
            .expect("version.json is valid");
        assert!(Version::parse(&manifest.version).is_some());
        assert!(is_trusted_download(&manifest.url), "{}", manifest.url);
        assert!(Version::parse(CURRENT_VERSION).is_some());
    }
}
