use sylphra::media_backend::DecodedMediaAsset;
use sylphra::media_budget::{
    MAX_AUDIO_DECODED_BYTES, MAX_DECODED_BYTES, MAX_INPUT_BYTES, MAX_VIDEO_DECODED_BYTES,
};
use sylphra::media_core::{DecodedAudioFrame, DecodedVideoFrame};
use sylphra::media_runtime::{BoundedStreamingDecoder, MediaOutputPipeline, MediaRuntimeLimits};

#[test]
fn shared_constants_keep_the_documented_values() {
    let limits = MediaRuntimeLimits::default();
    assert_eq!(limits.max_decoded_bytes, 64 * 1024 * 1024);
    assert_eq!(
        limits.max_decoded_bytes * 3 / 4 + limits.max_decoded_bytes / 4,
        limits.max_decoded_bytes
    );
}

#[test]
fn runtime_limits_default_to_the_shared_ceiling() {
    let limits = MediaRuntimeLimits::default();
    assert_eq!(limits.max_decoded_bytes, MAX_DECODED_BYTES);
}

#[test]
fn streaming_decoder_rejects_inputs_over_the_shared_ceiling() {
    assert!(
        BoundedStreamingDecoder::new(MAX_INPUT_BYTES + 1).is_err(),
        "budget above the shared ceiling must be rejected"
    );
    assert!(BoundedStreamingDecoder::new(0).is_err());
    assert!(BoundedStreamingDecoder::new(MAX_INPUT_BYTES).is_ok());

    let mut decoder = BoundedStreamingDecoder::new(MAX_INPUT_BYTES).unwrap();
    let error = decoder.append(&vec![0u8; MAX_INPUT_BYTES + 1]).unwrap_err();
    assert!(
        error.contains("budget exceeded"),
        "unexpected error: {error}"
    );
}

fn video_frame(width: u32, height: u32) -> DecodedVideoFrame {
    let pixels = width as usize * height as usize;
    DecodedVideoFrame {
        timestamp_us: 0,
        duration_us: 33_333,
        width,
        height,
        rgba: vec![0x80; pixels * 4],
    }
}

#[test]
fn pipeline_rejects_assets_that_cross_the_shared_decoded_ceiling() {
    let limits = MediaRuntimeLimits::default();

    let exact_share_frame = video_frame(4_096, 3_072);
    assert_eq!(exact_share_frame.rgba.len(), MAX_VIDEO_DECODED_BYTES);

    let at_limit = MediaOutputPipeline::from_asset(
        DecodedMediaAsset {
            video_frames: vec![exact_share_frame.clone()],
            audio_frames: Vec::new(),
        },
        limits,
    );
    assert!(
        at_limit.is_ok(),
        "an asset exactly at the ceiling must decode"
    );

    let over = MediaOutputPipeline::from_asset(
        DecodedMediaAsset {
            video_frames: vec![exact_share_frame, video_frame(64, 64)],
            audio_frames: Vec::new(),
        },
        limits,
    );
    let error = over.unwrap_err();
    assert!(error.contains("budget"), "unexpected error: {error}");
}

#[test]
fn backend_input_gate_uses_the_shared_encoded_input_ceiling() {
    let oversized = vec![0u8; MAX_INPUT_BYTES + 1];
    let error = sylphra::media_backend::decode_clear_content_bytes(&oversized).unwrap_err();
    assert!(
        error.contains("input budget"),
        "expected the early input-budget rejection, got: {error}"
    );
}

#[test]
fn audio_frames_respect_their_share_of_the_shared_ceiling() {
    let limits = MediaRuntimeLimits::default();
    let audio_share = limits.max_decoded_bytes - limits.max_decoded_bytes * 3 / 4;
    assert_eq!(audio_share, MAX_AUDIO_DECODED_BYTES);
    let asset = DecodedMediaAsset {
        video_frames: Vec::new(),
        audio_frames: vec![DecodedAudioFrame {
            timestamp_us: 0,
            duration_us: 21_333,
            channels: 2,
            sample_rate_hz: 48_000,
            interleaved_samples: vec![0i16; audio_share / 2 + 1],
        }],
    };
    assert!(MediaOutputPipeline::from_asset(asset, limits).is_err());
}
