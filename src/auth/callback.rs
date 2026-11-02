//! Minimal HTTP listener that receives the OAuth redirect on the loopback interface
//! (`http://127.0.0.1:PORT/callback`).

use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use url::Url;

use super::AuthError;

const MAX_REQUEST_BYTES: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The action for a request received on the redirect URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    /// It is not the OAuth callback (for example `/favicon.ico`): ignore it.
    Ignored,
    /// A callback for a different login attempt (incorrect `state`): continue to wait.
    Rejected,
    Code(String),
    /// Spotify reported an error (for example, the user refused access).
    Failed(AuthError),
}

/// Reads the request target (path + query) of a callback request.
pub fn parse_callback(target: &str, expected_state: &str) -> CallbackOutcome {
    let Ok(url) = Url::parse("http://127.0.0.1/").and_then(|base| base.join(target)) else {
        return CallbackOutcome::Ignored;
    };
    if url.path() != "/callback" {
        return CallbackOutcome::Ignored;
    }

    let (mut code, mut state, mut error) = (None, None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }

    if state.as_deref() != Some(expected_state) {
        return CallbackOutcome::Rejected;
    }
    if let Some(error) = error {
        return CallbackOutcome::Failed(if error == "access_denied" {
            AuthError::Denied
        } else {
            AuthError::Provider(error)
        });
    }
    match code {
        Some(code) if !code.is_empty() => CallbackOutcome::Code(code),
        _ => CallbackOutcome::Failed(AuthError::Provider("no code was received".into())),
    }
}

/// Serves the redirect URI until a request with the expected `state` arrives, and returns its
/// authorization code.
///
/// Each connection runs in its own task. Browsers open speculative connections that never send a
/// request, and they must not block the real connection.
pub async fn wait_for_code(
    listener: TcpListener,
    expected_state: String,
) -> Result<String, AuthError> {
    let (tx, mut rx) = mpsc::channel::<Result<String, AuthError>>(4);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|e| AuthError::Io(e.to_string()))?;
                let tx = tx.clone();
                let state = expected_state.clone();
                tokio::spawn(async move {
                    if let Some(result) = handle_connection(stream, &state).await {
                        let _ = tx.send(result).await;
                    }
                });
            }
            Some(result) = rx.recv() => return result,
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    expected_state: &str,
) -> Option<Result<String, AuthError>> {
    let target = read_request_target(&mut stream).await?;
    match parse_callback(&target, expected_state) {
        CallbackOutcome::Ignored => {
            respond(&mut stream, "404 Not Found", "").await;
            None
        }
        CallbackOutcome::Rejected => {
            let page = page(
                "Link expired",
                "This sign-in link is no longer valid. Go back to Melodia and click “Connect to Spotify”.",
                false,
            );
            respond(&mut stream, "400 Bad Request", &page).await;
            None
        }
        CallbackOutcome::Code(code) => {
            let page = page(
                "All set!",
                "Your Spotify account is connected. You can close this tab and go back to Melodia.",
                true,
            );
            respond(&mut stream, "200 OK", &page).await;
            Some(Ok(code))
        }
        CallbackOutcome::Failed(error) => {
            let message = format!("Could not connect the account: {error}.");
            respond(
                &mut stream,
                "200 OK",
                &page("Something went wrong", &message, false),
            )
            .await;
            Some(Err(error))
        }
    }
}

/// Reads the request head and returns the target of a `GET` request.
async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    // Read the full head. When a socket closes with unread data, some systems reset the
    // connection, and the browser does not see the page.
    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") && buffer.len() < MAX_REQUEST_BYTES {
        let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&buffer);
    let mut request_line = head.lines().next()?.split_whitespace();
    let (method, target) = (request_line.next()?, request_line.next()?);
    (method == "GET").then(|| target.to_owned())
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn page(title: &str, message: &str, success: bool) -> String {
    let (accent, symbol) = if success {
        ("#1ed760", "✓")
    } else {
        ("#ff6b6b", "!")
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Melodia · {title}</title>
<style>
  html,body{{height:100%;margin:0}}
  body{{display:grid;place-items:center;background:#0c0c0f;color:#f0f0f5;font:16px/1.5 "Segoe UI",system-ui,sans-serif}}
  main{{max-width:420px;padding:32px;text-align:center}}
  .badge{{width:56px;height:56px;margin:0 auto 20px;border-radius:50%;display:grid;place-items:center;
    background:{accent}22;color:{accent};font-size:28px;font-weight:600}}
  h1{{font-size:22px;font-weight:600;margin:0 0 8px}}
  p{{color:#a0a0ac;margin:0}}
</style></head>
<body><main><div class="badge">{symbol}</div><h1>{title}</h1><p>{message}</p></main></body></html>"#,
        title = escape_html(title),
        message = escape_html(message),
    )
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_code_when_state_matches() {
        assert_eq!(
            parse_callback("/callback?code=abc%2B1&state=xyz", "xyz"),
            CallbackOutcome::Code("abc+1".into())
        );
    }

    #[test]
    fn ignores_other_paths() {
        assert_eq!(
            parse_callback("/favicon.ico", "xyz"),
            CallbackOutcome::Ignored
        );
        assert_eq!(parse_callback("/", "xyz"), CallbackOutcome::Ignored);
    }

    #[test]
    fn rejects_callbacks_for_other_attempts() {
        assert_eq!(
            parse_callback("/callback?code=abc&state=old", "xyz"),
            CallbackOutcome::Rejected
        );
        assert_eq!(
            parse_callback("/callback?code=abc", "xyz"),
            CallbackOutcome::Rejected
        );
    }

    #[test]
    fn reports_denied_access_and_provider_errors() {
        assert_eq!(
            parse_callback("/callback?error=access_denied&state=xyz", "xyz"),
            CallbackOutcome::Failed(AuthError::Denied)
        );
        assert_eq!(
            parse_callback("/callback?error=server_error&state=xyz", "xyz"),
            CallbackOutcome::Failed(AuthError::Provider("server_error".into()))
        );
        assert!(matches!(
            parse_callback("/callback?state=xyz", "xyz"),
            CallbackOutcome::Failed(AuthError::Provider(_))
        ));
    }

    #[test]
    fn pages_escape_messages() {
        let html = page("Error", "<script>alert(1)</script>", false);
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
