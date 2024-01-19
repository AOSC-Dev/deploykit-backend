use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    process::{Command, Stdio},
};

use tracing::info;

use crate::{utils::run_command, InstallError, PasswdIllegalKind};

/// Sets Fullname
/// Must be used in a chroot context
pub fn passwd_set_fullname(full_name: &str, username: &str) -> Result<(), InstallError> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .open("/etc/passwd")
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/passwd".to_string(),
            err: e,
        })?;

    let mut buf = String::new();
    f.read_to_string(&mut buf)
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/passwd".to_string(),
            err: e,
        })?;

    let passwd = buf.split('\n').collect::<Vec<_>>();

    let s = set_full_name(full_name, username, passwd)?;
    f.seek(SeekFrom::Start(0))
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/passwd".to_string(),
            err: e,
        })?;

    f.write_all(s.as_bytes())
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/passwd".to_string(),
            err: e,
        })?;

    Ok(())
}

struct Passwd {
    username: String,
    time: String,
    uid: String,
    gid: String,
    fullname: String,
    home: String,
    login_shell: String,
}

fn set_full_name(
    full_name: &str,
    username: &str,
    passwd: Vec<&str>,
) -> Result<String, InstallError> {
    if full_name.contains(':') || full_name.contains('\n') {
        return Err(InstallError::FullNameIllegal(full_name.to_string()));
    }

    let mut v = Vec::new();
    for i in passwd {
        if i.trim().is_empty() {
            continue;
        }

        let mut i_split = i.split(':');
        // 用户名
        let i_username = i_split
            .next()
            .ok_or(InstallError::PasswdIllegal(PasswdIllegalKind::Username))?;
        // 过期时间
        let time = i_split
            .next()
            .ok_or(InstallError::PasswdIllegal(PasswdIllegalKind::Time))?;
        // uid
        let uid = i_split
            .next()
            .ok_or(InstallError::PasswdIllegal(PasswdIllegalKind::Uid))?;
        // gid
        let gid = i_split
            .next()
            .ok_or(InstallError::PasswdIllegal(PasswdIllegalKind::Gid))?;
        // fullname
        let i_fullname = i_split
            .next()
            .ok_or(InstallError::PasswdIllegal(PasswdIllegalKind::Fullname))?;
        // 家路径
        let home_directory = i_split
            .next()
            .ok_or(InstallError::PasswdIllegal(PasswdIllegalKind::Home))?;
        // Login shell
        let login_shell = i_split
            .next()
            .ok_or(InstallError::PasswdIllegal(PasswdIllegalKind::LoginShell))?;

        v.push(Passwd {
            username: i_username.to_owned(),
            time: time.to_owned(),
            uid: uid.to_owned(),
            gid: gid.to_owned(),
            fullname: i_fullname.to_owned(),
            home: home_directory.to_owned(),
            login_shell: login_shell.to_string(),
        })
    }

    v.iter_mut()
        .filter(|x| x.username == username)
        .for_each(|x| {
            x.fullname = full_name.to_owned();
        });

    let mut s = String::new();

    for i in v {
        let entry = [
            i.username,
            i.time,
            i.uid,
            i.gid,
            i.fullname,
            i.home,
            i.login_shell,
        ]
        .join(":")
            + "\n";
        s.push_str(&entry);
    }

    Ok(s)
}

pub fn is_acceptable_username(username: &str) -> bool {
    if username.is_empty() {
        return false;
    }

    if username == "root" {
        return false;
    }

    for (i, c) in username.as_bytes().iter().enumerate() {
        if i == 0 {
            if !c.is_ascii_lowercase() {
                return false;
            }
        } else if !c.is_ascii_lowercase() && !c.is_ascii_digit() {
            return false;
        }
    }

    true
}

/// Adds a new normal user to the guest environment
/// Must be used in a chroot context
pub fn add_new_user(name: &str, password: &str) -> Result<(), InstallError> {
    run_command("useradd", ["-m", "-s", "/bin/bash", name])?;
    run_command("usermod", ["-aG", "audio,cdrom,video,wheel,plugdev", name])?;

    chpasswd(name, password)?;

    Ok(())
}

