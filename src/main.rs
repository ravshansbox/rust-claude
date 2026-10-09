mod agent;
mod auth;
mod tools;
mod tui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let http = reqwest::Client::new();
    let credentials = auth::Credentials::load_or_login(&http).await?;
    let model = std::env::var("RUST_CLAUDE_MODEL").unwrap_or_else(|_| "claude-opus-5-5".into());
    tui::run(agent::Agent::new(http, credentials, model)).await
}
