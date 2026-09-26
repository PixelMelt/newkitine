mod lists;
mod migrations;

pub use lists::{add_to_list, load_list, remove_from_list};
pub use migrations::init_schema;

use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;

pub async fn connect(url: &str) -> Result<MySqlPool, sqlx::Error> {
    MySqlPoolOptions::new()
        .max_connections(5)
        .connect(url)
        .await
}

pub fn fatal(error: sqlx::Error) -> ! {
    tracing::error!(%error, "database write failed, terminating");
    std::process::exit(1);
}
