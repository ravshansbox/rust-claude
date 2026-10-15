mod challenge;

use challenge::{challenge, challenge_parameter, requested_scope};
use serde_json::Value;

/// What the sign-in server needs, found from the MCP server.
pub(super) struct Discovered {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub resource: String,
    pub scope: Option<String>,
}

/// `{origin}/.well-known/{name}{path}`, as RFC 8414 and RFC 9728 place
/// metadata for a URL with a path.
fn well_known(url: &reqwest::Url, name: &str) -> String {
    let path = url.path().trim_end_matches('/');
    format!(
        "{}/.well-known/{name}{path}",
        url.origin().ascii_serialization()
    )
}

async fn first_json(
    client: &reqwest::Client,
    urls: &[String],
    check: impl Fn(&Value) -> bool,
) -> Option<Value> {
    for url in urls {
        let request = client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Ok(response) = request.send().await
            && response.status().is_success()
            && let Ok(metadata) = response.json::<Value>().await
            && check(&metadata)
        {
            return Some(metadata);
        }
    }
    None
}

pub(super) async fn discover(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<Discovered, String> {
    if !secure_endpoint(server_url) {
        return Err(format!("the MCP server does not use HTTPS: {server_url}"));
    }
    let url = reqwest::Url::parse(server_url).map_err(|error| error.to_string())?;
    let challenge = challenge(client, server_url).await?;
    let origin = url.origin().ascii_serialization();
    let resource_urls: Vec<String> = challenge_parameter(&challenge, "resource_metadata")
        .into_iter()
        .chain([
            well_known(&url, "oauth-protected-resource"),
            format!("{origin}/.well-known/oauth-protected-resource"),
        ])
        .collect();
    let resource_metadata = first_json(client, &resource_urls, |metadata| {
        metadata["authorization_servers"][0].is_string()
    })
    .await;
    let issuer = match &resource_metadata {
        Some(metadata) => metadata["authorization_servers"][0]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        None => origin,
    };
    let resource = resource_metadata
        .as_ref()
        .and_then(|metadata| metadata["resource"].as_str());
    if let Some(resource) = resource
        && !covers(resource, &url)
    {
        return Err(format!(
            "the sign-in metadata of {server_url} is for another server: {resource}"
        ));
    }
    let server_metadata = find_server_metadata(client, &issuer, server_url).await?;
    if resource_metadata.is_some()
        && let Some(named) = server_metadata["issuer"].as_str()
        && named.trim_end_matches('/') != issuer.trim_end_matches('/')
    {
        return Err(format!(
            "the sign-in server {issuer} names another issuer: {named}"
        ));
    }
    for kind in ["authorization", "token"] {
        let endpoint = server_metadata[format!("{kind}_endpoint")]
            .as_str()
            .unwrap_or_default();
        if !secure_endpoint(endpoint) {
            return Err(format!(
                "the sign-in server's {kind} endpoint does not use HTTPS: {endpoint}"
            ));
        }
    }
    let text = |value: &Value| value.as_str().map(str::to_string);
    let scope = requested_scope(&challenge, resource_metadata.as_ref());
    Ok(Discovered {
        authorization_endpoint: text(&server_metadata["authorization_endpoint"])
            .unwrap_or_default(),
        token_endpoint: text(&server_metadata["token_endpoint"]).unwrap_or_default(),
        registration_endpoint: text(&server_metadata["registration_endpoint"]),
        resource: resource.unwrap_or(server_url).to_string(),
        scope,
    })
}

async fn find_server_metadata(
    client: &reqwest::Client,
    issuer: &str,
    server_url: &str,
) -> Result<Value, String> {
    let issuer_url = reqwest::Url::parse(issuer)
        .map_err(|error| format!("invalid sign-in server {issuer:?}: {error}"))?;
    let mut metadata_urls = vec![
        well_known(&issuer_url, "oauth-authorization-server"),
        well_known(&issuer_url, "openid-configuration"),
    ];
    if !issuer_url.path().trim_end_matches('/').is_empty() {
        metadata_urls.push(format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        ));
    }
    first_json(client, &metadata_urls, |metadata| {
        metadata["authorization_endpoint"].is_string() && metadata["token_endpoint"].is_string()
    })
    .await
    .ok_or_else(|| format!("cannot find the sign-in server for {server_url}"))
}

/// Whether `resource` is `url`, or its origin or a parent path of it.
fn covers(resource: &str, url: &reqwest::Url) -> bool {
    let Ok(resource) = reqwest::Url::parse(resource) else {
        return false;
    };
    let parent = resource.path().trim_end_matches('/');
    let path = url.path();
    resource.origin() == url.origin()
        && (path.trim_end_matches('/') == parent
            || path
                .strip_prefix(parent)
                .is_some_and(|rest| rest.starts_with('/')))
}

fn secure_endpoint(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match url.scheme() {
        "https" => true,
        "http" => {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        }
        _ => false,
    }
}
