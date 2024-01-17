use std::future::pending;

use eyre::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use tracing_subscriber::fmt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use zbus::{dbus_interface, ConnectionBuilder};

use crate::error::DeploykitError;

mod error;

#[derive(Debug, Serialize, Deserialize)]
struct InstallConfig {
    locale: Option<String>,
    timezone: Option<String>,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            locale: None,
            timezone: None,
        }
    }
}

#[dbus_interface(name = "io.aosc.Deploykit1")]
impl InstallConfig {
    fn get_config(&self, field: &str) -> String {
        if field.is_empty() {
            match serde_json::to_string(self) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to get config: {e}");
                    serde_json::to_string(&DeploykitError::get_config(e))
                        .expect("Failed to serialize error")
                }
            }
        } else {
            match field {
                "locale" => self.locale.clone().unwrap_or_else(|| {
                    error!("field {field} is not set");
                    serde_json::to_string(&DeploykitError::not_set(field))
                        .expect("Failed to serialize error")
                }),
                "timezone" => self.timezone.clone().unwrap_or_else(|| {
                    error!("field {field} is not set");
                    serde_json::to_string(&DeploykitError::not_set(field))
                        .expect("Failed to serialize error")
                }),
                _ => {
                    error!("Unknown field: {field}");
                    serde_json::to_string(&DeploykitError::unknown_field(field))
                        .expect("Failed to serialize error")
                }
            }
        }
    }

    fn set_locale(&mut self, locale: &str) {
        // TODO: 检查 locale 是否合法
        self.locale = Some(locale.to_string());
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let env_log = EnvFilter::try_from_default_env();

    if let Ok(filter) = env_log {
        tracing_subscriber::registry()
            .with(fmt::layer().with_filter(filter))
            .init();
    } else {
        tracing_subscriber::registry().with(fmt::layer()).init();
    }

    let install_config = InstallConfig::default();
    let _conn = ConnectionBuilder::system()?
        .name("io.aosc.Deploykit")?
        .serve_at("/io/aosc/Deploykit", install_config)?
        .build()
        .await?;

    debug!("zbus session created");
    pending::<()>().await;

    Ok(())
}
