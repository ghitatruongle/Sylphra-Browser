use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

use sylphra::compatibility_diagnostics::{
    build_compatibility_report, count_layout_boxes, detect_blank_content, evaluate_layout_overlap,
    redact_sensitive_url, verify_acceptance_manifest, AcceptanceManifest, CompatibilityStatus,
    MediaTelemetry,
};
use sylphra::css_parser::parse_css;
use sylphra::layout::create_layout_tree;
use sylphra::media_backend::{
    CodecCapability, DecodedMediaAsset, DecoderCapabilities, DecoderProvider,
};
use sylphra::media_core::{DecodedAudioFrame, DecodedVideoFrame, MediaCodec};
use sylphra::parser::parse_html;
use sylphra::tab::TabManager;
use sylphra::web_runtime::PageRuntime;
use sylphra::youtube::{
    select_playback_plan, LiveYouTubeController, LiveYouTubePlayback, YouTubeKeyboardAction,
    YouTubePlayerResponse, YouTubeShell,
};

fn mock_decoder_capabilities() -> DecoderCapabilities {
    DecoderCapabilities {
        codecs: vec![
            CodecCapability {
                codec: MediaCodec::Avc,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Aac,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Vp9,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Opus,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Av1,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Pcm,
                available: true,
                provider: DecoderProvider::BrowserPcm,
            },
        ],
        probe_error: None,
    }
}

#[test]
fn test_acceptance_manifest_and_sha256_integrity() {
    let manifest_str = std::fs::read_to_string("tests/fixtures/acceptance_manifest.json")
        .expect("read acceptance manifest");
    let manifest: AcceptanceManifest =
        serde_json::from_str(&manifest_str).expect("parse acceptance manifest json");

    assert_eq!(manifest.schema_version, 1);
    assert!(!manifest.fixtures.is_empty());
    assert!(manifest.fixtures.len() >= 10);

    verify_acceptance_manifest(&manifest, Path::new("."))
        .expect("acceptance manifest integrity verified");
}

#[test]
fn test_wikipedia_real_site_layout_typography_and_zero_overlap() {
    let html_str = std::fs::read_to_string("tests/fixtures/layout/wikipedia_layout_sample.html")
        .expect("read wikipedia html fixture");
    let css_str = std::fs::read_to_string("tests/fixtures/css/wikipedia_sample.css")
        .expect("read wikipedia css fixture");

    let css_rules = parse_css(&css_str);
    assert!(!css_rules.is_empty(), "CSS rules parsed");

    let dom = parse_html(&html_str);
    let layout_opt = create_layout_tree(&dom, &css_rules, 1200);
    assert!(layout_opt.is_some(), "Layout tree created");

    let layout = layout_opt.unwrap();
    assert!(layout.rect.width >= 1200.0);
    assert!(layout.rect.height > 100.0);

    let (_overlapping_boxes, collision_score) = evaluate_layout_overlap(&layout);
    assert!(
        collision_score < 0.05,
        "Wikipedia layout collision score must be near zero, got: {collision_score}"
    );

    let blank_ratio = detect_blank_content(Some(&layout), 1200.0, 800.0);
    assert!(
        blank_ratio < 0.95,
        "Wikipedia content must be meaningfully rendered, blank ratio: {blank_ratio}"
    );

    let total_boxes = count_layout_boxes(&layout);
    assert!(
        total_boxes >= 10,
        "Layout tree must contain full article structure, found {total_boxes} boxes"
    );
}

#[test]
fn test_wikipedia_multi_viewport_responsive_reflow() {
    let html_str = std::fs::read_to_string("tests/fixtures/layout/wikipedia_layout_sample.html")
        .expect("read wikipedia html fixture");
    let css_str = std::fs::read_to_string("tests/fixtures/css/wikipedia_sample.css")
        .expect("read wikipedia css fixture");

    let css_rules = parse_css(&css_str);
    let dom = parse_html(&html_str);

    for &viewport_width in &[800, 1200, 1600] {
        let layout_opt = create_layout_tree(&dom, &css_rules, viewport_width);
        assert!(
            layout_opt.is_some(),
            "Layout created for viewport {viewport_width}"
        );

        let layout = layout_opt.unwrap();
        assert!(layout.rect.width >= viewport_width as f64);
        assert!(layout.rect.height > 0.0);
        assert!(layout.rect.x >= 0.0);
        assert!(layout.rect.y >= 0.0);

        let (overlapping_boxes, collision_score) = evaluate_layout_overlap(&layout);
        assert!(
            collision_score < 0.05,
            "Collision score at {viewport_width}px must remain below threshold: {collision_score} (pairs: {overlapping_boxes})"
        );
    }
}

