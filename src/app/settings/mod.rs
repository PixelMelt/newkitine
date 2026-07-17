pub use db::{load_settings, save_settings};
pub(in crate::app) use http::router;

mod db;
mod http;

use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::types::{
    DEFAULT_BANNED_MESSAGE, DEFAULT_MAX_SEARCH_RESPONSES, DEFAULT_MAX_SEARCH_RESULTS,
    DEFAULT_MIN_SEARCH_CHARS, DEFAULT_QUEUE_FILE_LIMIT, DEFAULT_SERVER, DEFAULT_UPLOAD_SLOTS,
    DEFAULT_UPLOADS_PER_USER, FilterLevel, LoginConfig, RuntimeConfig, SearchConfig, SharedFolder,
    TransferConfig, TransferDirection,
};

use super::contract::{AppEvent, SettingsPayload};
use super::session;
use super::state::App;

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub(in crate::app) fn apply_settings_env(settings: &mut Settings) -> Vec<&'static str> {
    let mut locked = Vec::new();
    if let Some(value) = env_var("NEWKITINE_SERVER") {
        settings.server = value;
        locked.push("server");
    }
    if let Some(value) = env_var("NEWKITINE_USERNAME") {
        settings.username = value;
        locked.push("username");
    }
    if let Some(value) = env_var("NEWKITINE_PASSWORD") {
        settings.password = value;
        locked.push("password");
    }
    if let Some(value) = env_var("NEWKITINE_LISTEN_PORT") {
        settings.listen_port = value
            .parse()
            .expect("NEWKITINE_LISTEN_PORT must be a port number");
        locked.push("listen_port");
    }
    if let Some(value) = env_var("NEWKITINE_DOWNLOAD_DIR") {
        settings.download_dir = PathBuf::from(value);
        locked.push("download_dir");
    }
    locked
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server: String,
    pub username: String,
    pub password: String,
    pub listen_port: u16,
    pub description: String,
    pub download_dir: PathBuf,
    pub incomplete_dir: Option<PathBuf>,
    pub shares: Vec<SharedFolder>,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub uploads_per_user: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub auto_reconnect: bool,
    pub theme: String,
    pub filter_level: FilterLevel,
    pub denied_message: String,
    pub max_search_results: usize,
    pub min_search_chars: usize,
    pub respond_to_searches: bool,
    pub max_search_responses: usize,
    pub rescan_daily: bool,
    pub rescan_hour_utc: u8,
    pub autoclear_downloads: bool,
    pub autoclear_uploads: bool,
    pub queue_size_limit_mb: u64,
    pub banned_message: String,
    pub download_username_subfolders: bool,
    pub share_filters: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.into(),
            username: String::new(),
            password: String::new(),
            listen_port: 2234,
            description: String::new(),
            download_dir: PathBuf::from("downloads"),
            incomplete_dir: None,
            shares: Vec::new(),
            upload_slots: DEFAULT_UPLOAD_SLOTS,
            queue_file_limit: DEFAULT_QUEUE_FILE_LIMIT,
            uploads_per_user: DEFAULT_UPLOADS_PER_USER,
            upload_limit_kbps: 0,
            download_limit_kbps: 0,
            auto_reconnect: true,
            theme: "dark".into(),
            filter_level: FilterLevel::Open,
            denied_message: "You need to share files to download from me.".into(),
            max_search_results: DEFAULT_MAX_SEARCH_RESULTS,
            min_search_chars: DEFAULT_MIN_SEARCH_CHARS,
            respond_to_searches: true,
            max_search_responses: DEFAULT_MAX_SEARCH_RESPONSES,
            rescan_daily: false,
            rescan_hour_utc: 0,
            autoclear_downloads: false,
            autoclear_uploads: false,
            queue_size_limit_mb: 0,
            banned_message: DEFAULT_BANNED_MESSAGE.into(),
            download_username_subfolders: false,
            share_filters: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSettings {
    pub server: String,
    pub username: String,
    pub password_set: bool,
    pub listen_port: u16,
    pub description: String,
    pub download_dir: PathBuf,
    pub incomplete_dir: Option<PathBuf>,
    pub shares: Vec<SharedFolder>,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub uploads_per_user: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub auto_reconnect: bool,
    pub theme: String,
    pub filter_level: FilterLevel,
    pub denied_message: String,
    pub max_search_results: usize,
    pub min_search_chars: usize,
    pub respond_to_searches: bool,
    pub max_search_responses: usize,
    pub rescan_daily: bool,
    pub rescan_hour_utc: u8,
    pub autoclear_downloads: bool,
    pub autoclear_uploads: bool,
    pub queue_size_limit_mb: u64,
    pub banned_message: String,
    pub download_username_subfolders: bool,
    pub share_filters: Vec<String>,
}

