use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    process::{Command, Stdio},
};

use tracing::info;

use crate::{utils::run_command, InstallError};

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

    let mut passwd = buf.split('\n').map(|x| x.to_string()).collect::<Vec<_>>();

    set_full_name(full_name, username, &mut passwd)?;
    f.seek(SeekFrom::Start(0))
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/passwd".to_string(),
            err: e,
        })?;

    f.write_all(passwd.join("\n").as_bytes())
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/passwd".to_string(),
            err: e,
        })?;

    Ok(())
}

fn set_full_name(
    full_name: &str,
    username: &str,
    passwd: &mut Vec<String>,
) -> Result<(), InstallError> {
    for i in [':', '\n'] {
        if full_name.contains(i) {
            return Err(InstallError::FullNameIllegal(full_name.to_string()));
        }
    }

    let mut index = None;

    for (i, c) in passwd.iter().enumerate() {
        if c.trim().is_empty() {
            continue;
        }

        let mut split_entry = c.split(':');
        let entry_username = split_entry
            .next()
            .ok_or(InstallError::PasswdIllegal)?;

        if entry_username == username {
            index = Some(i);
        }
    }

    let index = index.ok_or(InstallError::PasswdIllegal)?;
    let mut entry = passwd[index].split(':').collect::<Vec<_>>();
    entry[4] = full_name;
    passwd[index] = entry.join(":").to_owned();

    Ok(())
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
fn test_set_fullname() {
    let mut passwd_1 = r#"root:x:0:0:root:/root:/bin/bash
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
"#.split('\n').map(|x| x.to_string()).collect::<Vec<_>>();
let mut passwd_2 = passwd_1.clone();
let mut passwd_3 = passwd_1.clone();


    set_full_name("Mag Mell", "saki", &mut passwd_1).unwrap();
    assert_eq!(passwd_1.join("\n"), "root:x:0:0:root:/root:/bin/bash\nbin:x:1:1:bin:/dev/null:/bin/false\nnobody:x:99:99:Unprivileged User:/dev/null:/bin/false\ndbus:x:18:18:D-Bus Message Daemon User:/var/run/dbus:/bin/false\nsystemd-journal-gateway:x:994:994:systemd Journal Gateway:/:/sbin/nologin\nsystemd-bus-proxy:x:993:993:systemd Bus Proxy:/:/sbin/nologin\nsystemd-network:x:992:992:systemd Network Management:/:/sbin/nologin\nsystemd-resolve:x:991:991:systemd Resolver:/:/sbin/nologin\nsystemd-timesync:x:990:990:systemd Time Synchronization:/:/sbin/nologin\nsystemd-journal-remote:x:989:989:systemd Journal Remote:/:/sbin/nologin\nsystemd-journal-upload:x:988:988:systemd Journal Upload:/:/sbin/nologin\nldap:x:439:439:LDAP daemon owner:/var/lib/openldap:/bin/bash\nhttp:x:207:207:HTTP daemon:/srv/http:/bin/true\nuuidd:x:209:209:UUIDD user:/dev/null:/bin/true\nlocate:x:191:191:Locate daemon owner:/var/lib/mlocate:/bin/bash\npolkitd:x:27:27:PolicyKit Daemon Owner:/etc/polkit-1:/bin/false\nrtkit:x:133:133:RealtimeKit User:/proc:/sbin/nologin\nnamed:x:40:40:BIND DNS Server:/var/named:/sbin/nologin\ntss:x:159:159:Account used by the trousers package to sandbox the tcsd daemon:/dev/null:/sbin/nologin\nunbound:x:986:986:unbound:/etc/unbound:/bin/false\nsystemd-coredump:x:985:985:systemd Core Dumper:/:/sbin/nologin\nsystemd-nobody:x:65534:65534:Unprivileged User (systemd):/dev/null:/bin/false\nsystemd-oom:x:980:980:systemd Userspace OOM Killer:/:/usr/bin/nologin\nmysql:x:89:89:MariaDB Daemon User:/var/lib/mysql:/bin/false\ndnsmasq:x:488:6:dnsmasq daemon owner:/:/bin/nologin\npostgres:x:90:90:Postgres Daemon Owner:/var/lib/postgres:/bin/bash\navahi:x:84:84:Avahi Daemon Owner:/run/avahi-daemon:/bin/false\nmongodb:x:300:6:MongoDB daemon owner:/var/lib/mongodb:/bin/bash\ncolord:x:124:124:Color Daemon Owner:/var/lib/colord:/bin/false\nfcron:x:33:33::/var/spool/fcron:/bin/bash\nflatpak:x:979:979:Flatpak system helper:/:/usr/bin/nologin\nsaned:x:978:978:SANE Daemon Owner:/:/usr/bin/nologin\nsddm:x:977:977:Simple Desktop Display Manager Daemon Owner:/var/lib/sddm:/usr/bin/nologin\nrpc:x:332:332:Rpcbind Daemon:/dev/null:/bin/false\nusbmux:x:140:140:usbmux user:/:/sbin/nologin\nnm-openconnect:x:104:104:NetworkManager user for OpenConnect:/:/sbin/nologin\nsaki:x:1000:1001:Mag Mell:/home/saki:/bin/bash\npulse:x:58:58:PulseAudio Daemon Owner:/var/run/pulse:/bin/false\n_apt:x:976:976::/var/lib/apt:/sbin/nologin\n");
    assert!(set_full_name("Mag Mell\n", "saki", &mut passwd_2).is_err());
    assert!(set_full_name("Mag Mell:", "saki", &mut passwd_3).is_err());
}
