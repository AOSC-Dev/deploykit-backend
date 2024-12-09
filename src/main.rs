use std::future::pending;

use crate::server::DeploykitServer;
use eyre::Result;
use take_wake_lock::take_wake_lock;
use tracing::level_filters::LevelFilter;
use tracing::{debug, info};
use tracing_subscriber::fmt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use zbus::{connection, Connection};

mod error;
mod server;
mod take_wake_lock;

#[tokio::main]
async fn main() -> Result<()> {
    let env_log = EnvFilter::try_from_default_env();

    // 按天数来划分文件
    let file_appender = tracing_appender::rolling::daily("/tmp", "dk.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    if let Ok(filter) = env_log {
        tracing_subscriber::registry()
            .with(fmt::layer().with_filter(filter))
            .with(fmt::layer().with_writer(non_blocking))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(LevelFilter::DEBUG)
            .with(fmt::layer().with_writer(non_blocking))
            .init();
    }

    info!("Deploykit version: {}", env!("VERGEN_GIT_DESCRIBE"));

    let conn = Connection::system().await?;
    let fds = take_wake_lock(&conn).await?;

    let deploykit_server = DeploykitServer::default();

    let _conn = connection::Builder::system()?
        .name("io.aosc.Deploykit")?
        .serve_at("/io/aosc/Deploykit", deploykit_server)?
        .build()
        .await?;

    debug!("zbus session created");
    pending::<()>().await;

    drop(fds);

    Ok(())
}