pub fn chpasswd(name: &str, password: &str) -> Result<(), InstallError> {
    info!("Running chpasswd ...");
    let command = Command::new("chpasswd")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| InstallError::RunCommand {
            command: "chpasswd".to_string(),
            err: e,
        })?;

    let mut stdin = command.stdin.ok_or(InstallError::RunCommand {
        command: "chpasswd".to_string(),
        err: io::Error::new(io::ErrorKind::NotFound, "stdin is None"),
    })?;

    stdin
        .write_all(format!("{name}:{password}\n").as_bytes())
        .map_err(|e| InstallError::RunCommand {
            command: "chpasswd".to_string(),
            err: e,
        })?;

    stdin.flush().map_err(|e| InstallError::RunCommand {
        command: "chpasswd".to_string(),
        err: e,
    })?;

    info!("Running chpasswd successfully");

    Ok(())
}

#[test]
fn test_username_validation() {
    assert!(is_acceptable_username("foo"));
    assert!(is_acceptable_username("cth451"));
    assert!(!is_acceptable_username("老白"));
    assert!(!is_acceptable_username("BAIMINGCONG"));
    assert!(!is_acceptable_username("root"));
    assert!(!is_acceptable_username("/root"));
    assert!(!is_acceptable_username("root:root"));
    assert!(!is_acceptable_username("root\n"));
    assert!(!is_acceptable_username("root\t"));
    assert!(!is_acceptable_username("ro ot"));
}

