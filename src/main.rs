use std::future::pending;

use crate::server::DeploykitServer;
use eyre::Result;
use take_wake_lock::take_wake_lock;
use tracing::level_filters::LevelFilter;
use tracing::{debug, info};
use tracing_subscriber::fmt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use zbus::{Connection, ConnectionBuilder};

mod error;
mod server;
mod take_wake_lock;

#[tokio::main]
async fn main() -> Result<()> {
    let env_log = EnvFilter::try_from_default_env();

    if let Ok(filter) = env_log {
        tracing_subscriber::registry()
            .with(fmt::layer().with_filter(filter))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(LevelFilter::DEBUG)
            .init();
    }

    info!("Deploykit version: {}", env!("VERGEN_GIT_DESCRIBE"));

    let conn = Connection::system().await?;
    take_wake_lock(&conn).await?;

    let deploykit_server = DeploykitServer::default();

    let _conn = ConnectionBuilder::system()?
        .name("io.aosc.Deploykit")?
        .serve_at("/io/aosc/Deploykit", deploykit_server)?
        .build()
        .await?;

    debug!("zbus session created");
    pending::<()>().await;

    Ok(())
}
