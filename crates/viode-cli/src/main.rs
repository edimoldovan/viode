fn main() {
    if let Err(e) = viode_cli::run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