impl From<&Settings> for PublicSettings {
    fn from(settings: &Settings) -> Self {
        Self {
            server: settings.server.clone(),
            username: settings.username.clone(),
            password_set: !settings.password.is_empty(),
            listen_port: settings.listen_port,
            description: settings.description.clone(),
            download_dir: settings.download_dir.clone(),
            incomplete_dir: settings.incomplete_dir.clone(),
            shares: settings.shares.clone(),
            upload_slots: settings.upload_slots,
            queue_file_limit: settings.queue_file_limit,
            uploads_per_user: settings.uploads_per_user,
            upload_limit_kbps: settings.upload_limit_kbps,
            download_limit_kbps: settings.download_limit_kbps,
            auto_reconnect: settings.auto_reconnect,
            theme: settings.theme.clone(),
            filter_level: settings.filter_level,
            denied_message: settings.denied_message.clone(),
            max_search_results: settings.max_search_results,
            min_search_chars: settings.min_search_chars,
            respond_to_searches: settings.respond_to_searches,
            max_search_responses: settings.max_search_responses,
            rescan_daily: settings.rescan_daily,
            rescan_hour_utc: settings.rescan_hour_utc,
            autoclear_downloads: settings.autoclear_downloads,
            autoclear_uploads: settings.autoclear_uploads,
            queue_size_limit_mb: settings.queue_size_limit_mb,
            banned_message: settings.banned_message.clone(),
            download_username_subfolders: settings.download_username_subfolders,
            share_filters: settings.share_filters.clone(),
        }
    }
}

impl Settings {
    pub fn locked_differs(&self, other: &Settings, key: &'static str) -> bool {
        match key {
            "server" => self.server != other.server,
            "username" => self.username != other.username,
            "password" => self.password != other.password,
            "listen_port" => self.listen_port != other.listen_port,
            "download_dir" => self.download_dir != other.download_dir,
            key => unreachable!("unknown locked settings key {key}"),
        }
    }

    pub fn copy_locked_from(&mut self, source: &Settings, key: &'static str) {
        match key {
            "server" => self.server = source.server.clone(),
            "username" => self.username = source.username.clone(),
            "password" => self.password = source.password.clone(),
            "listen_port" => self.listen_port = source.listen_port,
            "download_dir" => self.download_dir = source.download_dir.clone(),
            key => unreachable!("unknown locked settings key {key}"),
        }
    }

    pub fn resolve_server(&self) -> Result<std::net::SocketAddr, String> {
        self.server
            .to_socket_addrs()
            .map_err(|error| format!("cannot resolve server address {}: {error}", self.server))?
            .next()
            .ok_or_else(|| format!("cannot resolve server address {}", self.server))
    }

    pub fn incomplete_dir(&self) -> PathBuf {
        self.incomplete_dir
            .clone()
            .unwrap_or_else(|| self.download_dir.join("incomplete"))
    }

