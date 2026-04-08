mod cli;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("squadreplay: {error}");
        std::process::exit(1);
    }
}
