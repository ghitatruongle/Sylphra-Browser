use sylphra::adblock::{AdBlockConfig, AdBlocker, ResourceType};

fn blocker_with_easylist() -> AdBlocker {
    let mut blocker = AdBlocker::new(AdBlockConfig {
        enabled: true,
        ..Default::default()
    });
    blocker.load_easylist(
        "! EasyList sample\n\
         ||doubleclick.net^\n\
         ||adsrv.example.net^$script\n\
         @@||good.example.com^\n\
         ||tracked.example.org^$third-party\n\
         ||served.example.com^$domain=page.com|other.com\n\
         banana-rule\n",
    );
    blocker
}

#[test]
fn easylist_host_anchors_block_matching_hosts() {
    let mut blocker = blocker_with_easylist();
    assert!(blocker.should_block_resource(
        "https://ad.doubleclick.net/px.js",
        Some("https://news.com/article"),
        ResourceType::Script,
    ));
    assert!(!blocker.should_block_resource(
        "https://news.com/px.js",
        Some("https://news.com/article"),
        ResourceType::Script,
    ));
}

#[test]
fn easylist_resource_options_scope_matches() {
    let mut blocker = blocker_with_easylist();
    assert!(blocker.should_block_resource(
        "https://adsrv.example.net/t.js",
        Some("https://news.com"),
        ResourceType::Script,
    ));
    assert!(!blocker.should_block_resource(
        "https://adsrv.example.net/t.png",
        Some("https://news.com"),
        ResourceType::Image,
    ));
}

#[test]
fn easylist_exceptions_allow_listed_hosts() {
    let mut blocker = blocker_with_easylist();
    assert!(!blocker.should_block_resource(
        "https://good.example.com/track.js",
        Some("https://news.com"),
        ResourceType::Script,
    ));
}

#[test]
fn easylist_third_party_option_applies() {
    let mut blocker = blocker_with_easylist();
    assert!(blocker.should_block_resource(
        "https://tracked.example.org/t.js",
        Some("https://news.com"),
        ResourceType::Script,
    ));
}

#[test]
fn easylist_domain_option_scopes_to_pages() {
    let mut blocker = blocker_with_easylist();
    assert!(blocker.should_block_resource(
        "https://served.example.com/ad.js",
        Some("https://page.com"),
        ResourceType::Script,
    ));
    assert!(!blocker.should_block_resource(
        "https://served.example.com/ad.js",
        Some("https://unrelated.com"),
        ResourceType::Script,
    ));
}

#[test]
fn easylist_rejects_unknown_patterns_without_failing_the_set() {
    let mut blocker = blocker_with_easylist();
    assert!(!blocker.should_block_resource(
        "https://banana-rule.example.com/x.js",
        Some("https://news.com"),
        ResourceType::Script,
    ));
}
