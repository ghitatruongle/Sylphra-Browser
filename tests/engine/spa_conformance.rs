use sylphra::dom::get_element_by_id;
use sylphra::javascript::JsvValue;
use sylphra::memory_tracker::MemoryTracker;
use sylphra::parser::parse_html;
use sylphra::tab::Tab;
use sylphra::web_runtime::PageRuntime;

#[test]
fn test_tab_persistent_runtime_lifecycle() {
    let dom = parse_html("<div>Initial</div>");
    let mut tab = Tab::new(
        1,
        "https://example.com/app".to_string(),
        dom,
        "SPA App".to_string(),
    );
    assert!(tab.runtime.is_none());

    tab.init_runtime(Vec::new(), 800, "https://example.com/app")
        .unwrap();
    assert!(tab.runtime.is_some());

    let res1 = tab.evaluate_js("let count = 10; count * 2;");
    assert_eq!(res1, Ok(JsvValue::Number(20.0)));

    let res2 = tab.evaluate_js("count = count + 5; count;");
    assert_eq!(res2, Ok(JsvValue::Number(15.0)));

    let heap = tab.runtime_heap_bytes();
    assert!(heap > 0);

    tab.sleep();
    assert!(tab.runtime.is_none());
    assert_eq!(tab.runtime_heap_bytes(), 0);

    let _ = tab.wake();
    assert!(tab.runtime.is_none());

    tab.init_runtime(Vec::new(), 800, "https://example.com/app")
        .unwrap();
    let res3 = tab.evaluate_js("let count = 100; count;");
    assert_eq!(res3, Ok(JsvValue::Number(100.0)));
}

#[test]
fn test_memory_tracker_accounts_for_runtime_heap() {
    let dom = parse_html("<div>SPA Root</div>");
    let mut tab = Tab::new(
        2,
        "https://example.com/spa".to_string(),
        dom,
        "SPA".to_string(),
    );
    tab.init_runtime(Vec::new(), 800, "https://example.com/spa")
        .unwrap();
    tab.evaluate_js("let arr = [1, 2, 3, 4, 5];").unwrap();

    let estimate = MemoryTracker::estimate_tab(&tab);

    assert!(estimate.runtime_bytes > 0);
    assert!(estimate.total_bytes >= estimate.runtime_bytes);
}

#[test]
fn test_script_ordering_importmap_and_diagnostics() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <script type="importmap">
            {
                "imports": {
                    "app": "/app.js"
                }
            }
            </script>
            <script>
                document.title = "App Loaded";
            </script>
        </head>
        <body>
            <div id="root">Hello</div>
        </body>
        </html>
    "#;

    let mut runtime =
        PageRuntime::from_html(html, Vec::new(), 800, "https://example.com/").unwrap();
    runtime.run_document().unwrap();

    let title = runtime
        .dom_element()
        .find_tag("title")
        .map(|t| t.text.clone());
    assert_eq!(title.as_deref(), Some("App Loaded"));

    let report = runtime.report();
    assert!(report.scripts_seen >= 2);
    assert!(report
        .script_diagnostics
        .iter()
        .any(|d| d.script_type == "importmap" && d.status == "success"));
    assert!(report
        .script_diagnostics
        .iter()
        .any(|d| d.script_type == "classic" && d.status == "success"));
}

#[test]
fn test_framework_dom_class_list_and_dataset() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div id="item" class="primary bold" data-user-id="u42" data-is-active="true"></div>
            <script>
                let el = document.getElementById("item");
                el.classList.add("extra");
                el.classList.remove("bold");
                let hasPrimary = el.classList.contains("primary");
                let toggled = el.classList.toggle("hidden");
                el.dataset.role = "admin";
            </script>
        </body>
        </html>
    "#;

    let mut runtime =
        PageRuntime::from_html(html, Vec::new(), 800, "https://example.com/").unwrap();
    runtime.run_document().unwrap();

    let dom = runtime.dom_element();
    let item = get_element_by_id(&dom, "item").expect("item found");

    let class_str = item.get_attr("class").unwrap();
    assert!(class_str.contains("primary"));
    assert!(class_str.contains("extra"));
    assert!(class_str.contains("hidden"));
    assert!(!class_str.contains("bold"));

    assert_eq!(
        item.get_attr("data-role").map(String::as_str),
        Some("admin")
    );
}

#[test]
fn test_framework_dom_style_and_geometry() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div id="box" style="color: red;">Box</div>
            <script>
                let b = document.getElementById("box");
                b.style.backgroundColor = "blue";
                b.style.fontSize = "16px";
            </script>
        </body>
        </html>
    "#;

    let mut runtime =
        PageRuntime::from_html(html, Vec::new(), 800, "https://example.com/").unwrap();
    runtime.run_document().unwrap();

    let dom = runtime.dom_element();
    let box_el = get_element_by_id(&dom, "box").expect("box found");
    let style_str = box_el.get_attr("style").unwrap();

    assert!(style_str.contains("color: red") || style_str.contains("color:red"));
    assert!(style_str.contains("background-color: blue"));
    assert!(style_str.contains("font-size: 16px"));
}

