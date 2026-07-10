mod cli;
mod instance;
mod net;
mod server;
use clap::Parser;

fn main() {
    let args = cli::Args::parse();
    println!("{args:?}");
}
