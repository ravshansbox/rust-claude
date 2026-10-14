use super::{
    Started,
    config_file::{Configs, ServerConfig, enabled, http_url, uses_oauth},
    configs, oauth,
    server::start,
};
use crate::skills::Scope;

pub struct StartingServer {
    pub name: String,
    pub scope: Scope,
    pub started: futures::future::BoxFuture<'static, Started>,
}

pub struct Startup {
    pub diagnostics: Vec<String>,
    pub servers: Vec<StartingServer>,
    /// Global servers that project entries override.
    pub overridden: Vec<String>,
    /// HTTP servers that sign in with OAuth.
    pub sign_in_servers: Vec<SignInServer>,
}

#[derive(Clone)]
pub struct SignInServer {
    pub name: String,
    pub url: String,
}

impl SignInServer {
    /// Whether mcp-auth.json holds a sign-in for this server.
    pub fn signed_in(&self) -> bool {
        oauth::load(&self.url).is_some()
    }
}

fn starting(name: String, scope: Scope, config: ServerConfig) -> StartingServer {
    StartingServer {
        scope,
        started: Box::pin(start(name.clone(), scope, config)),
        name,
    }
}

/// Starts the configured server `name` again, as after signing in.
pub fn restart(name: &str) -> Option<StartingServer> {
    restart_from(configs().0, name)
}

pub(super) fn restart_from(mut configs: Configs, name: &str) -> Option<StartingServer> {
    let (scope, config) = configs.remove(name).filter(|(_, config)| enabled(config))?;
    Some(starting(name.to_string(), scope, config))
}

pub fn startup() -> Startup {
    let (configs, overridden, diagnostics) = configs();
    let enabled: Vec<_> = configs
        .into_iter()
        .filter(|(_, (_, config))| enabled(config))
        .collect();
    let sign_in_servers = enabled
        .iter()
        .filter(|(_, (_, config))| uses_oauth(config))
        .filter_map(|(name, (_, config))| {
            Some(SignInServer {
                name: name.clone(),
                url: http_url(config)?.to_string(),
            })
        })
        .collect();
    let servers = enabled
        .into_iter()
        .map(|(name, (scope, config))| starting(name, scope, config))
        .collect();
    Startup {
        diagnostics,
        servers,
        overridden,
        sign_in_servers,
    }
}
