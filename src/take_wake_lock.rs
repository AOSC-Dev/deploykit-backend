use eyre::Result;
use logind_zbus::manager::{InhibitType, ManagerProxy};
use tracing::info;
use zbus::{Connection, zvariant::OwnedFd};

pub async fn take_wake_lock(conn: &Connection) -> Result<Vec<OwnedFd>> {
    let proxy = ManagerProxy::new(conn).await?;

    let mut fds = vec![];
    for i in [
        InhibitType::Sleep,
        InhibitType::Idle,
        InhibitType::HandlePowerKey,
        InhibitType::HandleSuspendKey,
        InhibitType::HandleHibernateKey,
        InhibitType::HandleLidSwitch,
    ] {
        let fd = proxy
            .inhibit(i, "Deploykit", "Deploykit Installing system", "block")
            .await?;

        fds.push(fd);
    }

    info!("take wake lock: {:?}", fds);

    Ok(fds)
}
