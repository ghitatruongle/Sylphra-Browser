use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use sylphra::css_parser::parse_css;
use sylphra::image_loader::{decoded_buffer_bytes, fetch_and_decode_image};
use sylphra::ipc::{IpcChannel, IpcCommand, IpcMessage, RenderPart, IPC_VERSION};
use sylphra::javascript::JsvEngine;
use sylphra::memory_tracker::MemoryTracker;
use sylphra::parser::{parse_html, MAX_DOM_DEPTH};
use sylphra::process_architecture::{GenerationId, ProcessId};
use sylphra::resource_caps;
use sylphra::worker::{decode_wire_payload, MAX_WORKER_RESPONSE_BYTES};
use sylphra::Browser;

const MB: usize = 1024 * 1024;
const COMPRESSED_MAGIC: &[u8; 8] = b"GHZ10001";

fn repeated(unit: &str, count: usize) -> String {
    let mut output = String::with_capacity(unit.len().saturating_mul(count));
    for _ in 0..count {
        output.push_str(unit);
    }
    output
}

fn node_depth(element: &sylphra::parser::Element) -> usize {
    let mut stack = vec![(element, 1_usize)];
    let mut deepest = 1_usize;
    while let Some((node, depth)) = stack.pop() {
        deepest = deepest.max(depth);
        stack.extend(node.children.iter().map(|child| (child, depth + 1)));
    }
    deepest
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn an_oversized_body_declared_in_headers_is_refused_before_it_lands() {
    use sylphra::network_scheduler::{
        CancellationToken, NetworkScheduler, RequestPriority, ReqwestTransport, ResponseMode,
        ScheduledError, ScheduledRequest, SchedulerLimits,
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let address = listener.local_addr().expect("loopback address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let mut probe = [0_u8; 1_024];
        let _ = stream.read(&mut probe);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 200000000\r\n\r\n",
            )
            .expect("write headers");
        let mut sink = [0_u8; 64 * 1024];
        let mut forwarded = 0_usize;
        while let Ok(read) = stream.read(&mut sink) {
            if read == 0 {
                break;
            }
            forwarded += read;
        }
        forwarded
    });

    let scheduler = NetworkScheduler::new(
        ReqwestTransport::new().expect("transport"),
        SchedulerLimits {
            max_concurrency: 2,
            max_queued: 8,
            max_response_bytes: MB,
            request_timeout: Duration::from_secs(10),
        },
    )
    .expect("scheduler");
    let started = Instant::now();
    let response = scheduler
        .fetch(
            ScheduledRequest {
                id: 1,
                url: format!("http://{address}/huge"),
                cookie_header: String::new(),
                max_retries: 0,
                priority: RequestPriority::Navigation,
                response_mode: ResponseMode::Document,
            },
            CancellationToken::default(),
        )
        .await;
    let elapsed = started.elapsed();

    let ScheduledError::Transport(detail) = response.result.expect_err("must be refused") else {
        panic!("the oversized body must be refused as a budget violation");
    };
    assert!(
        detail.contains("budget"),
        "unexpected transport refusal: {detail}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the body must be refused from the headers, took {elapsed:?}"
    );
    drop(server);
}

#[test]
fn a_compressed_frame_cannot_announce_an_expansion_past_its_limit() {
    let announcement = {
        let mut frame = COMPRESSED_MAGIC.to_vec();
        frame.extend_from_slice(&((400 * MB) as u64).to_le_bytes());
        frame.extend_from_slice(&[0_u8; 64]);
        frame
    };
    let denied = decode_wire_payload(announcement, MAX_WORKER_RESPONSE_BYTES)
        .expect_err("a 400 MB expansion must be refused");
    assert!(
        denied.to_string().contains("expanded payload exceeds"),
        "unexpected error: {denied}"
    );

    let oversized_raw = vec![7_u8; MAX_WORKER_RESPONSE_BYTES + 1];
    let denied = decode_wire_payload(oversized_raw, MAX_WORKER_RESPONSE_BYTES)
        .expect_err("an oversized raw frame must be refused");
    assert!(
        denied.to_string().contains("payload exceeds"),
        "unexpected error: {denied}"
    );

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(repeated("<p>bomb</p>", 4_000).as_bytes())
        .expect("compress fixture");
    let compressed = encoder.finish().expect("finish");
    let mut liar = COMPRESSED_MAGIC.to_vec();
    liar.extend_from_slice(&1_200_000_usize.to_le_bytes());
    liar.extend_from_slice(&compressed);
    let denied = decode_wire_payload(liar, MAX_WORKER_RESPONSE_BYTES)
        .expect_err("a lying expansion length must be caught");
    assert!(
        denied.to_string().contains("length mismatch"),
        "unexpected error: {denied}"
    );
}

