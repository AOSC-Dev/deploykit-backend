use serde::{Deserialize, Serialize};
use tracing::error;
use zbus::dbus_interface;

use crate::error::DeploykitError;

#[derive(Debug, Serialize, Deserialize)]
pub struct DeploykitServer {
    config: InstallConfig,
    progress: ProgressStatus,
}

impl Default for DeploykitServer {
    fn default() -> Self {
        Self {
            config: InstallConfig::default(),
            progress: ProgressStatus::Pending,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ProgressStatus {
    Pending,
    Working(String, u8),
    Done,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstallConfig {
    locale: Option<String>,
    timezone: Option<String>,
    flavor: Option<String>,
    mirror_url: Option<String>,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            locale: None,
            timezone: None,
            flavor: None,
            mirror_url: None,
        }
    }
}

#[dbus_interface(name = "io.aosc.Deploykit1")]
impl DeploykitServer {
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
                "locale" => self.config.locale.clone().unwrap_or_else(|| not_set_error(field)),
                "timezone" => self.config.timezone.clone().unwrap_or_else(|| not_set_error(field)),
                "flaver" => self.config.flavor.clone().unwrap_or_else(|| not_set_error(field)),
                "mirror_url" => self.config.mirror_url.clone().unwrap_or_else(|| not_set_error(field)),
                _ => {
                    error!("Unknown field: {field}");
                    serde_json::to_string(&DeploykitError::unknown_field(field))
                        .unwrap_or_else(|_| "Failed to serialize".to_string())
                }
            }
        }
    }

    fn set_config(&mut self, field: &str, value: &str) -> String {
        match set_config_inner(&mut self.config, field, value) {
            Ok(()) => "ok".to_string(),
            Err(e) => {
                error!("Failed to set config: {e}");
                serde_json::to_string(&e)
                    .unwrap_or_else(|_| "Failed to serialize error".to_string())
            }
        }
    }

    fn get_progress(&self) -> String {
        serde_json::to_string(&self.progress).unwrap_or_else(|_| "Failed to serialize".to_string())
    }
}

fn set_config_inner(
    config: &mut InstallConfig,
    field: &str,
    value: &str,
) -> Result<(), DeploykitError> {
    match field {
        "locale" => {
            config.locale = Some(value.to_string());
            Ok(())
        }
        "timezone" => {
            config.timezone = Some(value.to_string());
            Ok(())
        }
        "flavor" => {
            config.flavor = Some(value.to_string());
            Ok(())
        }
        "mirror_url" => {
            config.mirror_url = Some(value.to_string());
            Ok(())
        }
        _ => {
            error!("Unknown field: {field}");
            Err(DeploykitError::unknown_field(field))
        }
    }
}

fn not_set_error(field: &str) -> String {
    error!("field {field} is not set");
    serde_json::to_string(&DeploykitError::not_set(field))
        .unwrap_or_else(|_| "Failed to serialize".to_string())
}
