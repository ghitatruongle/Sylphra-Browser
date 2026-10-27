use sylphra::media_backend::{CodecCapability, DecoderCapabilities, DecoderProvider};
use sylphra::media_core::MediaCodec;
use sylphra::media_runtime::{MediaRuntimeLimits, PageMediaRuntime};
use sylphra::runtime_core::{RuntimeLimits, RuntimeRealm};

fn av_capabilities() -> DecoderCapabilities {
    DecoderCapabilities {
        codecs: vec![CodecCapability {
            codec: MediaCodec::Avc,
            available: true,
            provider: DecoderProvider::WindowsMediaFoundation,
        }],
        probe_error: None,
    }
}

#[test]
fn page_media_source_queue_budget_is_enforced_across_sources() {
    let realm = RuntimeRealm::new(2, RuntimeLimits::default()).unwrap();
    let limits = MediaRuntimeLimits {
        max_page_queued_bytes: 16,
        ..Default::default()
    };
    let mut runtime = PageMediaRuntime::new(realm, limits);
    let capabilities = av_capabilities();
    let source = runtime.create_media_source().unwrap();
    let buffer = runtime
        .add_source_buffer(
            source.id,
            "video/mp4; codecs=\"avc1.640028\"",
            &capabilities,
        )
        .unwrap();

    let error = runtime
        .append_buffer(source.id, buffer, &[0u8; 20])
        .unwrap_err();
    assert!(error.contains("budget"), "got: {error}");
}

#[test]
fn detached_and_attached_media_sources_share_one_page_budget() {
    let realm = RuntimeRealm::new(2, RuntimeLimits::default()).unwrap();
    let limits = MediaRuntimeLimits {
        max_page_queued_bytes: 16,
        ..Default::default()
    };
    let mut runtime = PageMediaRuntime::new(realm, limits);
    let capabilities = av_capabilities();
    let source = runtime.create_media_source().unwrap();
    let buffer = runtime
        .add_source_buffer(
            source.id,
            "video/mp4; codecs=\"avc1.640028\"",
            &capabilities,
        )
        .unwrap();

    let element = runtime.create_media_element().unwrap();
    runtime.attach_source(element.id, source.id).unwrap();

    let error = runtime
        .append_buffer(source.id, buffer, &[0u8; 20])
        .unwrap_err();
    assert!(error.contains("budget"), "got: {error}");
}
