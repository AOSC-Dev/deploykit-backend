use eyre::Result;
use logind_zbus::manager::{InhibitType, ManagerProxy};
use tracing::info;
use zbus::{zvariant::OwnedFd, Connection};

pub async fn take_wake_lock(conn: &Connection) -> Result<Vec<OwnedFd>> {
    let proxy = ManagerProxy::new(conn).await?;

    let mut fds = Vec::new();
    for what in [InhibitType::Shutdown, InhibitType::Sleep] {
        let fd = proxy
            .inhibit(what, "Deploykit", "Deploykit Installing system", "block")
            .await?;

        fds.push(fd);
    }

    info!("take wake lock: {:?}", fds);

    Ok(fds)
}