#[test]
fn test_youtube_spa_and_degraded_shell_interaction_gate() {
    let spa_html = std::fs::read_to_string("tests/fixtures/spa/youtube_spa_sample.html")
        .expect("read youtube spa fixture");
    let mut runtime =
        PageRuntime::from_html(&spa_html, Vec::new(), 1200, "https://www.youtube.com/")
            .expect("runtime init succeeds");

    let _ = runtime.run_document();
    let _ = runtime.pump_events(50);

    let report = runtime.report();
    assert!(report.scripts_executed > 0, "SPA scripts must execute");
    assert!(
        report.dom_mutations > 0,
        "Custom element hydration must produce DOM mutations"
    );

    let search_json = std::fs::read_to_string("tests/fixtures/youtube/youtube_search_results.json")
        .expect("read search fixture");
    let search_val: Value = serde_json::from_str(&search_json).expect("parse search json");

    let shell = YouTubeShell::from_search_response("rust browser engine", &search_val)
        .expect("degraded search shell created");
    assert_eq!(shell.results.len(), 2);
    assert_eq!(shell.results[0].video_id, "vid001");
    assert!(!shell.results[0].title.is_empty());
}

#[test]
fn test_youtube_clear_media_decoding_and_pcm_output_gate() {
    let watch_json = std::fs::read_to_string("tests/fixtures/youtube/youtube_watch_clear.json")
        .expect("read clear watch fixture");
    let watch_val: Value = serde_json::from_str(&watch_json).expect("parse watch json");

    let player_response =
        YouTubePlayerResponse::from_value(&watch_val).expect("player response parsed");
    let caps = mock_decoder_capabilities();
    let plan = select_playback_plan(&player_response, &caps).expect("select playback plan");

    let video_frame = DecodedVideoFrame {
        timestamp_us: 0,
        duration_us: 40_000,
        width: 640,
        height: 360,
        rgba: vec![128u8; 640 * 360 * 4],
    };
    let audio_frame = DecodedAudioFrame {
        timestamp_us: 0,
        duration_us: 40_000,
        sample_rate_hz: 48_000,
        channels: 2,
        interleaved_samples: vec![1000i16; 48 * 40 * 2],
    };
    let asset = DecodedMediaAsset {
        video_frames: vec![video_frame],
        audio_frames: vec![audio_frame],
    };

    let playback = LiveYouTubePlayback {
        response: player_response,
        plan,
        decoded: Arc::new(asset),
        downloaded_bytes: 100_000,
    };

    let mut controller = LiveYouTubeController::new(playback).expect("create live controller");
    assert!(controller.controls().paused);

    controller
        .handle_keyboard_action(YouTubeKeyboardAction::TogglePlayPause)
        .unwrap();
    assert!(!controller.controls().paused);

    let tick_result = controller.tick(40).expect("tick succeeds");
    assert!(tick_result.video_frame_presented);
    assert_eq!(tick_result.audio_frames_emitted, 1);

    controller.set_volume(0.5).expect("set volume");
    let drained = controller.drain_audio_frames();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].interleaved_samples[0], 500);
}

