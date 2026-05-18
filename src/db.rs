use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub type DbPool = PgPool;

pub async fn init_pool(database_url: &str) -> Result<DbPool> {
    PgPoolOptions::new()
        .max_connections(16)
        .connect(database_url)
        .await
        .context("failed to connect to postgres")
}

pub async fn run_migrations(pool: &DbPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("failed to run database migrations")
}
