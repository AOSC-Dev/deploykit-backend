use std::{
    fs, io,
    path::{Path, PathBuf},
};

use fancy_regex::Regex;
use glob::glob;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use tracing::{debug, error};
use walkdir::WalkDir;

#[derive(Debug, Serialize, Deserialize)]
pub struct QuirkConfig {
    pub model: QuirkConfigModel,
    #[serde(default)]
    pub quirk: QuirkConfigInner,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QuirkConfigModel {
    #[serde(rename = "dmi")]
    Dmi { dmi_pattern: String },
    #[serde(rename = "path")]
    Path { path_pattern: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuirkConfigInner {
    #[serde(default = "QuirkConfigInner::default_command")]
    pub command: String,
    pub skip_stages: Option<Vec<String>>,
}

impl QuirkConfigInner {
    fn default_command() -> String {
        "quick.bash".to_string()
    }
}

impl Default for QuirkConfigInner {
    fn default() -> Self {
        Self {
            command: QuirkConfigInner::default_command(),
            skip_stages: None,
        }
    }
}

#[derive(Debug, Snafu)]
pub enum QuirkError {
    #[snafu(display("Read {} failed", path.display()))]
    Read { source: io::Error, path: PathBuf },
    #[snafu(display("Failed to parse file {}", path.display()))]
    Parse {
        source: toml::de::Error,
        path: PathBuf,
    },
}

fn get_quirk_configs(dir: impl AsRef<Path>) -> Vec<(QuirkConfig, PathBuf)> {
    let mut configs = vec![];
    for i in WalkDir::new(dir)
        .min_depth(2)
        .max_depth(2)
        .into_iter()
        .flatten()
    {
        if i.file_name() != "quick.toml" || !i.path().is_file() {
            continue;
        }

        match read_quirk_config(i.path()) {
            Ok(config) => configs.push((config, i.path().to_path_buf())),
            Err(e) => {
                error!("{e}");
            }
        }
    }

    configs
}

pub fn get_matches_quirk(dir: impl AsRef<Path>) -> Vec<QuirkConfigInner> {
    let configs = get_quirk_configs(dir);
    let mut matches = vec![];
    let modalias = match read_modalias() {
        Ok(m) => m,
        Err(e) => {
            error!("{e}");
            return vec![];
        }
    };

    for (mut config, path) in configs {
        match config.model {
            QuirkConfigModel::Dmi { ref dmi_pattern } => {
                let regex = match Regex::new(dmi_pattern) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Pattern {} not illegal: {}", dmi_pattern, e);
                        continue;
                    }
                };

                let is_match = match regex.is_match(&modalias) {
                    Ok(b) => b,
                    Err(e) => {
                        error!("Regex runtime error for {}: {}", dmi_pattern, e);
                        continue;
                    }
                };

                if !is_match {
                    debug!("{} and {} not match", dmi_pattern, modalias);
                    continue;
                }

                modify_command_path(&mut config, &path);
                matches.push(config.quirk);
            }
            QuirkConfigModel::Path { ref path_pattern } => {
                let mut match_paths = match glob(path_pattern) {
                    Ok(paths) => paths,
                    Err(e) => {
                        error!("Pattern {} is not illegal: {}", path_pattern, e);
                        continue;
                    }
                };

                if match_paths.next().is_none() {
                    debug!("{} glob not match path anything.", path_pattern);
                    continue;
                }

                modify_command_path(&mut config, &path);
                matches.push(config.quirk);
            }
        }
    }

    matches
}

fn modify_command_path(config: &mut QuirkConfig, path: &Path) {
    if !Path::new(&config.quirk.command).is_absolute() {
        config.quirk.command = path
            .join(&config.quirk.command)
            .to_string_lossy()
            .to_string()
    }
}

fn read_modalias() -> Result<String, QuirkError> {
    let mut s = fs::read_to_string("/sys/class/dmi/id/modalias").context(ReadSnafu {
        path: PathBuf::from("/sys/class/dmi/id/modalias"),
    })?;

    let trimmed = s.trim_end();
    s.truncate(trimmed.len());

    Ok(s)
}

fn read_quirk_config(i: &Path) -> Result<QuirkConfig, QuirkError> {
    let config = fs::read_to_string(i).context(ReadSnafu {
        path: i.to_path_buf(),
    })?;

    let config: QuirkConfig = toml::from_str(&config).context(ParseSnafu {
        path: i.to_path_buf(),
    })?;

    Ok(config)
}
