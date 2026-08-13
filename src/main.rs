mod cli;
mod config;
mod context;
mod doctor;
mod github;
mod protocol;
mod runtime;
mod store;
mod telemetry;
mod tunnel;
mod webhook;

#[tokio::main]
async fn main() {
    if let Err(error) = Box::pin(cli::run()).await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
