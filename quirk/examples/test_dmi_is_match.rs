use std::{env::args, process::exit};

use quirk::{dmi_is_match, QuirkError};

fn main() -> Result<(), QuirkError> {
    let args = args().skip(1).collect::<Vec<_>>();

    if args.len() != 2 {
        eprintln!("Usage: DMI_PATTERN MODALIAS");
        exit(1);
    }

    let result = dmi_is_match(&args[1], &args[0])?;

    if result {
        println!("Match.");
    } else {
        println!("Not match.");
        exit(1);
    }

    Ok(())
}
