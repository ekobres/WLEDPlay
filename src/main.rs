use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    wledplay::app::run_with_cli(wledplay::app::Cli::parse()).await
}
