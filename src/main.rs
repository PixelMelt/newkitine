mod app;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;
use tracing::{info, warn};

use newkitine::client::Client;

use app::config::{Bootstrap, GluetunConfig};
use app::settings::Settings;
use app::state::{App, AppData};
use app::{api, db, events, gluetun};

async fn initial_gluetun_port(gluetun: &GluetunConfig) -> Option<u16> {
    for attempt in 1..=5u32 {
        match gluetun::fetch_forwarded_port(gluetun).await {
            Ok(port) => return Some(port),
            Err(error) => {
                warn!(%error, attempt, "cannot fetch initial forwarded port from gluetun");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    }
    None
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

    let pool = db::connect(&bootstrap.database_url).await;
    db::init_schema(&pool).await;

    let stored_settings: Settings = match db::load_settings(&pool).await {
        Some(data) => serde_json::from_str(&data)
            .unwrap_or_else(|error| panic!("corrupt settings row: {error}")),
        None => {
            let settings = Settings::default();
            db::save_settings(&pool, &serde_json::to_string(&settings).unwrap()).await;
            settings
        }
    };
    let mut settings = stored_settings.clone();
    let locked_settings = settings.apply_env();

    let gluetun_port = match &bootstrap.gluetun {
        Some(gluetun) => initial_gluetun_port(gluetun).await,
        None => None,
    };
    if let Some(port) = gluetun_port {
        info!(port, "using forwarded port from gluetun");
        settings.listen_port = port;
    }

    let mut client_config = settings.to_client_config().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1)
    });
    client_config.buddies = db::load_list(&pool, "buddy").await;
    client_config.banned = db::load_list(&pool, "banned").await;
    client_config.ignored = db::load_list(&pool, "ignored").await;
    client_config.wishlist = db::load_wishlist(&pool).await;
    client_config.liked_interests = db::load_interests(&pool, "liked").await;
    client_config.hated_interests = db::load_interests(&pool, "hated").await;

    let mut data = AppData::default();
    data.status.server = settings.server.clone();
    data.status.username = settings.username.clone();
    data.status.listen_port = settings.listen_port;
    data.banned = client_config.banned.clone();
    data.ignored = client_config.ignored.clone();
    data.wishlist = client_config.wishlist.clone();
    data.interests.liked = client_config.liked_interests.clone();
    data.interests.hated = client_config.hated_interests.clone();
    for username in &client_config.buddies {
        data.buddies.insert(username.clone(), events::buddy_view(username));
    }
    for (username, note) in db::load_notes(&pool).await {
        if let Some(buddy) = data.buddies.get_mut(&username) {
            buddy.note = note;
        }
    }
    for view in db::load_transfers(&pool, "download").await {
        data.downloads.insert((view.username.clone(), view.virtual_path.clone()), view);
    }
    for view in db::load_transfers(&pool, "upload").await {
        data.uploads.insert((view.username.clone(), view.virtual_path.clone()), view);
    }

    let resumable: Vec<_> = data
        .downloads
        .values()
        .filter(|view| view.status == "queued" || view.status == "transferring")
        .cloned()
        .collect();

    let (client, client_events) = Client::spawn(client_config);
    let (events_tx, _) = broadcast::channel(256);

    let web_bind = bootstrap.web_bind.clone();
    let gluetun_config: Option<GluetunConfig> = bootstrap.gluetun.clone();
    let app = Arc::new(App {
        client,
        db: pool,
        data: RwLock::new(data),
        events: events_tx,
        settings: RwLock::new(stored_settings),
        locked_settings,
        gluetun_enabled: gluetun_config.is_some(),
    });

    for view in resumable {
        info!(username = view.username, virtual_path = view.virtual_path, "resuming download");
        app.client.download(&view.username, &view.virtual_path, view.size, view.attributes);
    }

    tokio::spawn(events::run(app.clone(), client_events));
    if let Some(gluetun) = gluetun_config {
        tokio::spawn(gluetun::watch(app.clone(), gluetun, gluetun_port));
    }

    let web_root = std::env::var("NEWKITINE_WEB_ROOT").unwrap_or_else(|_| "web/dist".into());
    let router = api::router(app, &web_root);
    let listener = tokio::net::TcpListener::bind(&web_bind)
        .await
        .unwrap_or_else(|error| panic!("cannot bind web interface on {web_bind}: {error}"));
    info!(%web_bind, "web interface listening");
    axum::serve(listener, router).await.expect("web server failed");
}
