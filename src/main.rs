use std::future::pending;

use crate::server::DeploykitServer;
use eyre::Result;
use tracing::debug;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use zbus::ConnectionBuilder;

mod error;
mod server;

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