#[test]
fn test_set_fullname() {
    let passwd = r#"root:x:0:0:root:/root:/bin/bash
bin:x:1:1:bin:/dev/null:/bin/false
nobody:x:99:99:Unprivileged User:/dev/null:/bin/false
dbus:x:18:18:D-Bus Message Daemon User:/var/run/dbus:/bin/false
systemd-journal-gateway:x:994:994:systemd Journal Gateway:/:/sbin/nologin
systemd-bus-proxy:x:993:993:systemd Bus Proxy:/:/sbin/nologin
systemd-network:x:992:992:systemd Network Management:/:/sbin/nologin
systemd-resolve:x:991:991:systemd Resolver:/:/sbin/nologin
systemd-timesync:x:990:990:systemd Time Synchronization:/:/sbin/nologin
systemd-journal-remote:x:989:989:systemd Journal Remote:/:/sbin/nologin
systemd-journal-upload:x:988:988:systemd Journal Upload:/:/sbin/nologin
ldap:x:439:439:LDAP daemon owner:/var/lib/openldap:/bin/bash
http:x:207:207:HTTP daemon:/srv/http:/bin/true
uuidd:x:209:209:UUIDD user:/dev/null:/bin/true
locate:x:191:191:Locate daemon owner:/var/lib/mlocate:/bin/bash
polkitd:x:27:27:PolicyKit Daemon Owner:/etc/polkit-1:/bin/false
rtkit:x:133:133:RealtimeKit User:/proc:/sbin/nologin
named:x:40:40:BIND DNS Server:/var/named:/sbin/nologin
tss:x:159:159:Account used by the trousers package to sandbox the tcsd daemon:/dev/null:/sbin/nologin
unbound:x:986:986:unbound:/etc/unbound:/bin/false
systemd-coredump:x:985:985:systemd Core Dumper:/:/sbin/nologin
systemd-nobody:x:65534:65534:Unprivileged User (systemd):/dev/null:/bin/false
systemd-oom:x:980:980:systemd Userspace OOM Killer:/:/usr/bin/nologin
mysql:x:89:89:MariaDB Daemon User:/var/lib/mysql:/bin/false
dnsmasq:x:488:6:dnsmasq daemon owner:/:/bin/nologin
postgres:x:90:90:Postgres Daemon Owner:/var/lib/postgres:/bin/bash
avahi:x:84:84:Avahi Daemon Owner:/run/avahi-daemon:/bin/false
mongodb:x:300:6:MongoDB daemon owner:/var/lib/mongodb:/bin/bash
colord:x:124:124:Color Daemon Owner:/var/lib/colord:/bin/false
fcron:x:33:33::/var/spool/fcron:/bin/bash
flatpak:x:979:979:Flatpak system helper:/:/usr/bin/nologin
saned:x:978:978:SANE Daemon Owner:/:/usr/bin/nologin
sddm:x:977:977:Simple Desktop Display Manager Daemon Owner:/var/lib/sddm:/usr/bin/nologin
rpc:x:332:332:Rpcbind Daemon:/dev/null:/bin/false
usbmux:x:140:140:usbmux user:/:/sbin/nologin
nm-openconnect:x:104:104:NetworkManager user for OpenConnect:/:/sbin/nologin
saki:x:1000:1001::/home/saki:/bin/bash
pulse:x:58:58:PulseAudio Daemon Owner:/var/run/pulse:/bin/false
_apt:x:976:976::/var/lib/apt:/sbin/nologin
"#.split('\n').collect::<Vec<_>>();

    assert_eq!(set_full_name("Mag Mell", "saki", passwd.clone()).unwrap(), "root:x:0:0:root:/root:/bin/bash\nbin:x:1:1:bin:/dev/null:/bin/false\nnobody:x:99:99:Unprivileged User:/dev/null:/bin/false\ndbus:x:18:18:D-Bus Message Daemon User:/var/run/dbus:/bin/false\nsystemd-journal-gateway:x:994:994:systemd Journal Gateway:/:/sbin/nologin\nsystemd-bus-proxy:x:993:993:systemd Bus Proxy:/:/sbin/nologin\nsystemd-network:x:992:992:systemd Network Management:/:/sbin/nologin\nsystemd-resolve:x:991:991:systemd Resolver:/:/sbin/nologin\nsystemd-timesync:x:990:990:systemd Time Synchronization:/:/sbin/nologin\nsystemd-journal-remote:x:989:989:systemd Journal Remote:/:/sbin/nologin\nsystemd-journal-upload:x:988:988:systemd Journal Upload:/:/sbin/nologin\nldap:x:439:439:LDAP daemon owner:/var/lib/openldap:/bin/bash\nhttp:x:207:207:HTTP daemon:/srv/http:/bin/true\nuuidd:x:209:209:UUIDD user:/dev/null:/bin/true\nlocate:x:191:191:Locate daemon owner:/var/lib/mlocate:/bin/bash\npolkitd:x:27:27:PolicyKit Daemon Owner:/etc/polkit-1:/bin/false\nrtkit:x:133:133:RealtimeKit User:/proc:/sbin/nologin\nnamed:x:40:40:BIND DNS Server:/var/named:/sbin/nologin\ntss:x:159:159:Account used by the trousers package to sandbox the tcsd daemon:/dev/null:/sbin/nologin\nunbound:x:986:986:unbound:/etc/unbound:/bin/false\nsystemd-coredump:x:985:985:systemd Core Dumper:/:/sbin/nologin\nsystemd-nobody:x:65534:65534:Unprivileged User (systemd):/dev/null:/bin/false\nsystemd-oom:x:980:980:systemd Userspace OOM Killer:/:/usr/bin/nologin\nmysql:x:89:89:MariaDB Daemon User:/var/lib/mysql:/bin/false\ndnsmasq:x:488:6:dnsmasq daemon owner:/:/bin/nologin\npostgres:x:90:90:Postgres Daemon Owner:/var/lib/postgres:/bin/bash\navahi:x:84:84:Avahi Daemon Owner:/run/avahi-daemon:/bin/false\nmongodb:x:300:6:MongoDB daemon owner:/var/lib/mongodb:/bin/bash\ncolord:x:124:124:Color Daemon Owner:/var/lib/colord:/bin/false\nfcron:x:33:33::/var/spool/fcron:/bin/bash\nflatpak:x:979:979:Flatpak system helper:/:/usr/bin/nologin\nsaned:x:978:978:SANE Daemon Owner:/:/usr/bin/nologin\nsddm:x:977:977:Simple Desktop Display Manager Daemon Owner:/var/lib/sddm:/usr/bin/nologin\nrpc:x:332:332:Rpcbind Daemon:/dev/null:/bin/false\nusbmux:x:140:140:usbmux user:/:/sbin/nologin\nnm-openconnect:x:104:104:NetworkManager user for OpenConnect:/:/sbin/nologin\nsaki:x:1000:1001:Mag Mell:/home/saki:/bin/bash\npulse:x:58:58:PulseAudio Daemon Owner:/var/run/pulse:/bin/false\n_apt:x:976:976::/var/lib/apt:/sbin/nologin\n");
    assert!(set_full_name("Mag Mell\n", "saki", passwd.clone()).is_err());
    assert!(set_full_name("Mag Mell:", "saki", passwd.clone()).is_err());
}
