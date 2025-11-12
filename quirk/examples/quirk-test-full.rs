use std::{env, path::PathBuf};

use quirk::get_matches_quirk;

fn main() -> Result<(), String> {
	let error = false;
	let quirks_path = if let Ok(p) = env::var("QUIRKS_DIR") {
		PathBuf::from(p)
	} else {
		env::current_dir().unwrap().join("./data/quirks").canonicalize().unwrap()
	};
	if !quirks_path.is_dir() {
		return Err(format!("Path {} is not a directory.", quirks_path.display()));
	}
	eprintln!("Quirks path: {}", &quirks_path.display());
	let matched_quirks = get_matches_quirk(quirks_path);
	for quirk in matched_quirks {
		println!("Matched quirk: \n{:#?}", quirk);
		let script_path = PathBuf::from(quirk.command);
		if !script_path.exists() {
			eprintln!("Error: script '{}' does not exist.", script_path.display());
			continue;
		}
		if !script_path.is_file() {
			eprintln!("Error: script '{}' is not a file.", script_path.display());
			continue;
		}
	}
	if error {
		return Err("Error(s) encountered.".into());
	}
	Ok(())
}