    pub fn runtime_config(&self) -> Result<RuntimeConfig, String> {
        for share in &self.shares {
            if share.virtual_name.is_empty() || share.virtual_name.contains(['/', '\\']) {
                return Err(format!(
                    "share name {:?} is invalid: remote clients rewrite slashes in \
                     virtual paths and their download requests would no longer match",
                    share.virtual_name
                ));
            }
        }
        for filter in &self.share_filters {
            if filter.is_empty() || filter.contains(['/', '\\']) {
                return Err(format!(
                    "share filter {filter:?} is invalid: filters match exact file or \
                     folder names and cannot be empty or contain path separators"
                ));
            }
        }
        if self.max_search_responses == 0 || self.max_search_responses > 2000 {
            return Err(format!(
                "max_search_responses {} is invalid: retained results per search are \
                 bounded to 1-2000",
                self.max_search_responses
            ));
        }
        if self.max_search_results == 0 || self.max_search_results > DEFAULT_MAX_SEARCH_RESULTS {
            return Err(format!(
                "max_search_results {} is invalid: the protocol answer cap is \
                 1-{DEFAULT_MAX_SEARCH_RESULTS}",
                self.max_search_results
            ));
        }
        if self.rescan_hour_utc > 23 {
            return Err(format!(
                "rescan_hour_utc {} is invalid: hours run 0-23",
                self.rescan_hour_utc
            ));
        }
        if self.banned_message.is_empty() {
            return Err("banned_message must not be empty".into());
        }
        Ok(RuntimeConfig {
            login: LoginConfig {
                server: self.resolve_server()?,
                username: self.username.clone(),
                password: self.password.clone(),
                listen_port: self.listen_port,
            },
            description: self.description.clone(),
            auto_reconnect: self.auto_reconnect,
            transfers: TransferConfig {
                download_dir: self.download_dir.clone(),
                incomplete_dir: self.incomplete_dir(),
                upload_slots: self.upload_slots,
                queue_file_limit: self.queue_file_limit,
                uploads_per_user: self.uploads_per_user,
                upload_limit_kbps: self.upload_limit_kbps,
                download_limit_kbps: self.download_limit_kbps,
                queue_size_limit_mb: self.queue_size_limit_mb,
                banned_message: self.banned_message.clone(),
                download_username_subfolders: self.download_username_subfolders,
            },
            search: SearchConfig {
                max_search_results: self.max_search_results,
                min_search_chars: self.min_search_chars,
                respond_to_searches: self.respond_to_searches,
            },
            shared_folders: self.shares.clone(),
            share_filters: self.share_filters.clone(),
        })
    }
}

pub struct SettingsState {
    locked: Vec<&'static str>,
    gluetun_enabled: bool,
    mutation: tokio::sync::Mutex<()>,
    inner: RwLock<Inner>,
}

struct Inner {
    stored: Settings,
    live_port: Option<u16>,
}

struct SettingsChange {
    stored: Settings,
    effective: Settings,
    runtime: RuntimeConfig,
    behavior_changed: bool,
}

impl SettingsState {
    pub fn new(stored: Settings, locked: Vec<&'static str>, live_port: Option<u16>) -> Self {
        Self {
            locked,
            gluetun_enabled: live_port.is_some(),
            mutation: tokio::sync::Mutex::new(()),
            inner: RwLock::new(Inner { stored, live_port }),
        }
    }

    pub fn effective(&self) -> Settings {
        let inner = self.inner.read().unwrap();
        let mut settings = inner.stored.clone();
        apply_settings_env(&mut settings);
        if let Some(port) = inner.live_port {
            settings.listen_port = port;
        }
        settings
    }

    pub fn behavior_policy(&self) -> (FilterLevel, String) {
        let inner = self.inner.read().unwrap();
        (
            inner.stored.filter_level,
            inner.stored.denied_message.clone(),
        )
    }

    pub fn max_search_responses(&self) -> usize {
        self.inner.read().unwrap().stored.max_search_responses
    }

    pub fn autoclear(&self, direction: TransferDirection) -> bool {
        let inner = self.inner.read().unwrap();
        match direction {
            TransferDirection::Download => inner.stored.autoclear_downloads,
            TransferDirection::Upload => inner.stored.autoclear_uploads,
        }
    }

