use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sylphra::{
    adblock::{AdBlockConfig, AdBlocker, ResourceType},
    css_parser,
    javascript::JsvEngine,
    layout, paint, parser,
    resource_ledger::{LedgerLimits, OwnerId, ResourceLedger, SubsystemId},
    string_pool::InternedString,
    web_runtime, Browser,
};

const REPEATED_NAMES: [&str; 8] = [
    "div",
    "span",
    "section",
    "article",
    "class",
    "href",
    "style",
    "aria-label",
];

const SAMPLE_HTML: &str = r#"
<html><head><title>Benchmark</title><style>
body { color: #202124; background: white; }
.card { margin: 8px; padding: 12px; border: 1px solid #dadce0; }
</style></head><body>
<main><h1>Sylphra</h1>
<div class="card"><p>A bounded document rendering benchmark.</p>
<a href="https://example.com">Example link</a></div>
</main></body></html>
"#;

fn benchmark_document_pipeline(c: &mut Criterion) {
    c.bench_function("parse_layout_paint", |b| {
        b.iter(|| {
            let dom = parser::parse_html(black_box(SAMPLE_HTML));
            let rules = css_parser::parse_css("body { font-size: 16px; } .card { padding: 12px; }");
            let layout = layout::create_layout_tree(&dom, &rules, 1100)
                .expect("sample document must produce a layout tree");
            black_box(paint::build_display_list(&layout));
        });
    });
}

fn benchmark_javascript(c: &mut Criterion) {
    c.bench_function("javascript_bounded_loop", |b| {
        b.iter(|| {
            let mut engine = JsvEngine::new();
            black_box(
                engine
                    .eval("let i = 0; while (i < 100) { i = i + 1; } i")
                    .expect("benchmark script must evaluate"),
            );
        });
    });
}

fn benchmark_runtime_and_filtering(c: &mut Criterion) {
    c.bench_function("dom_runtime_host_bridge", |b| {
        b.iter(|| {
            let mut dom = parser::parse_html(
                "<p id='status'>old</p><script>document.getElementById('status').textContent='ready';localStorage.setItem('theme','dark');fetch('/data');</script>",
            );
            black_box(web_runtime::run_inline_scripts(
                &mut dom,
                "https://example.test/page",
            ));
        });
    });

    c.bench_function("request_filter_1000", |b| {
        b.iter(|| {
            let mut blocker = AdBlocker::new(AdBlockConfig::default());
            for index in 0..1_000 {
                black_box(blocker.should_block_resource(
                    &format!("https://cdn{index}.example.test/assets/app.js"),
                    Some("news.example.test"),
                    ResourceType::Script,
                ));
            }
        });
    });
}

fn benchmark_resource_governance(c: &mut Criterion) {
    c.bench_function("ledger_reserve_release", |b| {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        b.iter(|| {
            for index in 0..64_usize {
                let owner = OwnerId::Tab(index % 8);
                let mut reservation = ledger
                    .reserve(SubsystemId::DomTree, owner, 4_096)
                    .expect("inside the dom ceiling");
                reservation.commit_bytes(black_box(2_048));
                black_box(reservation.held_bytes());
                drop(reservation);
                black_box(ledger.release_owner(owner));
            }
            black_box(ledger.totals().committed_bytes)
        });
    });

    c.bench_function("estimate_1000_tabs", |b| {
        let mut browser = Browser::new_in_memory();
        let fixture = format!("<main>{}</main>", "<p>bench row</p>".repeat(200));
        let dom = parser::parse_html(&fixture);
        for index in 0..1_000 {
            browser.add_tab(
                &format!("https://bench{index}.test/page"),
                dom.clone(),
                "Bench",
            );
        }
        b.iter(|| black_box(browser.estimate_memory().total_bytes));
    });

    c.bench_function("intern_repeated_names", |b| {
        b.iter(|| {
            let mut total = 0_usize;
            for _ in 0..256 {
                for name in REPEATED_NAMES {
                    total += InternedString::new(name).len();
                }
            }
            black_box(total)
        });
    });

    c.bench_function("clone_repeated_names", |b| {
        let owned: Vec<String> = REPEATED_NAMES.iter().map(|name| name.to_string()).collect();
        b.iter(|| {
            let mut total = 0_usize;
            for _ in 0..256 {
                for name in &owned {
                    total += name.clone().len();
                }
            }
            black_box(total)
        });
    });
}

criterion_group!(
    benches,
    benchmark_document_pipeline,
    benchmark_javascript,
    benchmark_runtime_and_filtering,
    benchmark_resource_governance
);
criterion_main!(benches);
