use clap::Parser;
use eyre::Result;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};
use zbus::Result as zResult;
use zbus::{dbus_proxy, Connection};

#[dbus_proxy(
    interface = "io.aosc.Deploykit1",
    default_service = "io.aosc.Deploykit",
    default_path = "/io/aosc/Deploykit"
)]
trait Deploykit {
    async fn set_config(&self, field: &str, value: &str) -> zResult<String>;
    async fn get_config(&self, field: &str) -> zResult<String>;
    async fn get_progress(&self) -> zResult<String>;
    async fn reset_config(&self) -> zResult<String>;
}

#[derive(Parser, Debug)]
struct Args {
    /// Select AOSC OS variant to install (e.g., Desktop, Server, Base)
    #[clap(long, default_value = "Base")]
    flaver: String,
    /// Set URL for download source
    #[clap(long, default_value = "https://repo.aosc.io/aosc-os")]
    mirror_url: String,
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
    /// Toggle using RTC (real time clock) time as local time
    #[clap(long, action = clap::ArgAction::SetTrue)]
    rtc_as_localtime: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let Args {
        flaver,
        mirror_url,
        user,
        password,
        hostname,
        timezone,
        locale,
        rtc_as_localtime,
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

    proxy.set_config("flaver", &flaver).await?;
    proxy.set_config("mirror_url", &mirror_url).await?;
    proxy.set_config("timezone", &timezone).await?;
    proxy.set_config("locale", &locale).await?;
    proxy
        .set_config("rtc_as_localtime", if rtc_as_localtime { "1" } else { "0" })
        .await?;

    proxy.set_config("hostname", &hostname).await?;
    proxy
        .set_config(
            "user",
            &serde_json::json! {{
                "username": &user,
                "password": &password,
            }}
            .to_string(),
        )
        .await?;

    println!("{}", proxy.get_config("").await?);

    proxy.reset_config().await?;
    println!("{}", proxy.get_config("").await?);

    Ok(())
}
