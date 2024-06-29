use zbus::{proxy, zvariant::OwnedFd, Connection, Result as zResult};

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1 {
    /// Inhibit method
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zResult<OwnedFd>;
}

pub async fn take_wake_lock(conn: &Connection) -> zResult<OwnedFd> {
    let proxy = Login1Proxy::new(conn).await?;

    proxy
        .inhibit(
            "shutdown:sleep",
            "deploykit-backend",
            "deploykit maybe changing system",
            "block",
        )
        .await
}