#[test]
fn test_element_traversal_and_query_methods() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div id="container">
                <section class="card" id="card1">
                    <p class="desc">Card 1 Text</p>
                </section>
                <section class="card" id="card2">
                    <p class="desc">Card 2 Text</p>
                </section>
            </div>
            <script>
                let desc = document.querySelector(".desc");
                let closestSection = desc.closest("section");
                let isMatch = desc.matches("p.desc");
                let allCards = document.querySelectorAll(".card");
            </script>
        </body>
        </html>
    "#;

    let mut runtime =
        PageRuntime::from_html(html, Vec::new(), 800, "https://example.com/").unwrap();
    runtime.run_document().unwrap();

    let match_res = runtime
        .evaluate("document.querySelector('.desc').matches('p.desc')")
        .unwrap();
    assert_eq!(match_res, JsvValue::Boolean(true));

    let closest_id = runtime
        .evaluate("document.querySelector('.desc').closest('section').id")
        .unwrap();
    assert_eq!(closest_id, JsvValue::String("card1".to_string()));
}

#[test]
fn test_animation_frame_event_loop() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div id="anim">Init</div>
            <script>
                let frameFired = false;
                let rafId = window.requestAnimationFrame(function(ts) {
                    let el = document.getElementById("anim");
                    el.textContent = "FrameFired";
                });
            </script>
        </body>
        </html>
    "#;

    let mut runtime =
        PageRuntime::from_html(html, Vec::new(), 800, "https://example.com/").unwrap();
    runtime.run_document().unwrap();

    let text_before = get_element_by_id(&runtime.dom_element(), "anim").map(|e| e.text.clone());
    assert_eq!(text_before.as_deref(), Some("Init"));

    let pumped = runtime.pump_events(16).unwrap();
    assert!(pumped > 0);

    let text_after = get_element_by_id(&runtime.dom_element(), "anim").map(|e| e.text.clone());
    assert_eq!(text_after.as_deref(), Some("FrameFired"));
    assert_eq!(runtime.report().animation_frames_fired, 1);
}

#[test]
fn test_cancel_animation_frame() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div id="target">Untouched</div>
            <script>
                let id = window.requestAnimationFrame(function() {
                    document.getElementById("target").textContent = "ShouldNotRun";
                });
                window.cancelAnimationFrame(id);
            </script>
        </body>
        </html>
    "#;

    let mut runtime =
        PageRuntime::from_html(html, Vec::new(), 800, "https://example.com/").unwrap();
    runtime.run_document().unwrap();

    runtime.pump_events(16).unwrap();

    let text = get_element_by_id(&runtime.dom_element(), "target").map(|e| e.text.clone());
    assert_eq!(text.as_deref(), Some("Untouched"));
    assert_eq!(runtime.report().animation_frames_fired, 0);
}

#[test]
fn test_youtube_spa_sample_fixture() {
    let fixture_path = "tests/fixtures/spa/youtube_spa_sample.html";
    let html = std::fs::read_to_string(fixture_path).expect("read fixture");

    let mut runtime =
        PageRuntime::from_html(&html, Vec::new(), 1200, "https://www.youtube.com/").unwrap();
    runtime.run_document().unwrap();

    let dom = runtime.dom_element();

    assert_eq!(
        dom.find_tag("title").map(|t| t.text.clone()).as_deref(),
        Some("YouTube - Next-Gen Engine")
    );

    let card = get_element_by_id(&dom, "card-1").expect("card-1 exists");
    let class_str = card.get_attr("class").unwrap();
    assert!(class_str.contains("active"));
    assert_eq!(
        card.get_attr("data-view-count").map(String::as_str),
        Some("1000000")
    );

    let player = get_element_by_id(&dom, "player").expect("player exists");
    assert_eq!(
        player.get_attr("src").map(String::as_str),
        Some("https://example.com/stream.mp4")
    );
}

#[test]
fn test_web_component_todo_app_fixture() {
    let fixture_path = "tests/fixtures/spa/web_component_todo_app.html";
    let html = std::fs::read_to_string(fixture_path).expect("read fixture");

    let mut runtime =
        PageRuntime::from_html(&html, Vec::new(), 800, "https://example.com/todo").unwrap();
    runtime.run_document().unwrap();

    let dom = runtime.dom_element();

    let t1 = get_element_by_id(&dom, "task-1").expect("task-1 exists");
    let class_t1 = t1.get_attr("class").unwrap();
    assert!(!class_t1.contains("done"));
    assert!(class_t1.contains("pending"));
    assert_eq!(
        t1.get_attr("data-priority").map(String::as_str),
        Some("high")
    );

    let t2 = get_element_by_id(&dom, "task-2").expect("task-2 exists");
    let class_t2 = t2.get_attr("class").unwrap();
    assert!(class_t2.contains("highlight"));
    let style_t2 = t2.get_attr("style").unwrap();
    assert!(
        style_t2.contains("background-color: rgb(255, 255, 0)")
            || style_t2.contains("background-color:rgb(255, 255, 0)")
    );
}
