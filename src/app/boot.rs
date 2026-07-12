use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::client::Client;
use crate::types::{ClientBootstrap, Settings, TransferSnapshot};

use super::behavior::{self, Behavior};
use super::chat::Chat;
use super::config::{self, Bootstrap, GluetunConfig};
use super::interests::{self, Interests};
use super::projection::{AppData, Projection};
use super::search::{self, SearchState};
use super::session::Session;
use super::settings::{self, SettingsState};
use super::state::App;
use super::stats::{self, StatsSink};
use super::transfers::{self, Transfers};
use super::users::{self, UsersState};
use super::{api, db, events, geo, gluetun};

async fn connect_database(url: &str) -> sqlx::MySqlPool {
    let mut attempts = 0u32;
    loop {
        match db::connect(url).await {
            Ok(pool) => {
                info!("connected to database");
                return pool;
            }
            Err(error) => {
                attempts += 1;
                if attempts > 30 {
                    panic!("cannot connect to database at {url}: {error}");
                }
                warn!(%error, attempts, "database not ready, retrying");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

async fn initial_gluetun_port(gluetun: &GluetunConfig) -> u16 {
    let mut last_error = String::new();
    for attempt in 1..=5u32 {
        match gluetun::fetch_forwarded_port(gluetun).await {
            Ok(port) => return port,
            Err(error) => {
                warn!(%error, attempt, "cannot fetch initial forwarded port from gluetun");
                last_error = error;
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    }
    panic!("gluetun is configured but its forwarded port is unavailable: {last_error}");
}

pub async fn run(config_path: PathBuf) {
    let bootstrap = Bootstrap::load(&config_path);

    let pool = connect_database(&bootstrap.database_url).await;
    db::init_schema(&pool).await;

    let stored_settings: Settings = match settings::load_settings(&pool).await {
        Some(data) => serde_json::from_str(&data)
            .unwrap_or_else(|error| panic!("corrupt settings row: {error}")),
        None => {
            let defaults = Settings::default();
            settings::save_settings(&pool, &serde_json::to_string(&defaults).unwrap())
                .await
                .expect("save default settings");
            defaults
        }
    };
    let locked_settings = config::apply_settings_env(&mut stored_settings.clone());

    let gluetun_port = match &bootstrap.gluetun {
        Some(gluetun) => Some(initial_gluetun_port(gluetun).await),
        None => None,
    };
    if let Some(port) = gluetun_port {
        info!(port, "using forwarded port from gluetun");
    }
    let settings_state = SettingsState::new(stored_settings, locked_settings, gluetun_port);
    let settings = settings_state.effective();

    let runtime = settings.runtime_config().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1)
    });
    let transfer_views = transfers::bootstrap(&pool).await;
    let client_config = ClientBootstrap {
        runtime,
        buddies: db::load_list(&pool, "buddy").await,
        banned: db::load_list(&pool, "banned").await,
        ignored: db::load_list(&pool, "ignored").await,
        ip_bans: db::load_list(&pool, "ip_ban").await,
        wishlist: search::load_wishlist(&pool).await,
        liked_interests: interests::load_interests(&pool, "liked").await,
        hated_interests: interests::load_interests(&pool, "hated").await,
        transfers: transfer_views.iter().map(TransferSnapshot::from).collect(),
    };

    let data = AppData::new(
        Session::new(
            settings.server.clone(),
            settings.username.clone(),
            settings.listen_port,
        ),
        Transfers::load(transfer_views),
        SearchState::load(client_config.wishlist.clone()),
        UsersState::load(
            &client_config.buddies,
            users::load_notes(&pool).await,
            client_config.banned.clone(),
            client_config.ignored.clone(),
        ),
        Chat::load(db::load_list(&pool, "chat").await),
        Interests::load(
            client_config.liked_interests.clone(),
            client_config.hated_interests.clone(),
        ),
    );

    let (client, client_events, transfer_events) = Client::spawn(client_config);

    let web_bind = bootstrap.web_bind.clone();
    let gluetun_config: Option<GluetunConfig> = bootstrap.gluetun.clone();
    let geo = bootstrap
        .geoip_db
        .as_deref()
        .map(|path| geo::Geo::load(std::path::Path::new(path)));
    let app = Arc::new(App {
        client,
        db: pool,
        projection: Projection::new(data),
        settings: settings_state,
        geo,
        stats: StatsSink::default(),
        behavior: Behavior::default(),
    });

    behavior::load(&app).await;

    let events_task = tokio::spawn(events::run(app.clone(), client_events));
    let worker_task = tokio::spawn(transfers::run_worker(app.clone(), transfer_events));
    let stats_task = tokio::spawn(stats::flush_loop(app.clone()));
    let gluetun_task = match gluetun_config {
        Some(gluetun) => tokio::spawn(gluetun::watch(
            app.clone(),
            gluetun,
            gluetun_port.expect("gluetun configured without an initial forwarded port"),
        )),
        None => tokio::spawn(std::future::pending()),
    };

    let bind_addrs: Vec<std::net::SocketAddr> =
        std::net::ToSocketAddrs::to_socket_addrs(web_bind.as_str())
            .unwrap_or_else(|error| panic!("cannot resolve web_bind {web_bind}: {error}"))
            .collect();
    if !bootstrap.allow_public_bind && bind_addrs.iter().any(|addr| !addr.ip().is_loopback()) {
        panic!(
            "refusing to bind the unauthenticated web interface on {web_bind}; \
             set allow_public_bind = true or NEWKITINE_ALLOW_PUBLIC_BIND=1 to accept the exposure"
        );
    }

    let web_root = std::env::var("NEWKITINE_WEB_ROOT").unwrap_or_else(|_| "web/dist".into());
    let router = api::router(app, &web_root);
    let listener = tokio::net::TcpListener::bind(&web_bind)
        .await
        .unwrap_or_else(|error| panic!("cannot bind web interface on {web_bind}: {error}"));
    info!(%web_bind, "web interface listening");
    tokio::select! {
        result = events_task => task_ended("client event loop", result),
        result = worker_task => task_ended("transfer worker", result),
        result = stats_task => task_ended("statistics flush loop", result),
        result = gluetun_task => task_ended("gluetun watcher", result),
        result = axum::serve(listener, router) => {
            match result {
                Ok(()) => tracing::error!("web server stopped"),
                Err(error) => tracing::error!(%error, "web server failed"),
            }
            std::process::exit(1);
        }
    }
}

fn task_ended(task: &str, result: Result<(), tokio::task::JoinError>) -> ! {
    match result {
        Ok(()) => tracing::error!(task, "task terminated, shutting down"),
        Err(error) => tracing::error!(task, %error, "task panicked, shutting down"),
    }
    std::process::exit(1);
}
