use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Serialize, Default)]
pub struct InstallConfig {
    pub tarball: Option<Tarball>,
    pub mirror: Option<String>,
    pub storage: Option<Storage>,
    pub install_rescuekit: Option<bool>,
    pub user: Option<User>,
    pub hostname: Option<String>,
    pub timezone: Option<Timezone>,
    pub swap_size: Option<usize>,
}

#[derive(Debug, Serialize)]
pub enum Tarball {
    Desktop,
    Server,
    Base,
}

#[derive(Debug, Serialize)]
pub struct Storage {
    path: Option<PathBuf>,
    parent_path: Option<PathBuf>,
    fs_type: Option<String>,
    size: u64,
}

#[derive(Debug, Serialize)]
pub struct User {
    username: String,
    password: String,
    root_password: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Timezone {
    language: String,
    timezone: String,
    tc: TC,
}

#[derive(Debug, Serialize)]
pub enum TC {
    RTC,
    UTC,
}
