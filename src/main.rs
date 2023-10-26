mod handler;

use eyre::{Result, bail};
use handler::{InstallConfig, Tarball};
use serde::{Deserialize, Serialize};
use std::io::stdin;

#[derive(Debug, Deserialize)]
struct Reqeust {
    t: ReqeustType,
}

#[derive(Debug, Deserialize)]
enum ReqeustType {
    // {"t":"GetStatus"}
    GetStatus,
    // // {"t":{"Echo":"qaq"}}
    // Echo(String),
    // // {"t":{"StructEcho":{"arg_1":"a","arg_2":"b"}}},
    // StructEcho { arg_1: String, arg_2: String },
    SetTarball(String),
    SetMirror(String),
}

#[derive(Debug, Serialize)]
enum Status {
    Configing,
    Installing { step: u8, progress: u8 },
    Done
}

fn main() -> Result<()> {
    let mut result = InstallConfig::default();
    let mut status = Status::Configing;

    loop {
        let mut buf = String::new();
        let _ = stdin().read_line(&mut buf);
        let s: Reqeust = serde_json::from_str(&buf)?;

        match s.t {
            ReqeustType::GetStatus => {
                let s = serde_json::to_string(&status)?;
                println!("{s}");
            }
            ReqeustType::SetTarball(s) => {
                let tar = match s.as_str() {
                    "Base" => Tarball::Base,
                    "Server" => Tarball::Server,
                    "Desktop" => Tarball::Desktop,
                    x => bail!("Unsupported tarball: {x}"),
                };
                result.tarball = Some(tar);
                let s = serde_json::to_string(&result)?;
                println!("{s}");
            }
            ReqeustType::SetMirror(s) => {
                result.mirror = Some(s);
                let s = serde_json::to_string(&result)?;
                println!("{s}");
            }
        }
    }
}
