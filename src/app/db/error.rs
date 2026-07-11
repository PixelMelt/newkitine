pub fn fatal(error: sqlx::Error) -> ! {
    tracing::error!(%error, "database write failed, terminating");
    std::process::exit(1);
}
