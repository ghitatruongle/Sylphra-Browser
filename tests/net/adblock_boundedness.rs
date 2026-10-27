use sylphra::adblock::{AdBlockConfig, AdBlocker, ResourceType};

#[test]
fn ten_thousand_decisions_are_counted_and_bounded() {
    let mut blocker = AdBlocker::new(AdBlockConfig::default());
    for index in 0..10_000 {
        let url = format!("https://cdn{index}.other.test/assets/app.js");
        let _ = blocker.evaluate_resource(&url, Some("https://site.test"), ResourceType::Script);
    }
    assert_eq!(blocker.stats().evaluated_count, 10_000);
}
