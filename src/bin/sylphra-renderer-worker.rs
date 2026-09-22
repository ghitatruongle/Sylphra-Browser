fn main() {
    if let Err(error) = sylphra::worker::run_worker_stdio() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
