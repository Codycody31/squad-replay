#[cfg(not(feature = "cli"))]
compile_error!("The `cli` feature is required to build the squadreplay binary. Enable it with `--features cli` or use default features.");

mod cli;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("squadreplay: {error}");
        std::process::exit(1);
    }
}
