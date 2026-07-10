use clap::Parser;
use fshare::cli;

fn main() {
    let args = cli::Args::parse();
    println!("{args:?}");
}
