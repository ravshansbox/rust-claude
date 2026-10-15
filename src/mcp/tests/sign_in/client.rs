use super::*;

#[tokio::test]
async fn signs_in_with_a_client_id_from_the_config() {
    let (url, requests) = oauth_server().await;
    let mut config = http_config(&url);
    config.oauth = Some(OAuthConfig {
        client_id: Some("own".into()),
        ..OAuthConfig::default()
    });
    let sign_in = begin_sign_in_with("web", &config).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["client_id"], "own");
    assert!(requests_for_path(&requests, "/register").is_empty());
}

#[tokio::test]
async fn does_not_save_the_client_secret_from_the_config() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = oauth_server().await;
    let mut config = http_config(&url);
    config.oauth = Some(OAuthConfig {
        client_id: Some("own".into()),
        client_secret: Some("own-secret".into()),
    });
    let sign_in = begin_sign_in_with("web", &config).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    visit_callback(&authorize_url, "the-code", None).await;
    assert_eq!(finished.await.unwrap(), Ok(()));
    assert_eq!(
        requests_for_path(&requests, "/token")[0].form()["client_secret"],
        "own-secret"
    );
    let file = std::fs::read_to_string(config::dir().unwrap().join("mcp-auth.json")).unwrap();
    assert!(!file.contains("own-secret"), "{file}");
}
