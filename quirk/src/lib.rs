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

const DMI_MODALIAS: &str = "/sys/class/dmi/id/modalias";
const DT_COMPATIBLE: &str = "/sys/firmware/devicetree/base/compatible";

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
    #[serde(rename = "dt_compatible")]
    DTCompatible { compatible_pattern: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuirkConfigInner {
    #[serde(default = "QuirkConfigInner::default_command")]
    pub command: String,
    pub skip_stages: Option<Vec<String>>,
}

impl QuirkConfigInner {
    fn default_command() -> String {
        "quirk.bash".to_string()
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
    #[snafu(display("Pattern {regex} got error"))]
    Regex {
        source: fancy_regex::Error,
        regex: String,
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
        if i.file_name() != "quirk.toml" || !i.path().is_file() {
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
    let dt_compatible = match read_dt_compatible() {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };
    let modalias = match read_modalias() {
        Ok(m) => m,
        Err(e) => {
            error!("{}", e);
            String::new()
        }
    };

    for (mut config, path) in configs {
        match config.model {
            QuirkConfigModel::Dmi { ref dmi_pattern } => {
                match dmi_is_match(&modalias, dmi_pattern) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        error!("{e}");
                        continue;
                    }
                }

                modify_command_path(&mut config, &path);
                matches.push(config.quirk);
            }
            QuirkConfigModel::Path { ref path_pattern } => {
                let mut match_paths = match glob(path_pattern) {
                    Ok(paths) => paths,
                    Err(e) => {
                        error!("Not a valid pattern '{}': {}", path_pattern, e);
                        continue;
                    }
                };

                if match_paths.next().is_none() {
                    debug!("Pattern '{}' didn't match anything.", path_pattern);
                    continue;
                }

                modify_command_path(&mut config, &path);
                matches.push(config.quirk);
            }
            QuirkConfigModel::DTCompatible {
                ref compatible_pattern,
            } => {
                match dt_compatible_matches(&dt_compatible, &compatible_pattern) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        // It is absent on most platforms this installer supports
                        error!("{}", e);
                        continue;
                    }
                }
                modify_command_path(&mut config, &path);
                matches.push(config.quirk);
            }
        }
    }

    matches
}

pub fn dmi_is_match(modalias: &str, dmi_pattern: &str) -> Result<bool, QuirkError> {
    let regex = Regex::new(dmi_pattern).context(RegexSnafu {
        regex: dmi_pattern.to_string(),
    })?;

    let is_match = regex.is_match(modalias).context(RegexSnafu {
        regex: dmi_pattern.to_string(),
    })?;

    if !is_match {
        debug!("{} and {} not match", dmi_pattern, modalias);
        return Ok(false);
    }

    Ok(true)
}

pub fn dt_compatible_matches(
    dt_compatible: &Vec<String>,
    pattern: &str,
) -> Result<bool, QuirkError> {
    let regex = Regex::new(pattern).context(RegexSnafu {
        regex: pattern.to_string(),
    })?;

    let mut result = false;
    for element in dt_compatible {
        if regex.is_match(&element).context(RegexSnafu {
            regex: pattern.to_string(),
        })? {
            result = true;
            break;
        }
    }
    return Ok(result);
}

fn modify_command_path(config: &mut QuirkConfig, path: &Path) {
    if !Path::new(&config.quirk.command).is_absolute() {
        let dirname = path.parent().unwrap();
        config.quirk.command = dirname
            .join(&config.quirk.command)
            .to_string_lossy()
            .to_string()
    }
}

fn read_modalias() -> Result<String, QuirkError> {
    let mut s = fs::read_to_string(DMI_MODALIAS).context(ReadSnafu {
        path: PathBuf::from(DMI_MODALIAS),
    })?;

    let trimmed = s.trim_end();
    s.truncate(trimmed.len());

    Ok(s)
}

fn read_dt_compatible() -> Result<Vec<String>, QuirkError> {
    let s = fs::read_to_string(DT_COMPATIBLE);
    let s = match s {
        Ok(s) => s,
        Err(e) => {
            // They simply don't exist, which is not a problem.
            // Return an empty one.
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(vec![]);
            }
            // Don't really know how to properly do this ...
            return Err(QuirkError::Read {
                source: e,
                path: PathBuf::from(DT_COMPATIBLE),
            });
        }
    };

    // NOTE: elemnts of arrays in DT are null separated
    let vec = s.split('\0').map(|x| x.to_string()).collect::<Vec<_>>();
    Ok(vec)
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
