//! Checks one time, a moment after the app opens, if the update manifest
//! ([`crate::update::Manifest`]) tells about a newer version, and sends
//! [`Event::UpdateAvailable`]. If the check fails, it only logs the failure. It runs beside each
//! backend, because it is not related to Spotify.

use std::time::Duration;

use super::{Event, EventSink};
use crate::update::{CURRENT_VERSION, Manifest, newer_than};

/// The wait of the check after the start, so that it does not slow down the load of the app.
pub(super) const CHECK_DELAY: Duration = Duration::from_secs(2);

const TIMEOUT: Duration = Duration::from_secs(15);
/// The manifest is a few bytes. A larger file is not a manifest.
const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// The reason why a check failed.
#[derive(Debug, thiserror::Error)]
enum CheckError {
    #[error("could not reach the update server: {0}")]
    Request(#[from] reqwest::Error),
    #[error("the update server answered with status {0}")]
    Status(u16),
    #[error("the update manifest is too big")]
    TooBig,
    #[error("the update manifest is not valid: {0}")]
    Invalid(#[from] serde_json::Error),
}

/// Checks the manifest at `url` one time, after `delay`.
pub(super) async fn check(url: String, sink: EventSink, delay: Duration) {
    let client = match reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("Melodia/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            log::warn!("Could not create the HTTP client for the update check: {error}");
            return;
        }
    };
    tokio::time::sleep(delay).await;
    match fetch(&client, &url).await {
        Ok(manifest) => {
            log::debug!("Latest version: {}", manifest.version);
            if let Some(update) = newer_than(manifest, CURRENT_VERSION) {
                log::info!("Melodia {} is available", update.version);
                sink.send(Event::UpdateAvailable(update));
            }
        }
        Err(error) => log::info!("Could not check for updates at {url}: {error}"),
    }
}

/// Downloads and reads the manifest at `url`.
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Manifest, CheckError> {
    let response = client
        .get(url)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(CheckError::Status(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err(CheckError::TooBig);
    }
    let body = response.bytes().await?;
    if body.len() > MAX_MANIFEST_BYTES {
        return Err(CheckError::TooBig);
    }
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::channel;
    use crate::update::AvailableUpdate;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const URL: &str = "https://github.com/nuzkito/melodia/releases";

    async fn serve(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/version.json"))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    fn manifest_url(server: &MockServer) -> String {
        format!("{}/version.json", server.uri())
    }

    /// Checks the manifest at `url` and returns the updates that it tells about.
    async fn announced(url: String) -> Vec<AvailableUpdate> {
        let (handle, sink, _commands) = channel(|| {});
        check(url, sink, Duration::ZERO).await;
        std::iter::from_fn(|| handle.try_recv())
            .filter_map(|event| match event {
                Event::UpdateAvailable(update) => Some(update),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_newer_version_is_announced() {
        let server = serve(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "version": "99.0.0", "url": URL })),
        )
        .await;
        assert_eq!(
            announced(manifest_url(&server)).await,
            [AvailableUpdate {
                version: "99.0.0".into(),
                url: URL.into(),
            }]
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_same_version_is_not_announced() {
        let server = serve(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "version": CURRENT_VERSION, "url": URL })),
        )
        .await;
        assert!(announced(manifest_url(&server)).await.is_empty());
    }

    #[tokio::test]
    async fn a_failed_check_announces_nothing() {
        for response in [
            ResponseTemplate::new(404),
            ResponseTemplate::new(200).set_body_string("<html>not json</html>"),
            // Without a download page.
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "version": "99.0.0" })),
            ResponseTemplate::new(200).set_body_string(" ".repeat(MAX_MANIFEST_BYTES + 1)),
        ] {
            let server = serve(response).await;
            assert!(announced(manifest_url(&server)).await.is_empty());
        }
    }

    #[tokio::test]
    async fn an_unreachable_server_announces_nothing() {
        let server = MockServer::start().await;
        let url = manifest_url(&server);
        drop(server);
        assert!(announced(url).await.is_empty());
    }
}
