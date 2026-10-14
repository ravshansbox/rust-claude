use super::*;

#[tokio::test]
async fn stays_quiet_unless_the_latest_release_is_a_newer_version() {
    for tag in ["v0.1.0", "v0.0.9", "v0.3.0-rc.1", "v0.3", "v0.3.0.1"] {
        let (base, _) = github(tag).await;
        let shown = run(updater(&base, None)).await;
        assert!(shown.is_empty(), "{tag}: {shown:?}");
    }
}

#[tokio::test]
async fn compares_versions_by_number() {
    let (base, _) = github("v0.10.0").await;
    let mut updater = updater(&base, None);
    updater.current_version = "0.9.0";
    assert_eq!(run(updater).await[0], "update v0.10.0 available");
}

#[tokio::test]
async fn skips_builds_from_main() {
    let (base, requests) = github("v0.2.0").await;
    let mut updater = updater(&base, None);
    updater.current_version = "0.0.0";
    assert_eq!(run(updater).await, Vec::<String>::new());
    assert_eq!(*requests.lock().unwrap(), Vec::<String>::new());
}

#[test]
fn can_be_turned_off_in_settings() {
    let settings = |check_for_updates| Settings {
        check_for_updates,
        ..Default::default()
    };
    assert!(Updater::new(&settings(Some(false)), false).is_none());
    assert!(Updater::new(&settings(Some(true)), false).is_some());
    assert!(Updater::new(&settings(None), false).is_some());
}

#[test]
fn skips_debug_builds_such_as_cargo_run() {
    let settings = Settings {
        check_for_updates: Some(true),
        ..Default::default()
    };
    assert!(Updater::new(&settings, true).is_none());
    if cfg!(debug_assertions) {
        assert!(Updater::for_this_build(&settings).is_none());
    }
}
