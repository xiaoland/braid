mod agent_session;
mod cli;
mod config;
mod context;
mod doctor;
mod github;
mod protocol;
mod provider;
mod runtime;
mod setup;
mod store;
mod telemetry;
mod tunnel;
mod webhook;
mod worker;
mod worktree;
mod writer;

#[tokio::main]
async fn main() {
    if let Err(error) = Box::pin(cli::run()).await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
