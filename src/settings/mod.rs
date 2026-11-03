pub use db::{load_settings, save_settings};
pub(crate) use http::router;

mod db;
mod http;

use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use soulseek::storage::{
    DownloadStore, LocalDownloads, LocalShares, ScanMode, ShareFilters, ShareSource, SharedFolder,
};
use soulseek::types::{
    DEFAULT_BANNED_MESSAGE, DEFAULT_MAX_SEARCH_RESULTS, DEFAULT_MIN_SEARCH_CHARS,
    DEFAULT_QUEUE_FILE_LIMIT, DEFAULT_SERVER, DEFAULT_UPLOAD_SLOTS, LoginConfig, RuntimeConfig,
    SearchConfig, TransferConfig, TransferDirection,
};

use super::behavior::{DenialMessages, FilterLevel};
use super::config::env_var;
use super::contract::{AppEvent, SettingsPayload};
use super::profile::DescriptionTemplate;
use super::pushover::Keys;
use super::session;
use super::state::App;
use super::storage::{WebDav, WebDavDownloads, WebDavShare, WebDavShares};

pub(crate) fn apply_settings_env(settings: &mut Settings) -> Vec<&'static str> {
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
        settings.download_dir = value;
        locked.push("download_dir");
    }
    locked
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Storage {
    Local,
    WebDav,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Share {
    pub virtual_name: String,
    pub path: String,
    #[serde(default)]
    pub buddy_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub server: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    pub listen_port: u16,
    pub description: String,
    pub download_storage: Storage,
    pub download_dir: String,
    pub incomplete_dir: Option<String>,
    pub share_storage: Storage,
    pub fast_scan: bool,
    pub shares: Vec<Share>,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub auto_reconnect: bool,
    pub theme: String,
    pub filter_level: FilterLevel,
    pub abusive_message: String,
    pub leech_message: String,
    pub clear_verdict_on_message: bool,
    pub max_search_results: usize,
    pub min_search_chars: usize,
    pub respond_to_searches: bool,
    pub max_search_responses: usize,
    pub scan_on_startup: bool,
    pub rescan_daily: bool,
    pub rescan_hour_utc: u8,
    pub autoclear_downloads: bool,
    pub autoclear_uploads: bool,
    pub queue_size_limit_mb: u64,
    pub banned_message: String,
    pub download_username_subfolders: bool,
    pub share_filters: Vec<String>,
    pub pushover_token: String,
    pub pushover_user_key: String,
    pub pushover_private_messages: bool,
}

const DEFAULT_MAX_SEARCH_RESPONSES: usize = 500;
const PATH_LIMIT: usize = 4096;
const NAME_LIMIT: usize = 255;
const MESSAGE_LIMIT: usize = 1024;

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.into(),
            username: String::new(),
            password: String::new(),
            listen_port: 2234,
            description: String::new(),
            download_storage: Storage::Local,
            download_dir: "downloads".into(),
            incomplete_dir: None,
            share_storage: Storage::Local,
            fast_scan: false,
            shares: Vec::new(),
            upload_slots: DEFAULT_UPLOAD_SLOTS,
            queue_file_limit: DEFAULT_QUEUE_FILE_LIMIT,
            upload_limit_kbps: 0,
            download_limit_kbps: 0,
            auto_reconnect: true,
            theme: "dark".into(),
            filter_level: FilterLevel::Open,
            abusive_message: "Your client is flooding me with requests.".into(),
            leech_message: "You need to share files to download from me.".into(),
            clear_verdict_on_message: true,
            max_search_results: DEFAULT_MAX_SEARCH_RESULTS,
            min_search_chars: DEFAULT_MIN_SEARCH_CHARS,
            respond_to_searches: true,
            max_search_responses: DEFAULT_MAX_SEARCH_RESPONSES,
            scan_on_startup: false,
            rescan_daily: false,
            rescan_hour_utc: 0,
            autoclear_downloads: false,
            autoclear_uploads: false,
            queue_size_limit_mb: 0,
            banned_message: DEFAULT_BANNED_MESSAGE.into(),
            download_username_subfolders: false,
            share_filters: Vec::new(),
            pushover_token: String::new(),
            pushover_user_key: String::new(),
            pushover_private_messages: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSettings {
    password_set: bool,
    #[serde(flatten)]
    settings: Settings,
}

impl From<Settings> for PublicSettings {
    fn from(mut settings: Settings) -> Self {
        Self {
            password_set: !std::mem::take(&mut settings.password).is_empty(),
            settings,
        }
    }
}

impl Settings {
    pub fn from_stored(data: &str) -> serde_json::Result<Self> {
        let mut merged = serde_json::to_value(Self::default())?;
        let stored: serde_json::Map<String, serde_json::Value> = serde_json::from_str(data)?;
        merged.as_object_mut().unwrap().extend(stored);
        serde_json::from_value(merged)
    }

    fn locked_differs(&self, other: &Settings, key: &'static str) -> bool {
        let mut copy = self.clone();
        copy.copy_locked_from(other, key);
        copy != *self
    }

    fn copy_locked_from(&mut self, source: &Settings, key: &'static str) {
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

    fn incomplete_dir(&self) -> String {
        self.incomplete_dir.clone().unwrap_or_else(|| {
            let dir = &self.download_dir;
            let separator = if dir.is_empty() || dir.ends_with('/') {
                ""
            } else {
                "/"
            };
            format!("{dir}{separator}incomplete")
        })
    }

    pub fn runtime_config(&self) -> Result<RuntimeConfig, String> {
        if self.max_search_responses == 0 || self.max_search_responses > 2000 {
            return Err(format!(
                "max_search_responses {} is invalid: retained results per search are \
                 bounded to 1-2000",
                self.max_search_responses
            ));
        }
        if self.rescan_hour_utc > 23 {
            return Err(format!(
                "rescan_hour_utc {} is invalid: hours run 0-23",
                self.rescan_hour_utc
            ));
        }
        let paths = [Some(&self.download_dir), self.incomplete_dir.as_ref()];
        if paths.into_iter().flatten().any(|path| path.contains('\0')) {
            return Err("download_dir and incomplete_dir must not contain a NUL byte".into());
        }
        if paths
            .into_iter()
            .flatten()
            .any(|path| path.len() > PATH_LIMIT)
        {
            return Err(format!(
                "download_dir and incomplete_dir are bounded to {PATH_LIMIT} bytes"
            ));
        }
        if self
            .shares
            .iter()
            .any(|share| share.virtual_name.len() > NAME_LIMIT)
        {
            return Err(format!("share names are bounded to {NAME_LIMIT} bytes"));
        }
        let messages = [
            &self.banned_message,
            &self.abusive_message,
            &self.leech_message,
        ];
        if messages
            .into_iter()
            .any(|message| message.chars().count() > MESSAGE_LIMIT)
        {
            return Err(format!(
                "banned_message, abusive_message and leech_message are bounded to \
                 {MESSAGE_LIMIT} characters"
            ));
        }
        if self.pushover_private_messages
            && (self.pushover_token.is_empty() || self.pushover_user_key.is_empty())
        {
            return Err(
                "pushover_private_messages requires both pushover_token and \
                 pushover_user_key to be set"
                    .into(),
            );
        }
        let config = RuntimeConfig {
            login: LoginConfig {
                server: self.resolve_server()?,
                username: self.username.clone(),
                password: self.password.clone(),
                listen_port: self.listen_port,
            },
            auto_reconnect: self.auto_reconnect,
            transfers: TransferConfig {
                upload_slots: self.upload_slots,
                queue_file_limit: self.queue_file_limit,
                upload_limit_kbps: self.upload_limit_kbps,
                download_limit_kbps: self.download_limit_kbps,
                queue_size_limit_mb: self.queue_size_limit_mb,
                banned_message: self.banned_message.clone(),
            },
            search: SearchConfig {
                max_search_results: self.max_search_results,
                min_search_chars: self.min_search_chars,
                respond_to_searches: self.respond_to_searches,
            },
        };
        config.validate()?;
        Ok(config)
    }

    pub fn description_template(&self) -> Result<DescriptionTemplate, String> {
        DescriptionTemplate::parse(&self.description)
    }

    fn shares_differ(&self, other: &Settings) -> bool {
        self.share_storage != other.share_storage
            || self.fast_scan != other.fast_scan
            || self.shares != other.shares
            || self.share_filters != other.share_filters
    }

    fn downloads_differ(&self, other: &Settings) -> bool {
        self.download_storage != other.download_storage
            || self.download_dir != other.download_dir
            || self.incomplete_dir() != other.incomplete_dir()
            || self.download_username_subfolders != other.download_username_subfolders
    }

    fn check_download_switch(&self, old: &Settings, unfinished: usize) -> Result<(), String> {
        if self.download_storage == old.download_storage || unfinished == 0 {
            return Ok(());
        }
        let noun = if unfinished == 1 {
            "download"
        } else {
            "downloads"
        };
        Err(format!(
            "Finish or clear the {unfinished} unfinished {noun} before switching download storage"
        ))
    }
}

pub struct SettingsState {
    locked: Vec<&'static str>,
    gluetun_enabled: bool,
    share_catalog: PathBuf,
    webdav: Option<WebDav>,
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
    description: DescriptionTemplate,
    shares: Option<Arc<dyn ShareSource>>,
    downloads: Option<Arc<dyn DownloadStore>>,
    behavior_changed: bool,
}

impl SettingsState {
    pub fn new(
        stored: Settings,
        locked: Vec<&'static str>,
        live_port: Option<u16>,
        share_catalog: PathBuf,
        webdav: Option<WebDav>,
    ) -> Self {
        Self {
            locked,
            gluetun_enabled: live_port.is_some(),
            share_catalog,
            webdav,
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

    pub fn clear_verdict_on_message(&self) -> bool {
        self.inner.read().unwrap().stored.clear_verdict_on_message
    }

    pub fn behavior_policy(&self) -> (FilterLevel, DenialMessages) {
        let inner = self.inner.read().unwrap();
        (
            inner.stored.filter_level,
            DenialMessages {
                abusive: inner.stored.abusive_message.clone(),
                leech: inner.stored.leech_message.clone(),
            },
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

    pub fn pushover_keys(&self) -> Option<Keys> {
        let inner = self.inner.read().unwrap();
        let set =
            !inner.stored.pushover_token.is_empty() && !inner.stored.pushover_user_key.is_empty();
        set.then(|| Keys {
            token: inner.stored.pushover_token.clone(),
            user_key: inner.stored.pushover_user_key.clone(),
        })
    }

    pub fn pushover_private_messages(&self) -> Option<Keys> {
        let inner = self.inner.read().unwrap();
        inner.stored.pushover_private_messages.then(|| Keys {
            token: inner.stored.pushover_token.clone(),
            user_key: inner.stored.pushover_user_key.clone(),
        })
    }

    pub fn payload(&self) -> SettingsPayload {
        SettingsPayload {
            settings: self.effective().into(),
            locked: self.locked.clone(),
            gluetun: self.gluetun_enabled,
            webdav: self.webdav.is_some(),
        }
    }

    fn webdav_for(&self, setting: &str) -> Result<WebDav, String> {
        self.webdav.clone().ok_or_else(|| {
            format!(
                "{setting} is webdav but no WebDAV server is configured: add a [webdav] section \
                 with url, username and password to the config file, or set \
                 NEWKITINE_WEBDAV_URL, NEWKITINE_WEBDAV_USERNAME and NEWKITINE_WEBDAV_PASSWORD"
            )
        })
    }

    pub fn share_source(&self, settings: &Settings) -> Result<Arc<dyn ShareSource>, String> {
        let filters = ShareFilters::new(settings.share_filters.clone())?;
        match settings.share_storage {
            Storage::Local => {
                let folders = settings
                    .shares
                    .iter()
                    .map(|share| SharedFolder {
                        virtual_name: share.virtual_name.clone(),
                        path: PathBuf::from(&share.path),
                        buddy_only: share.buddy_only,
                    })
                    .collect();
                let mode = if settings.fast_scan {
                    ScanMode::Fast
                } else {
                    ScanMode::Full
                };
                let shares = LocalShares::new(folders, filters, self.share_catalog.clone(), mode)?;
                Ok(Arc::new(shares))
            }
            Storage::WebDav => {
                let client = self.webdav_for("share_storage")?;
                if !settings.fast_scan {
                    return Err(
                        "share_storage webdav needs fast_scan: WebDAV shares are listed \
                         without reading their files"
                            .into(),
                    );
                }
                let shares = settings
                    .shares
                    .iter()
                    .map(|share| WebDavShare {
                        virtual_name: share.virtual_name.clone(),
                        path: share.path.clone(),
                        buddy_only: share.buddy_only,
                    })
                    .collect();
                Ok(Arc::new(WebDavShares::new(client, shares, filters)?))
            }
        }
    }

    pub fn download_store(&self, settings: &Settings) -> Result<Arc<dyn DownloadStore>, String> {
        let incomplete_dir = settings.incomplete_dir();
        let subfolders = settings.download_username_subfolders;
        match settings.download_storage {
            Storage::Local => Ok(Arc::new(LocalDownloads::new(
                PathBuf::from(&settings.download_dir),
                PathBuf::from(incomplete_dir),
                subfolders,
            )?)),
            Storage::WebDav => Ok(Arc::new(WebDavDownloads::new(
                self.webdav_for("download_storage")?,
                settings.download_dir.clone(),
                incomplete_dir,
                subfolders,
            )?)),
        }
    }

    fn stage_update(
        &self,
        effective: Settings,
        unfinished_downloads: usize,
    ) -> Result<SettingsChange, String> {
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
        effective.check_download_switch(&old, unfinished_downloads)?;
        let runtime = effective.runtime_config()?;
        let description = effective.description_template()?;
        let shares = self.share_source(&effective)?;
        let downloads = self.download_store(&effective)?;
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
            || effective.abusive_message != old.abusive_message
            || effective.leech_message != old.leech_message;
        Ok(SettingsChange {
            shares: effective.shares_differ(&old).then_some(shares),
            downloads: effective.downloads_differ(&old).then_some(downloads),
            stored,
            effective,
            runtime,
            description,
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
    data.broadcast(AppEvent::Settings(Box::new(app.settings.payload())));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNCONFIGURED: &str = "is webdav but no WebDAV server is configured: add a [webdav] \
                                section with url, username and password to the config file, \
                                or set NEWKITINE_WEBDAV_URL, NEWKITINE_WEBDAV_USERNAME and \
                                NEWKITINE_WEBDAV_PASSWORD";

    fn state(webdav: Option<WebDav>) -> SettingsState {
        SettingsState::new(
            Settings {
                server: "127.0.0.1:2242".into(),
                ..Default::default()
            },
            Vec::new(),
            None,
            PathBuf::from("share-catalog.gz"),
            webdav,
        )
    }

    fn dav() -> Option<WebDav> {
        Some(WebDav::new("https://dav.example/remote.php/dav/", "u", "p").unwrap())
    }

    fn share(virtual_name: &str, path: &str, buddy_only: bool) -> Share {
        Share {
            virtual_name: virtual_name.into(),
            path: path.into(),
            buddy_only,
        }
    }

    #[test]
    fn settings_json_round_trip() {
        let settings = Settings {
            username: "someone".into(),
            password: "secret".into(),
            share_storage: Storage::WebDav,
            fast_scan: true,
            shares: vec![share("Music", "/data/shared", true)],
            download_storage: Storage::WebDav,
            download_dir: "/remote/downloads".into(),
            incomplete_dir: Some("/remote/partials".into()),
            theme: "catppuccin".into(),
            description: "first line\nsecond line\n\nfourth".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(settings, Settings::from_stored(&json).unwrap());
        let stored: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(stored["share_storage"], "webdav");
        assert_eq!(stored["download_storage"], "webdav");
        assert_eq!(stored["fast_scan"], true);
        assert_eq!(
            stored["shares"],
            serde_json::json!([{ "virtual_name": "Music", "path": "/data/shared", "buddy_only": true }])
        );
        assert_eq!(stored["download_dir"], "/remote/downloads");
        assert_eq!(stored["incomplete_dir"], "/remote/partials");
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(defaults["share_storage"], "local");
        assert_eq!(defaults["download_storage"], "local");
        assert_eq!(defaults["fast_scan"], false);
        let public = serde_json::to_value(PublicSettings::from(settings)).unwrap();
        assert_eq!(public["password_set"], true);
        assert!(public.get("password").is_none());
    }

    #[test]
    fn stored_settings_without_the_new_messages_still_load() {
        let loaded = Settings::from_stored(r#"{"username":"someone"}"#).unwrap();
        assert_eq!(loaded.username, "someone");
        assert_eq!(loaded.abusive_message, Settings::default().abusive_message);
        assert_eq!(loaded.leech_message, Settings::default().leech_message);
    }

    #[test]
    fn stored_settings_from_before_the_storage_choice_load_as_local() {
        let loaded = Settings::from_stored(
            r#"{"download_dir":"/data/downloads","incomplete_dir":null,
                "shares":[{"virtual_name":"Music","path":"/data/shared"}]}"#,
        )
        .unwrap();
        assert_eq!(loaded.share_storage, Storage::Local);
        assert_eq!(loaded.download_storage, Storage::Local);
        assert!(!loaded.fast_scan);
        assert_eq!(loaded.download_dir, "/data/downloads");
        assert_eq!(loaded.incomplete_dir, None);
        assert_eq!(loaded.shares, vec![share("Music", "/data/shared", false)]);
    }

    #[test]
    fn the_incomplete_dir_defaults_inside_the_download_dir() {
        let with = |download_dir: &str, incomplete_dir: Option<&str>| Settings {
            download_dir: download_dir.into(),
            incomplete_dir: incomplete_dir.map(Into::into),
            ..Default::default()
        };
        assert_eq!(
            with("downloads", None).incomplete_dir(),
            "downloads/incomplete"
        );
        assert_eq!(with("", None).incomplete_dir(), "incomplete");
        assert_eq!(with("/", None).incomplete_dir(), "/incomplete");
        assert_eq!(
            with("/remote/", None).incomplete_dir(),
            "/remote/incomplete"
        );
        assert_eq!(
            with("/remote", Some("/partials")).incomplete_dir(),
            "/partials"
        );
    }

    #[test]
    fn share_sources_follow_the_chosen_storage() {
        let with = |share_storage, fast_scan, shared: Share, filter: &str| Settings {
            share_storage,
            fast_scan,
            shares: vec![shared],
            share_filters: vec![filter.into()],
            ..Default::default()
        };
        let music = || share("Music", "/data/shared", false);
        let configured = state(dav());
        for (storage, fast_scan) in [
            (Storage::Local, false),
            (Storage::Local, true),
            (Storage::WebDav, true),
        ] {
            let settings = with(storage, fast_scan, music(), "Thumbs.db");
            assert!(configured.share_source(&settings).is_ok());
        }
        for storage in [Storage::Local, Storage::WebDav] {
            let bad_name = with(
                storage,
                true,
                share("a/b", "/data/shared", false),
                "Thumbs.db",
            );
            assert_eq!(
                configured.share_source(&bad_name).err().unwrap(),
                "share name \"a/b\" is invalid: remote clients rewrite slashes in virtual \
                 paths and their download requests would no longer match"
            );
            let bad_filter = with(storage, true, music(), "a/b");
            assert_eq!(
                configured.share_source(&bad_filter).err().unwrap(),
                "share filter \"a/b\" is invalid: filters match exact file or folder names \
                 and cannot be empty or contain path separators"
            );
        }
        let relative = with(
            Storage::WebDav,
            true,
            share("Music", "data", false),
            "Thumbs.db",
        );
        assert_eq!(
            configured.share_source(&relative).err().unwrap(),
            "remote path \"data\" is invalid: it must start with / and have no empty, . or .. \
             segments"
        );
        let full_scan = with(Storage::WebDav, false, music(), "Thumbs.db");
        assert_eq!(
            configured.share_source(&full_scan).err().unwrap(),
            "share_storage webdav needs fast_scan: WebDAV shares are listed without reading \
             their files"
        );
        let unconfigured = with(Storage::WebDav, true, music(), "Thumbs.db");
        assert_eq!(
            state(None).share_source(&unconfigured).err().unwrap(),
            format!("share_storage {UNCONFIGURED}")
        );
    }

    #[test]
    fn download_stores_follow_the_chosen_storage() {
        let with = |download_storage, download_dir: &str| Settings {
            download_storage,
            download_dir: download_dir.into(),
            ..Default::default()
        };
        let configured = state(dav());
        assert!(
            configured
                .download_store(&with(Storage::Local, "downloads"))
                .is_ok()
        );
        assert!(
            configured
                .download_store(&with(Storage::WebDav, "/downloads"))
                .is_ok()
        );
        assert_eq!(
            configured
                .download_store(&with(Storage::WebDav, "downloads"))
                .err()
                .unwrap(),
            "remote path \"downloads\" is invalid: it must start with / and have no empty, . \
             or .. segments"
        );
        assert_eq!(
            state(None)
                .download_store(&with(Storage::WebDav, "/downloads"))
                .err()
                .unwrap(),
            format!("download_storage {UNCONFIGURED}")
        );
    }

    #[test]
    fn storage_changes_replace_only_their_side() {
        let base = Settings::default();
        for changed in [
            Settings {
                share_storage: Storage::WebDav,
                ..base.clone()
            },
            Settings {
                fast_scan: true,
                ..base.clone()
            },
        ] {
            assert!(changed.shares_differ(&base));
            assert!(!changed.downloads_differ(&base));
        }
        let downloads = Settings {
            download_storage: Storage::WebDav,
            ..base.clone()
        };
        assert!(downloads.downloads_differ(&base));
        assert!(!downloads.shares_differ(&base));
    }

    #[test]
    fn switching_download_storage_needs_every_download_finished() {
        let local = Settings::default();
        let webdav = Settings {
            download_storage: Storage::WebDav,
            ..Default::default()
        };
        assert_eq!(
            webdav.check_download_switch(&local, 3),
            Err(
                "Finish or clear the 3 unfinished downloads before switching download storage"
                    .to_owned()
            )
        );
        assert_eq!(
            local.check_download_switch(&webdav, 1),
            Err(
                "Finish or clear the 1 unfinished download before switching download storage"
                    .to_owned()
            )
        );
        assert_eq!(webdav.check_download_switch(&local, 0), Ok(()));
        assert_eq!(webdav.check_download_switch(&webdav, 5), Ok(()));
    }

    #[test]
    fn a_staged_update_refuses_the_switch_while_downloads_are_unfinished() {
        let state = state(dav());
        let switched = Settings {
            download_storage: Storage::WebDav,
            download_dir: "/downloads".into(),
            ..state.effective()
        };
        assert_eq!(
            state.stage_update(switched.clone(), 2).err().unwrap(),
            "Finish or clear the 2 unfinished downloads before switching download storage"
        );
        let change = state.stage_update(switched, 0).unwrap();
        assert!(change.downloads.is_some());
        assert!(change.shares.is_none());
        let moved = Settings {
            download_dir: "elsewhere".into(),
            ..state.effective()
        };
        assert!(state.stage_update(moved, 2).unwrap().downloads.is_some());
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
                pushover_private_messages: true,
                pushover_token: "token".into(),
                ..base()
            },
            Settings {
                pushover_private_messages: true,
                pushover_user_key: "key".into(),
                ..base()
            },
        ];
        for settings in invalid {
            assert!(settings.runtime_config().is_err());
        }
        let valid = Settings {
            rescan_hour_utc: 23,
            pushover_private_messages: true,
            pushover_token: "token".into(),
            pushover_user_key: "key".into(),
            ..base()
        };
        assert!(valid.runtime_config().is_ok());
    }

    #[test]
    fn a_broken_description_template_fails_at_the_settings_boundary() {
        let with_description = |description: &str| Settings {
            server: "127.0.0.1:2242".into(),
            description: description.into(),
            ..Default::default()
        };
        assert!(
            with_description("hi ${user.nope}")
                .description_template()
                .is_err()
        );
        assert!(
            with_description("${user.is_buddy?Hey buddy:Hi} ${user.name}")
                .description_template()
                .is_ok()
        );
    }

    #[test]
    fn the_settings_page_documents_every_description_variable() {
        let page = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/web/src/tabs/Settings.svelte"
        ))
        .unwrap();
        for name in crate::profile::description_variables() {
            assert!(
                page.contains(name),
                "{name} is missing from the profile hint"
            );
        }
    }
}
