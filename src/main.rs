use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;
use tracing::{info, warn};

use newkitine::app;
use newkitine::client::Client;
use newkitine::types::{ClientBootstrap, Settings, TransferSeed};

use app::behavior::Behavior;
use app::chat::Chat;
use app::config::{Bootstrap, GluetunConfig};
use app::interests::Interests;
use app::search::SearchState;
use app::session::Session;
use app::settings::SettingsState;
use app::state::{App, AppData};
use app::stats::StatsSink;
use app::transfers::Transfers;
use app::users::UsersState;
use app::{
    api, behavior, config, db, events, geo, gluetun, interests, search, settings, stats, transfers,
    users,
};

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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "newkitine=info".into()),
        )
        .init();

    let config_path = PathBuf::from(
        std::env::var("NEWKITINE_CONFIG").unwrap_or_else(|_| "newkitine.toml".into()),
    );
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
        transfers: transfer_views.iter().map(TransferSeed::from).collect(),
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

    let (client, client_events) = Client::spawn(client_config);
    let (events_tx, _) = broadcast::channel(256);
    let (transfer_work_tx, transfer_work_rx) = tokio::sync::mpsc::channel(4096);

    let web_bind = bootstrap.web_bind.clone();
    let gluetun_config: Option<GluetunConfig> = bootstrap.gluetun.clone();
    let geo = bootstrap
        .geoip_db
        .as_deref()
        .map(|path| geo::Geo::load(std::path::Path::new(path)));
    let app = Arc::new(App {
        client,
        db: pool,
        data: RwLock::new(data),
        events: events_tx,
        settings: settings_state,
        geo,
        stats: StatsSink::default(),
        behavior: Behavior::default(),
        settings_revision: AtomicU64::new(0),
        transfer_work: transfer_work_tx,
    });

    behavior::load(&app).await;

    tokio::spawn(events::run(app.clone(), client_events));
    tokio::spawn(transfers::run_worker(app.clone(), transfer_work_rx));
    tokio::spawn(stats::flush_loop(app.clone()));
    if let Some(gluetun) = gluetun_config {
        tokio::spawn(gluetun::watch(
            app.clone(),
            gluetun,
            gluetun_port.expect("gluetun configured without an initial forwarded port"),
        ));
    }

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
    axum::serve(listener, router)
        .await
        .expect("web server failed");
}
