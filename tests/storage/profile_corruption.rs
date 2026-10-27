use rstest::rstest;
use sylphra::storage::StorageManager;

fn profile_dir(base: &std::path::Path) -> std::path::PathBuf {
    base.join("Profiles").join("profile")
}

fn corrupt_main_profile_file(base: &std::path::Path, contents: &[u8]) {
    let profile = profile_dir(base);
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("storage.json"), contents).unwrap();
}

#[rstest]
#[case::truncated_json(1, "{corrupt".as_bytes())]
#[case::empty_file(2, b"")]
#[case::wrong_shape(3, b"[]")]
#[case::null_bytes(4, b"\x00\x01\x02\x03")]
#[case::future_schema(5, br#"{"schema_version": 999, "version": "0.0.0"}"#)]
fn corrupted_storage_profiles_fail_closed_to_defaults(
    #[case] case_id: usize,
    #[case] contents: &[u8],
) {
    let root = std::env::temp_dir().join(format!(
        "sylphra_t2_corrupt_{}_{}",
        std::process::id(),
        case_id
    ));
    let _ = std::fs::remove_dir_all(&root);
    corrupt_main_profile_file(&root, contents);

    let manager = StorageManager::for_profile(&root, "profile").unwrap();
    assert_eq!(manager.cookie_count(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn oversized_storage_profile_is_refused_without_panicking() {
    let root = std::env::temp_dir().join(format!("sylphra_t2_oversize_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let oversized = vec![b'a'; 65 * 1024 * 1024];
    corrupt_main_profile_file(&root, &oversized);

    let manager = StorageManager::for_profile(&root, "profile").unwrap();
    assert_eq!(manager.cookie_count(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn backup_recovers_when_main_profile_file_is_corrupt() {
    let root = std::env::temp_dir().join(format!("sylphra_t2_bak_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    let mut manager = StorageManager::for_profile(&root, "profile").unwrap();
    manager
        .local_storage("https://example.com")
        .unwrap()
        .set("phase", "first");
    manager.save();
    manager
        .local_storage("https://example.com")
        .unwrap()
        .set("phase", "second");
    manager.save();
    drop(manager);

    corrupt_main_profile_file(&root, b"{corrupt");

    let manager = StorageManager::for_profile(&root, "profile").unwrap();
    let restored = manager
        .get_local_storage("https://example.com")
        .and_then(|storage| storage.get("phase").cloned());
    assert_eq!(restored.as_deref(), Some("second"));
    let _ = std::fs::remove_dir_all(&root);
}
