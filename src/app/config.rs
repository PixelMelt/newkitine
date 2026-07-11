use std::path::Path;

use serde::Deserialize;

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GluetunConfig {
    pub host: String,
    pub port: u16,
    pub api_key: String,
    pub username: String,
    pub password: String,
    pub poll_secs: u64,
}

impl Default for GluetunConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 8000,
            api_key: String::new(),
            username: String::new(),
            password: String::new(),
            poll_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Bootstrap {
    pub database_url: String,
    pub web_bind: String,
    pub geoip_db: Option<String>,
    pub gluetun: Option<GluetunConfig>,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            database_url: "mysql://newkitine:newkitine@localhost:3306/newkitine".into(),
            web_bind: "127.0.0.1:8080".into(),
            geoip_db: None,
            gluetun: None,
        }
    }
}

impl Bootstrap {
    pub fn load(path: &Path) -> Self {
        let mut bootstrap = if path.exists() {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            toml::from_str(&text)
                .unwrap_or_else(|error| panic!("invalid config {}: {error}", path.display()))
        } else {
            Self::default()
        };
        if let Some(value) = env_var("NEWKITINE_DB_URL") {
            bootstrap.database_url = value;
        }
        if let Some(value) = env_var("NEWKITINE_WEB_BIND") {
            bootstrap.web_bind = value;
        }
        if let Some(value) = env_var("NEWKITINE_GEOIP_DB") {
            bootstrap.geoip_db = Some(value);
        }
        if let Some(host) = env_var("GLUETUN_HOST") {
            let mut gluetun = bootstrap.gluetun.take().unwrap_or_default();
            gluetun.host = host;
            if let Some(port) = env_var("GLUETUN_PORT") {
                gluetun.port = port.parse().expect("GLUETUN_PORT must be a port number");
            }
            if let Some(key) = env_var("GLUETUN_APIKEY") {
                gluetun.api_key = key;
            }
            if let Some(user) = env_var("GLUETUN_USER") {
                gluetun.username = user;
            }
            if let Some(pass) = env_var("GLUETUN_PASS") {
                gluetun.password = pass;
            }
            bootstrap.gluetun = Some(gluetun);
        }
        bootstrap
    }
}