#[test]
fn a_two_hundred_thousand_node_dom_stops_at_the_node_cap() {
    let source = format!("<main>{}</main>", repeated("<p>row</p>", 250_000));
    assert!(source.len() < resource_caps::DOM_TRANSFER_TOTAL_BYTES * 2);
    let dom = parse_html(&source);
    let nodes = MemoryTracker::dom_node_count(&dom);
    assert!(
        nodes <= resource_caps::DOM_MAX_NODES + 2,
        "parsed {nodes} nodes past the cap"
    );
    assert!(nodes > resource_caps::DOM_MAX_NODES / 2);
    let estimate = MemoryTracker::estimate_dom(&dom);
    assert_eq!(estimate, nodes * resource_caps::DOM_NODE_ESTIMATE_BYTES);
    assert!(
        estimate <= (resource_caps::DOM_MAX_NODES + 2) * resource_caps::DOM_NODE_ESTIMATE_BYTES
    );
}

#[test]
fn an_unboundedly_nested_document_is_flattened_at_the_depth_cap() {
    let source = format!(
        "{}deep{}tail{}",
        repeated("<div>", 5_000),
        repeated("<span>", 500),
        repeated("</div>", 5_500)
    );
    let dom = parse_html(&source);
    let depth = node_depth(&dom);
    assert!(depth <= MAX_DOM_DEPTH, "built a DOM {depth} levels deep");
    assert!(depth > 10);
}

#[test]
fn an_oversized_stylesheet_is_cut_at_the_source_and_rule_caps() {
    let source = repeated("a{color:red;background:blue}", 200_000);
    assert!(source.len() > resource_caps::CSS_MAX_SOURCE_BYTES);
    let rules = parse_css(&source);
    assert_eq!(rules.len(), resource_caps::CSS_MAX_RULES);
    assert!(
        rules.iter().all(|rule| !rule.selectors.is_empty()),
        "a truncated stylesheet must stay well-formed"
    );
    let inside = parse_css(&repeated("b{color:red}", 500));
    assert_eq!(inside.len(), 500);
}

#[test]
fn a_script_beyond_the_source_cap_never_reaches_the_interpreter() {
    let mut engine = JsvEngine::new();
    let source = format!("var guard = 1;{}", repeated("guard += 1;", 250_000));
    assert!(source.len() > resource_caps::JS_MAX_SOURCE_BYTES);
    let denied = engine.eval(&source).expect_err("oversized source");
    assert!(denied.contains("2 MB"), "unexpected error: {denied}");
    assert!(engine.eval("var ok = 1 + 1; ok").is_ok());
}

#[test]
fn an_expanding_string_allocation_is_refused() {
    let mut engine = JsvEngine::new();
    let denied = engine
        .eval("let s = 'a'; while (true) { s = s + s }")
        .expect_err("string growth must be bounded");
    assert!(
        denied.contains("String too large") || denied.contains("budget"),
        "unexpected error: {denied}"
    );

    let mut engine = JsvEngine::new();
    let denied = engine
        .eval("'y'.repeat(4000000)")
        .expect_err("a single string beyond the per-string cap");
    assert!(
        denied.contains("String too large"),
        "unexpected error: {denied}"
    );

    let mut engine = JsvEngine::new();
    let denied = engine
        .eval("let a = []; let i = 0; while (i < 500) { a.push('x'.repeat(100000)); i = i + 1; } a.length")
        .expect_err("the aggregate string budget must bind");
    assert!(denied.contains("budget"), "unexpected error: {denied}");
}

#[test]
fn decoded_pixels_are_capped_before_a_buffer_exists() {
    assert_eq!(
        decoded_buffer_bytes(100, 100).expect("small image"),
        100 * 100 * resource_caps::IMAGE_BYTES_PER_PIXEL as usize
    );
    assert!(decoded_buffer_bytes(4_096, 4_096).is_ok());
    let denied = decoded_buffer_bytes(4_097, 4_097).expect_err("edge cap");
    assert!(denied.to_string().contains("edge limit"));
    let denied = decoded_buffer_bytes(20_000, 20_000).expect_err("pixel cap");
    assert!(
        denied.to_string().contains("edge limit"),
        "unexpected error: {denied}"
    );
}

