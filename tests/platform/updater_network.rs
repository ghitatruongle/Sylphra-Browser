use sylphra::updater::{fetch_manifest, parse_manifest, update_source_url};

#[test]
fn parse_manifest_rejects_malformed_json() {
    assert!(parse_manifest(b"{ not json").is_err());
    assert!(parse_manifest(b"{}").is_err());
    assert!(parse_manifest(b"[]").is_err());
}

#[test]
fn parse_manifest_rejects_incomplete_manifests() {
    let missing_fields = br#"{"version":"1.0.0"}"#;
    assert!(parse_manifest(missing_fields).is_err());
}

#[test]
fn fetch_manifest_rejects_non_https_remote_sources() {
    let error = fetch_manifest("http://example.com/manifest.json").unwrap_err();
    assert!(format!("{error:?}").contains("HTTPS"));
}

#[test]
fn fetch_manifest_fails_fast_on_refused_loopback() {
    let result = fetch_manifest("http://127.0.0.1:9/manifest.json");
    assert!(
        result.is_err(),
        "refused loopback port must surface an error"
    );
}

#[test]
fn update_source_is_https() {
    assert!(update_source_url().starts_with("https://"));
}
