use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use super::config::GluetunConfig;
use super::settings;
use super::state::App;

pub async fn fetch_forwarded_port(config: &GluetunConfig) -> Result<u16, String> {
    let url = format!("http://{}:{}/v1/portforward", config.host, config.port);
    let mut request = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?
        .get(url);
    if !config.api_key.is_empty() {
        request = request.header("X-API-Key", &config.api_key);
    } else if !config.username.is_empty() {
        request = request.basic_auth(&config.username, Some(&config.password));
    }
    let body = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("request failed: {error}"))?
        .text()
        .await
        .map_err(|error| format!("read failed: {error}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| format!("invalid json: {error}"))?;
    let port = value
        .get("port")
        .and_then(|port| port.as_u64())
        .ok_or_else(|| format!("missing port in response: {body}"))?;
    if !(1024..65536).contains(&port) {
        return Err(format!("forwarded port out of range: {port}"));
    }
    Ok(port as u16)
}

const MAX_CONSECUTIVE_POLL_FAILURES: u32 = 30;

pub async fn watch(app: Arc<App>, config: GluetunConfig, mut current: u16) {
    let interval = Duration::from_secs(config.poll_secs.max(10));
    let mut failures = 0u32;
    loop {
        tokio::time::sleep(interval).await;
        match fetch_forwarded_port(&config).await {
            Ok(port) => {
                failures = 0;
                if current != port {
                    info!(port, "gluetun forwarded port changed, updating listen port");
                    match settings::apply_live_port(&app, port).await {
                        Ok(()) => current = port,
                        Err(error) => warn!(%error, "cannot apply forwarded port, will retry"),
                    }
                }
            }
            Err(error) => {
                failures += 1;
                if failures >= MAX_CONSECUTIVE_POLL_FAILURES {
                    error!(
                        %error,
                        failures,
                        "gluetun forwarded port has been unavailable for too long"
                    );
                    return;
                }
                warn!(%error, failures, "cannot fetch forwarded port from gluetun");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    async fn fetch_from(
        response: &'static str,
        config: GluetunConfig,
    ) -> (Result<u16, String>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = GluetunConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            ..config
        };
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).await.unwrap();
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request[..read]).to_lowercase()
        });
        (fetch_forwarded_port(&config).await, server.await.unwrap())
    }

    #[tokio::test]
    async fn the_forwarded_port_is_read_with_the_configured_credentials() {
        const OK: &str =
            "HTTP/1.1 200 OK\r\ncontent-length: 14\r\nconnection: close\r\n\r\n{\"port\":51234}";
        let mut config = GluetunConfig {
            api_key: "secret".into(),
            ..GluetunConfig::default()
        };
        let (port, request) = fetch_from(OK, config.clone()).await;
        assert_eq!(port, Ok(51234));
        assert!(request.starts_with("get /v1/portforward http/1.1"));
        assert!(request.contains("x-api-key: secret"));

        config = GluetunConfig {
            username: "user".into(),
            password: "pass".into(),
            ..GluetunConfig::default()
        };
        let (port, request) = fetch_from(OK, config).await;
        assert_eq!(port, Ok(51234));
        assert!(request.contains("authorization: basic dxnlcjpwyxnz"));

        const DENIED: &str =
            "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let (port, _) = fetch_from(DENIED, GluetunConfig::default()).await;
        assert!(port.unwrap_err().contains("401"));

        const LOW: &str =
            "HTTP/1.1 200 OK\r\ncontent-length: 10\r\nconnection: close\r\n\r\n{\"port\":0}";
        let (port, _) = fetch_from(LOW, GluetunConfig::default()).await;
        assert_eq!(port, Err("forwarded port out of range: 0".into()));
    }
}
