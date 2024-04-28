use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use eyre::{bail, Result};
use serde::Deserialize;
use serde_json::Value;
use tokio::time::sleep;
use tracing::info;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};
use zbus::Result as zResult;
use zbus::{proxy, Connection};

#[derive(Debug, Deserialize)]
struct Dbus {
    result: DbusResult,
    data: Value,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
enum DbusResult {
    Ok,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status")]
enum AutoPartitionProgress {
    Pending,
    Working,
    Finish { res: Result<Value, Value> },
}

#[proxy(
    interface = "io.aosc.Deploykit1",
    default_service = "io.aosc.Deploykit",
    default_path = "/io/aosc/Deploykit"
)]
trait Deploykit {
    async fn set_config(&self, field: &str, value: &str) -> zResult<String>;
    async fn get_config(&self, field: &str) -> zResult<String>;
    async fn get_progress(&self) -> zResult<String>;
    async fn reset_config(&self) -> zResult<String>;
    async fn get_list_devices(&self) -> zResult<String>;
    async fn auto_partition(&self, dev: &str) -> zResult<String>;
    async fn start_install(&self) -> zResult<String>;
    async fn get_auto_partition_progress(&self) -> zResult<String>;
}

#[derive(Parser, Debug)]
struct Args {
    /// Set URL for download source
    #[clap(long)]
    // mirror_url: String,
    /// Set name of the default user
    #[clap(long)]
    user: String,
    /// Set password for default user
    #[clap(long)]
    password: String,
    /// Set device hostname
    #[clap(long, default_value = "aosc")]
    hostname: String,
    /// Set default timezone
    #[clap(long, default_value = "UTC")]
    timezone: String,
    /// Set default locale (affects display language, units, time/date format etc.)
    #[clap(long, default_value = "C.UTF-8")]
    locale: String,
    /// Set install disk (will auto partition)
    #[clap(long)]
    disk_target: String,
    /// Toggle using RTC (real time clock) time as local time
    #[clap(long, action = clap::ArgAction::SetTrue)]
    rtc_as_localtime: bool,
}

impl TryFrom<String> for Dbus {
    type Error = eyre::Error;

    fn try_from(value: String) -> std::prelude::v1::Result<Self, <Dbus as TryFrom<String>>::Error> {
        let res = serde_json::from_str::<Dbus>(&value)?;

        match res.result {
            DbusResult::Ok => Ok(res),
            DbusResult::Error => bail!("Failed to execute query: {:?}", res.data),
        }
    }
}

impl Dbus {
    async fn set_config(proxy: &DeploykitProxy<'_>, field: &str, value: &str) -> Result<Self> {
        let res = proxy.set_config(field, value).await?;
        let res = Self::try_from(res)?;

        Ok(res)
    }

    async fn auto_partition(proxy: &DeploykitProxy<'_>, dev: &str) -> Result<Self> {
        let res = proxy.auto_partition(dev).await?;
        let res = Self::try_from(res)?;

        Ok(res)
    }

    async fn get_progress(proxy: &DeploykitProxy<'_>) -> Result<Self> {
        let res = proxy.get_progress().await?;
        let res = Self::try_from(res)?;

        Ok(res)
    }

    async fn start_install(proxy: &DeploykitProxy<'_>) -> Result<Self> {
        let res = proxy.start_install().await?;
        let res = Self::try_from(res)?;

        Ok(res)
    }

    async fn get_auto_partition_progress(proxy: &DeploykitProxy<'_>) -> Result<Self> {
        let res = proxy.get_auto_partition_progress().await?;
        let res = Self::try_from(res)?;

        Ok(res)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let Args {
        user,
        password,
        hostname,
        timezone,
        locale,
        rtc_as_localtime,
        disk_target,
    } = args;

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

    let connection = Connection::system().await?;
    let proxy = DeploykitProxy::new(&connection).await?;

    Dbus::set_config(&proxy, "download", &serde_json::json!({
        "Http": {
            "url": "https://mirrors.bfsu.edu.cn/anthon/aosc-os/os-amd64/base/aosc-os_base_20240414_amd64.squashfs",
            "hash": "fe99624958e33c5b5ac71b3cf88822f343fc31814655bb3e554753a7fd0c1051",
        }
        // "File": "/home/saki/squashfs"
    })
    .to_string()).await?;

    Dbus::set_config(&proxy, "timezone", &timezone).await?;
    Dbus::set_config(&proxy, "locale", &locale).await?;
    Dbus::set_config(
        &proxy,
        "rtc_as_localtime",
        if rtc_as_localtime { "1" } else { "0" },
    )
    .await?;

    Dbus::set_config(&proxy, "hostname", &hostname).await?;

    Dbus::set_config(
        &proxy,
        "user",
        &serde_json::json! {{
            "username": &user,
            "password": &password,
        }}
        .to_string(),
    )
    .await?;

    Dbus::set_config(&proxy, "swapfile", "\"Disable\"").await?;

    info!("Auto partitioning {disk_target}...");
    Dbus::auto_partition(&proxy, &disk_target).await?;

    // 等待分区工作完成
    loop {
        let res = Dbus::get_auto_partition_progress(&proxy).await?;
        let data: AutoPartitionProgress = serde_json::from_value(res.data)?;

        match data {
            AutoPartitionProgress::Pending => println!("Pending"),
            AutoPartitionProgress::Working => println!("Working"),
            AutoPartitionProgress::Finish { res } => {
                match res {
                    Ok(_) => println!("Done"),
                    Err(e) => eprintln!("Got Error: {e:?}"),
                }
                break;
            }
        }

        sleep(Duration::from_millis(10)).await;
    }

    println!("{}", proxy.get_config("").await?);

    let proxy = Arc::new(proxy);
    let proxy_clone = proxy.clone();

    let t = tokio::spawn(async move {
        loop {
            match Dbus::get_progress(&proxy_clone).await {
                Ok(progress) => {
                    println!("Progress: {:?}", progress);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    break;
                }
            }
            sleep(Duration::from_millis(300)).await;
        }
    });

    let res = Dbus::start_install(&proxy).await?;

    println!("{:?}", res);

    t.await?;

    Ok(())
}