    pub fn payload(&self) -> SettingsPayload {
        SettingsPayload {
            settings: PublicSettings::from(&self.effective()),
            locked: self.locked.clone(),
            gluetun: self.gluetun_enabled,
        }
    }

    fn stage_update(&self, effective: Settings) -> Result<SettingsChange, String> {
        let old = self.effective();
        if let Some(key) = self
            .locked
            .iter()
            .find(|key| old.locked_differs(&effective, key))
        {
            return Err(format!("{key} is set by the environment"));
        }
        if self.gluetun_enabled && effective.listen_port != old.listen_port {
            return Err("listen_port is managed by gluetun".into());
        }
        let runtime = effective.runtime_config()?;
        let stored = {
            let inner = self.inner.read().unwrap();
            let mut stored = effective.clone();
            for key in &self.locked {
                stored.copy_locked_from(&inner.stored, key);
            }
            if self.gluetun_enabled {
                stored.listen_port = inner.stored.listen_port;
            }
            stored
        };
        let behavior_changed = effective.filter_level != old.filter_level
            || effective.denied_message != old.denied_message;
        Ok(SettingsChange {
            stored,
            effective,
            runtime,
            behavior_changed,
        })
    }

    fn commit(&self, stored: Settings) {
        self.inner.write().unwrap().stored = stored;
    }

    fn runtime_with_live_port(&self, port: u16) -> Result<RuntimeConfig, String> {
        let mut settings = self.effective();
        settings.listen_port = port;
        settings.runtime_config()
    }

    fn set_live_port(&self, port: u16) {
        self.inner.write().unwrap().live_port = Some(port);
    }
}

pub async fn apply_live_port(app: &Arc<App>, port: u16) -> Result<(), String> {
    let _mutation = app.settings.mutation.lock().await;
    let runtime = app.settings.runtime_with_live_port(port)?;
    app.client.apply_config(runtime).await;
    app.settings.set_live_port(port);
    session::set_listen_port(app, port);
    let mut data = app.projection.write();
    data.broadcast(AppEvent::Settings(app.settings.payload()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_round_trip() {
        let settings = Settings {
            username: "someone".into(),
            shares: vec![SharedFolder {
                virtual_name: "Music".into(),
                path: PathBuf::from("/data/shared"),
                buddy_only: true,
            }],
            theme: "catppuccin".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings, loaded);
    }

    #[test]
    fn share_names_with_slashes_are_rejected() {
        let with_share = |name: &str| Settings {
            server: "127.0.0.1:2242".into(),
            shares: vec![SharedFolder {
                virtual_name: name.into(),
                path: PathBuf::from("/data/shared"),
                buddy_only: false,
            }],
            ..Default::default()
        };
        for name in ["/", "a/b", "a\\b", ""] {
            assert!(
                with_share(name).runtime_config().is_err(),
                "{name:?} must be rejected"
            );
        }
        assert!(with_share("Music").runtime_config().is_ok());
    }

    #[test]
    fn invalid_settings_values_are_rejected() {
        let base = || Settings {
            server: "127.0.0.1:2242".into(),
            ..Default::default()
        };
        let invalid = [
            Settings {
                rescan_hour_utc: 24,
                ..base()
            },
            Settings {
                max_search_results: 0,
                ..base()
            },
            Settings {
                max_search_results: 301,
                ..base()
            },
            Settings {
                banned_message: String::new(),
                ..base()
            },
            Settings {
                share_filters: vec![String::new()],
                ..base()
            },
            Settings {
                share_filters: vec!["a/b".into()],
                ..base()
            },
            Settings {
                share_filters: vec!["a\\b".into()],
                ..base()
            },
        ];
        for settings in invalid {
            assert!(settings.runtime_config().is_err());
        }
        let valid = Settings {
            rescan_hour_utc: 23,
            share_filters: vec!["Thumbs.db".into(), "desktop.ini".into()],
            ..base()
        };
        assert!(valid.runtime_config().is_ok());
    }
}
