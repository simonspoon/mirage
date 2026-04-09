mod parser;
mod schema;
mod seeder;
mod server;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mirage", about = "Swagger 2.0 mock API server")]
struct Cli {
    /// Path to the Swagger spec file
    spec: PathBuf,

    /// Port to listen on
    #[arg(short, long, default_value_t = 3000)]
    port: u16,
}

fn main() {
    let _cli = Cli::parse();
}