#[test]
fn test_youtube_truthful_restricted_error_reporting_gate() {
    let restricted_json =
        std::fs::read_to_string("tests/fixtures/youtube/youtube_watch_restricted.json")
            .expect("read restricted watch fixture");
    let fixture: Value = serde_json::from_str(&restricted_json).expect("parse json");

    let err_unplayable = YouTubePlayerResponse::from_value(&fixture["unplayable"]).unwrap_err();
    assert!(err_unplayable.contains("UNPLAYABLE"));
    assert!(err_unplayable.contains("unavailable in your region"));

    let err_login = YouTubePlayerResponse::from_value(&fixture["login_required"]).unwrap_err();
    assert!(err_login.contains("LOGIN_REQUIRED"));
    assert!(err_login.contains("Sign in"));

    let err_age = YouTubePlayerResponse::from_value(&fixture["age_check_required"]).unwrap_err();
    assert!(err_age.contains("AGE_CHECK_REQUIRED"));
    assert!(err_age.contains("Age-restricted"));

    let err_cipher = YouTubePlayerResponse::from_value(&fixture["cipher_only"]).unwrap_err();
    assert!(err_cipher.contains("no direct clear-content formats"));
}

#[test]
fn test_compatibility_telemetry_redaction_and_failure_states() {
    let secret_url =
        "https://example.com/watch?v=12345&auth_token=supersecret123&session=xyz987&s=ciphersig";
    let redacted = redact_sensitive_url(secret_url);
    assert!(!redacted.contains("supersecret123"));
    assert!(!redacted.contains("xyz987"));
    assert!(redacted.contains("v=12345"));
    assert!(redacted.contains("REDACTED"));

    let report = build_compatibility_report(
        secret_url,
        None,
        None,
        None,
        Some(MediaTelemetry {
            route: Some("Watch".to_string()),
            display_mode: Some("SiteApplication".to_string()),
            is_playable: true,
            playability_status: Some("OK".to_string()),
            formats_count: 2,
            video_frames_count: 60,
            audio_frames_count: 60,
        }),
        1200.0,
        800.0,
    );

    assert_eq!(report.status, CompatibilityStatus::FullyCompatible);
    assert!(!report.url.contains("supersecret123"));
}

#[test]
fn test_multitab_memory_soak_and_deterministic_teardown() {
    let mut manager = TabManager::new();
    assert_eq!(manager.tab_count(), 0);

    let html = "<html><head><style>body { margin: 0; padding: 10px; color: #333; }</style></head><body><h1>Soak Test</h1><p>Running multi-tab lifecycle.</p><script>window.counter = 42;</script></body></html>";

    for cycle in 0..10 {
        let dom = parse_html(html);
        let tab_id = manager.add_tab(&format!("https://example.com/page_{cycle}"), dom, "Soak");
        assert!(manager.get_tab(tab_id).is_some());

        let tab = manager.get_tab_mut(tab_id).unwrap();
        tab.set_url(format!("https://example.com/page_{cycle}"));
        tab.init_runtime(Vec::new(), 1200, html)
            .expect("runtime init succeeds");
        let _ = tab.evaluate_js("window.test = true;");

        assert!(tab.can_sleep());
        tab.sleep();
        assert!(tab.is_sleeping);

        tab.wake();
        assert!(!tab.is_sleeping);

        tab.discard();
        assert!(tab.is_discarded);

        manager.remove_tab(tab_id);
    }

    assert_eq!(manager.tab_count(), 0);
}

#[test]
fn test_local_release_readiness_audit_evaluation() {
    let manifest_str = std::fs::read_to_string("tests/fixtures/acceptance_manifest.json")
        .expect("read acceptance manifest");
    let manifest: AcceptanceManifest =
        serde_json::from_str(&manifest_str).expect("parse acceptance manifest json");

    assert_eq!(
        manifest.track,
        "Real-Site Acceptance, Diagnostics, and Corrective Release Gates"
    );
    assert!(manifest.fixtures.len() >= 10);
    verify_acceptance_manifest(&manifest, Path::new(".")).expect("manifest verification passes");
}

#[test]
fn unsupported_runtime_keeps_readable_page_text() {
    let html = sylphra::ui::build_readable_fallback(
        "https://app.test/?token=secret",
        "App",
        "unsupported hydration",
        "Account overview and recent activity",
    );
    assert!(html.contains("Account overview"));
    assert!(html.contains("unsupported hydration"));
    assert!(html.contains("https://app.test/"));
    assert!(
        !html.contains("token=secret"),
        "fallback must redact URL secrets"
    );
}
