fn main() {
    if hangar_mutation::run_elevated_helper_cli(std::env::args_os().skip(1)).is_err() {
        // Do not echo command-line values, capabilities, paths, handles, nonces
        // or key material from the elevated boundary.
        eprintln!("Code Hangar elevated helper refused the one-shot invocation");
        std::process::exit(1);
    }
}
