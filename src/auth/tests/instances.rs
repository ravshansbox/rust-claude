use super::*;

#[tokio::test]
async fn running_copies_renew_one_at_a_time() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", false).await;
    let path =
        credentials_file(&json!({ "access": "old", "refresh": "first", "expires": 0 }).to_string());
    let http = reqwest::Client::new();
    let (mut one, mut other) = (expired("first"), expired("first"));
    let (token, other_token) = tokio::join!(one.access_token(&http), other.access_token(&http));
    let _ = std::fs::remove_file(&path);
    assert_eq!(*endpoint.received.lock().await, ["first"]);
    assert_eq!(token.unwrap(), "access from first");
    assert_eq!(other_token.unwrap(), "access from first");
}

#[tokio::test]
async fn uses_a_sign_in_another_instance_renewed_after_startup() {
    let _lock = AUTH_FILE.lock().await;
    let path = credentials_file(
        &json!({ "access": "renewed", "refresh": "new", "expires": 4_102_444_800_000u64 })
            .to_string(),
    );
    let read_at_startup = json!({ "access": "old", "refresh": "spent", "expires": 0 }).to_string();
    let credentials = Credentials::load_and_refresh(&read_at_startup, &unreachable_client()).await;
    let _ = std::fs::remove_file(&path);
    assert_eq!(credentials.unwrap().access, "renewed");
}

#[tokio::test]
async fn uses_a_sign_in_without_expiry_another_instance_renewed() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("spent", false).await;
    let renewed = json!({ "access": "renewed", "refresh": "new", "expires": u64::MAX });
    let path = credentials_file(&renewed.to_string());
    let mut credentials: Credentials =
        serde_json::from_value(json!({ "access": "old", "refresh": "spent", "expires": u64::MAX }))
            .unwrap();
    let token = credentials
        .renew_rejected(&reqwest::Client::new(), "old")
        .await;
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(token.unwrap(), "renewed");
    assert!(endpoint.received.lock().await.is_empty());
    assert_eq!(saved, renewed);
}

#[tokio::test]
async fn saving_never_leaves_a_reader_with_a_partial_sign_in() {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    let _lock = AUTH_FILE.lock().await;
    let old = json!({ "access": "old", "refresh": "old", "expires": 1 }).to_string();
    let path = credentials_file(&old);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut reader = std::fs::File::open(&path).unwrap();

    let new: Credentials =
        serde_json::from_value(json!({ "access": "new", "refresh": "new", "expires": 2 })).unwrap();
    new.save().unwrap();

    let mut seen_by_reader = String::new();
    reader.read_to_string(&mut seen_by_reader).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let saved: Credentials =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let leftovers = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            name.to_string_lossy().starts_with("auth.json.")
        })
        .count();
    let _ = std::fs::remove_file(&path);
    assert_eq!(seen_by_reader, old);
    assert_eq!(mode, 0o600);
    assert_eq!((saved.access.as_str(), saved.expires), ("new", 2));
    assert_eq!(leftovers, 0);
}
