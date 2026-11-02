//! The OAuth 2.0 Authorization Code + PKCE login, with a simulated browser.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use melodia::auth::{self, AuthError, OAuthConfig, wait_for_code};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tokio::net::TcpListener;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn query(url: &str) -> HashMap<String, String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

/// The action of the browser after the user accepts: follow the redirect.
async fn browser_redirect(redirect_uri: &str, params: &str) -> (u16, String) {
    let response = http()
        .get(format!("{redirect_uri}?{params}"))
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

#[tokio::test]
async fn login_exchanges_the_code_using_pkce() {
    let accounts = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "refresh-1",
            "scope": "user-read-playback-state user-modify-playback-state"
        })))
        .expect(1)
        .mount(&accounts)
        .await;

    let config = OAuthConfig {
        client_id: "my-client".into(),
        accounts_base: accounts.uri(),
        redirect_port: 0, // any free port
        bind_addr: LOCALHOST,
    };
    let (url_tx, url_rx) = tokio::sync::oneshot::channel::<String>();
    let browser = tokio::spawn(async move {
        let auth_url = url_rx.await.unwrap();
        let params = query(&auth_url);
        let page = browser_redirect(
            &params["redirect_uri"],
            &format!("code=the-code&state={}", params["state"]),
        )
        .await;
        (auth_url, page)
    });

    let session = auth::login(&http(), &config, |url| url_tx.send(url.to_owned()).unwrap())
        .await
        .unwrap();
    assert_eq!(session.access_token, "access-1");
    assert_eq!(session.refresh_token, "refresh-1");

    let (auth_url, (status, page)) = browser.await.unwrap();
    assert_eq!(status, 200);
    assert!(page.contains("All set!"));

    let params = query(&auth_url);
    assert!(auth_url.starts_with(&format!("{}/authorize?", accounts.uri())));
    assert_eq!(params["client_id"], "my-client");
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["code_challenge_method"], "S256");
    assert!(params["redirect_uri"].starts_with("http://127.0.0.1:"));

    // The token request proves that the client has the verifier of the challenge.
    let requests = accounts.received_requests().await.unwrap();
    let form: HashMap<String, String> = url::form_urlencoded::parse(&requests[0].body)
        .into_owned()
        .collect();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "the-code");
    assert_eq!(form["client_id"], "my-client");
    assert_eq!(form["redirect_uri"], params["redirect_uri"]);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes()));
    assert_eq!(challenge, params["code_challenge"]);
}

#[tokio::test]
async fn callback_server_ignores_other_requests_until_the_right_state_arrives() {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(wait_for_code(listener, "expected".into()));

    let base = redirect_uri.trim_end_matches("/callback");
    let favicon = http()
        .get(format!("{base}/favicon.ico"))
        .send()
        .await
        .unwrap();
    assert_eq!(favicon.status().as_u16(), 404);

    let (status, page) = browser_redirect(&redirect_uri, "code=old&state=another-attempt").await;
    assert_eq!(status, 400);
    assert!(page.contains("no longer valid"));
    assert!(!server.is_finished());

    let (status, _) = browser_redirect(&redirect_uri, "code=fresh&state=expected").await;
    assert_eq!(status, 200);
    assert_eq!(server.await.unwrap(), Ok("fresh".to_owned()));
}

#[tokio::test]
async fn denied_access_ends_the_login() {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(wait_for_code(listener, "s".into()));

    let (_, page) = browser_redirect(&redirect_uri, "error=access_denied&state=s").await;
    assert!(page.contains("you cancelled access"));
    assert_eq!(server.await.unwrap(), Err(AuthError::Denied));
}

#[tokio::test]
async fn rejected_code_is_reported() {
    let accounts = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Invalid authorization code"
        })))
        .mount(&accounts)
        .await;

    let result = auth::exchange_code(
        &http(),
        &accounts.uri(),
        "c",
        "bad",
        "http://127.0.0.1:1/callback",
        "v",
    )
    .await;
    assert_eq!(
        result,
        Err(AuthError::Exchange("Invalid authorization code".into()))
    );
}

#[tokio::test]
async fn busy_port_is_reported() {
    let taken = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let config = OAuthConfig {
        client_id: "c".into(),
        accounts_base: "http://127.0.0.1:1".into(),
        redirect_port: taken.local_addr().unwrap().port(),
        bind_addr: LOCALHOST,
    };
    let result = auth::login(&http(), &config, |_| panic!("no URL without a listener")).await;
    assert!(matches!(result, Err(AuthError::Bind { .. })), "{result:?}");
}
