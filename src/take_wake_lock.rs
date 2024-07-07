use eyre::Result;
use logind_zbus::manager::{InhibitType, ManagerProxy};
use tracing::info;
use zbus::{zvariant::OwnedFd, Connection};

pub async fn take_wake_lock(conn: &Connection) -> Result<OwnedFd> {
    let proxy = ManagerProxy::new(conn).await?;

    let fd = proxy
        .inhibit(InhibitType::Sleep, "Deploykit", "Deploykit Installing system", "block")
        .await?;

    info!("take wake lock: {:?}", fd);

    Ok(fd)
}
