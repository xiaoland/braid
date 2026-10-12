mod agent_session;
mod builtin;
mod cli;
mod collaboration;
mod config;
mod console;
mod context;
mod doctor;
mod github;
mod group;
mod health;
mod home;
mod local;
mod outbox;
mod platform;
mod producer;
mod protocol;
mod provider;
mod queue;
mod run;
mod runtime;
mod setup;
mod store;
mod telemetry;
mod tunnel;
mod webhook;
mod worktree;
mod writer;

#[tokio::main]
async fn main() {
    if let Err(error) = Box::pin(cli::run()).await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
