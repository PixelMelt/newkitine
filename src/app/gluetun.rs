use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, info, warn};

use super::config::GluetunConfig;
use super::settings;
use super::state::App;

pub async fn fetch_forwarded_port(config: &GluetunConfig) -> Result<u16, String> {
    let body = http_get(config, "/v1/portforward").await?;
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

async fn http_get(config: &GluetunConfig, path: &str) -> Result<String, String> {
    let address = (config.host.as_str(), config.port);
    let mut stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(address))
        .await
        .map_err(|_| "connect timeout".to_owned())?
        .map_err(|error| format!("connect failed: {error}"))?;

    let mut request = format!("GET {path} HTTP/1.0\r\nHost: {}\r\n", config.host);
    if !config.api_key.is_empty() {
        request.push_str(&format!("X-API-Key: {}\r\n", config.api_key));
    } else if !config.username.is_empty() {
        let credentials = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", config.username, config.password));
        request.push_str(&format!("Authorization: Basic {credentials}\r\n"));
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("write failed: {error}"))?;

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut response))
        .await
        .map_err(|_| "read timeout".to_owned())?
        .map_err(|error| format!("read failed: {error}"))?;
    let response = String::from_utf8_lossy(&response);

    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed http response".to_owned())?;
    let status_line = head.lines().next().unwrap_or_default();
    if !status_line.contains("200") {
        return Err(format!("gluetun returned: {status_line}"));
    }
    Ok(body.to_owned())
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