fn bmp_declaring(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"BM".to_vec();
    bytes.extend_from_slice(&54_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&54_u32.to_le_bytes());
    bytes.extend_from_slice(&40_u32.to_le_bytes());
    bytes.extend_from_slice(&(width as i32).to_le_bytes());
    bytes.extend_from_slice(&(height as i32).to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&24_u16.to_le_bytes());
    bytes.extend_from_slice(&[0_u8; 24]);
    bytes
}

#[test]
fn a_giant_image_is_refused_before_its_bytes_are_decoded() {
    let directory = std::env::temp_dir().join(format!(
        "sylphra-image-{}-{}",
        std::process::id(),
        resource_caps::IMAGE_MAX_EDGE_PX
    ));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let path = directory.join("oversized.bmp");
    std::fs::write(&path, bmp_declaring(20_000, 20_000)).expect("write fixture");

    let url = format!("file://{}", path.display());
    let denied = fetch_and_decode_image(&url).expect_err("a 20000x20000 frame must be refused");
    assert!(
        denied.to_string().contains("edge limit"),
        "unexpected error: {denied}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn ipc_chunks_from_a_stale_generation_are_dropped() {
    let mut channel = IpcChannel::new(
        4_096,
        ProcessId(1),
        GenerationId(4),
        ProcessId(2),
        GenerationId(4),
    );
    let stale = IpcMessage {
        version: IPC_VERSION,
        sender_id: ProcessId(1),
        sender_generation: GenerationId(3),
        target_id: ProcessId(2),
        target_generation: GenerationId(3),
        sequence: 1,
        command: IpcCommand::RenderChunk {
            seq: 0,
            part: RenderPart::Html,
            text: "<script>stale()</script>".to_string(),
        },
    };
    let dropped = channel
        .deliver_incoming(stale)
        .expect_err("a stale generation must never be delivered");
    assert_eq!(dropped, "StaleGenerationMessageDropped");
    assert!(channel.receive().is_none());
    assert_eq!(channel.receive_queue.len(), 0);

    let future = IpcMessage {
        version: IPC_VERSION + 1,
        sender_id: ProcessId(1),
        sender_generation: GenerationId(4),
        target_id: ProcessId(2),
        target_generation: GenerationId(4),
        sequence: 2,
        command: IpcCommand::DomChunk {
            seq: 0,
            text: "{}".to_string(),
        },
    };
    let rejected = channel
        .deliver_incoming(future)
        .expect_err("an unknown protocol version must be refused");
    assert!(rejected.contains("IPC version mismatch"));
}

#[test]
fn an_oversized_snapshot_is_refused_before_the_writer_grows() {
    let mut browser = Browser::new_in_memory();
    let id = browser.add_tab(
        "https://snapshot.test/huge",
        parse_html(&repeated(
            "<p>payload row for the snapshot cap</p>",
            150_000,
        )),
        "Huge",
    );
    let directory =
        std::env::temp_dir().join(format!("sylphra-snapshot-cap-{}-{id}", std::process::id()));
    let live_nodes = MemoryTracker::dom_node_count(&browser.active_tab().unwrap().dom);
    let denied = browser
        .active_tab_mut()
        .unwrap()
        .spill_to_disk(&directory)
        .expect_err("a DOM beyond the RAM cap must not be serialised");
    assert!(denied.contains("byte cap"), "unexpected error: {denied}");
    assert!(
        !directory.exists(),
        "a refused snapshot must not touch the disk"
    );
    assert_eq!(
        MemoryTracker::dom_node_count(&browser.active_tab().unwrap().dom),
        live_nodes
    );
    assert!(!browser.active_tab().unwrap().is_disk_backed());
}

#[test]
fn every_boundary_keeps_its_commitment_inside_the_hard_limit() {
    let mut browser = Browser::new_in_memory();
    browser.add_tab(
        "https://boundaries.test/page",
        parse_html(&repeated("<p>bounded</p>", 20_000)),
        "Bounded",
    );
    browser.sync_tab_budgets();
    let hard = browser.resources.limits().hard_bytes;
    assert_eq!(hard, resource_caps::mb(resource_caps::GLOBAL_HARD_LIMIT_MB));
    let totals = browser.resources.totals();
    assert!(totals.committed_bytes + totals.reserved_bytes < hard);
    assert!(browser.pressure_status().consumed_bytes > 0);
    assert!(browser.measured_working_set_bytes() > 0);
    assert_eq!(browser.resource_denials(), 0);
}
