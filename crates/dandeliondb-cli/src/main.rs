mod commands;
mod shell;

fn main() {
    if let Err(error) = commands::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
