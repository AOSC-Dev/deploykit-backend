use std::io::stdin;
use eyre::Result;
use serde::Deserialize;


#[derive(Debug, Deserialize)]
struct Reqeust {
    t: ReqeustType,
}


#[derive(Debug, Deserialize)]
enum ReqeustType {
    GetStatus
}

fn main() -> Result<()> {
    loop {
        let mut buf = String::new();
        let _ = stdin().read_line(&mut buf);
        let s: Reqeust = serde_json::from_str(&buf)?;
        
        match s.t {
            ReqeustType::GetStatus => {
                dbg!("Get status");
            }
        }
    }
}
