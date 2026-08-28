use std::path::PathBuf;

use hangar_mutation::verify_release_installation;

fn main() {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(directory) = args.next() else {
        eprintln!("usage: code-hangar-release-verify <signed-install-directory>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: code-hangar-release-verify <signed-install-directory>");
        std::process::exit(2);
    }

    match verify_release_installation(&PathBuf::from(directory)) {
        Ok(proof) => match serde_json::to_string(&proof) {
            Ok(encoded) => println!("{encoded}"),
            Err(_) => {
                eprintln!("release verification succeeded but its proof could not be encoded");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("release verification failed: {error}");
            std::process::exit(1);
        }
    }
}
