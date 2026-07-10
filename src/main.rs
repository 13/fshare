mod cli;
mod net;
use clap::Parser;

fn main() {
    let args = cli::Args::parse();
    println!("{args:?}");
}
