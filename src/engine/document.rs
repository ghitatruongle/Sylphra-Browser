use std::time::Instant;

use crate::css_parser::{self, CssRule};
use crate::layout::{self, LayoutNode};
use crate::parser::{self, Element};
use crate::text_renderer::TextRenderer;
use crate::RenderStats;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PreparedDocument {
    pub dom: Element,
    pub title: String,
    pub layout: Option<LayoutNode>,
    pub accessibility: crate::accessibility::AccessibilityTree,
    pub runtime: crate::web_runtime::RuntimeReport,
    pub rendered_text: String,
    pub stats: RenderStats,
}

pub fn prepare_document(
    html: &str,
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
) -> PreparedDocument {
    prepare_document_impl(
        html,
        fallback_title,
        base_rules,
        viewport_width,
        viewport_height,
        true,
    )
}

pub fn prepare_document_static(
    html: &str,
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
) -> PreparedDocument {
    prepare_document_impl(
        html,
        fallback_title,
        base_rules,
        viewport_width,
        viewport_height,
        false,
    )
}

fn prepare_document_impl(
    html: &str,
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
    execute_scripts: bool,
) -> PreparedDocument {
    let total_start = Instant::now();

    let parse_start = Instant::now();
    let mut dom = parser::parse_html(html);
    let parse_time_ms = parse_start.elapsed().as_millis() as u64;

    let runtime = if execute_scripts {
        crate::web_runtime::run_inline_scripts(&mut dom, fallback_title)
    } else {
        crate::web_runtime::RuntimeReport::default()
    };

    let title = dom
        .find_tag("title")
        .or_else(|| dom.find_tag("h1"))
        .map(|element| element.text.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| fallback_title.to_string());
    let style_start = Instant::now();

    let mut all_rules = base_rules.to_vec();
    for style in dom.find_all_tags("style") {
        if !style.text.trim().is_empty() {
            all_rules.extend(css_parser::parse_css_with_media(
                style.text.trim(),
                viewport_width,
            ));
        }
    }
    let style_time_ms = style_start.elapsed().as_millis() as u64;

    let layout_start = Instant::now();

    let live = crate::live_dom::LiveDocument::from_element(&dom, all_rules, viewport_width);
    let live_render = live.render_state();

    let dom = live_render.dom.clone();
    let accessibility = live_render.accessibility.clone();
    let layout = live_render.layout.clone();
    let layout_time_ms = layout_start.elapsed().as_millis() as u64;

    let render_start = Instant::now();
    let rendered_text = layout
        .as_ref()
        .map(|root| TextRenderer::new(viewport_width, viewport_height).render_to_text(root))
        .unwrap_or_else(|| "[Empty page]".to_string());
    let render_time_ms = render_start.elapsed().as_millis() as u64;

    let dom_nodes = count_dom_nodes(&dom);
    let layout_nodes = layout.as_ref().map(layout::count_layout_nodes).unwrap_or(0);

    PreparedDocument {
        dom,
        title,
        layout,
        accessibility,
        runtime,
        rendered_text,
        stats: RenderStats {
            parse_time_ms,
            style_time_ms,
            layout_time_ms,
            render_time_ms,
            total_time_ms: total_start.elapsed().as_millis() as u64,
            dom_nodes,
            layout_nodes,
        },
    }
}

pub fn prepare_live_document(
    html: &str,
    base_url: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
) -> crate::live_dom::LiveDocument {
    let mut dom = parser::parse_html(html);
    let _runtime = crate::web_runtime::run_inline_scripts(&mut dom, base_url);

    let mut rules = base_rules.to_vec();
    for style in dom.find_all_tags("style") {
        if !style.text.trim().is_empty() {
            rules.extend(css_parser::parse_css_with_media(
                style.text.trim(),
                viewport_width,
            ));
        }
    }
    crate::live_dom::LiveDocument::from_element(&dom, rules, viewport_width)
}

fn count_dom_nodes(element: &Element) -> usize {
    let mut count = 0usize;
    let mut stack: Vec<&Element> = vec![element];
    while let Some(current) = stack.pop() {
        count = count.saturating_add(1);
        stack.extend(current.children.iter());
    }
    count
}

pub fn skeleton_document(document: &mut PreparedDocument, max_depth: usize, text_budget: usize) {
    skeleton_element(&mut document.dom, 1, max_depth.max(1));
    let mut rendered = String::new();
    collect_skeleton_text(&document.dom, text_budget, &mut rendered);
    document.rendered_text = rendered;
    document.layout = None;
    document.accessibility = crate::accessibility::AccessibilityTree {
        root: None,
        node_count: 0,
        truncated: true,
    };
}

fn skeleton_element(element: &mut Element, depth: usize, max_depth: usize) {
    clamp_str(&mut element.text, SKELETON_NODE_TEXT_BYTES);
    element.attrs.retain(|_, value| {
        clamp_str(value, SKELETON_ATTRIBUTE_TEXT_BYTES);
        true
    });
    if depth >= max_depth {
        element.children.clear();
        return;
    }
    for child in &mut element.children {
        skeleton_element(child, depth + 1, max_depth);
    }
}

fn collect_skeleton_text(element: &Element, budget: usize, output: &mut String) {
    if output.len() >= budget {
        return;
    }
    if !element.text.trim().is_empty() {
        let remaining = budget.saturating_sub(output.len());
        let mut end = remaining.min(element.text.len());
        while end > 0 && !element.text.is_char_boundary(end) {
            end -= 1;
        }
        output.push_str(element.text[..end].trim());
        output.push('\n');
    }
    for child in &element.children {
        collect_skeleton_text(child, budget, output);
    }
}

fn clamp_str(value: &mut String, limit: usize) {
    if value.len() <= limit {
        return;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

const SKELETON_NODE_TEXT_BYTES: usize = 512;
const SKELETON_ATTRIBUTE_TEXT_BYTES: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_keeps_structure_and_drops_layout() {
        let mut prepared = prepare_document_static(
            "<html><body><div><p>deep text</p></div></body></html>",
            "skeleton",
            &[],
            800,
            600,
        );
        skeleton_document(
            &mut prepared,
            crate::resource_caps::DOM_TRANSFER_SKELETON_DEPTH,
            4096,
        );
        assert!(prepared.layout.is_none());
        assert!(prepared.rendered_text.contains("deep text"));

        let mut shallow = prepare_document_static(
            "<html><body><div><p>gone</p></div></body></html>",
            "skeleton",
            &[],
            800,
            600,
        );
        let breadth = shallow.dom.children.len();
        skeleton_document(&mut shallow, 1, 4096);
        assert!(shallow.dom.children.is_empty() || breadth == 0);
    }

    #[test]
    fn prepares_title_inline_style_and_stats() {
        let prepared = prepare_document(
            "<html><head><title>Doc</title><style>p{color:red}</style></head><body><p>x</p></body></html>",
            "fallback",
            &[],
            800,
            600,
        );
        assert_eq!(prepared.title, "Doc");
        assert!(prepared.layout.is_some());
        assert!(prepared.rendered_text.contains('x'));
        assert!(prepared.stats.dom_nodes >= 2);
        assert!(prepared.stats.layout_nodes >= 1);
    }

    #[test]
    fn uses_fallback_title_for_untitled_document() {
        let prepared = prepare_document("<p>hello</p>", "https://example.com", &[], 800, 600);
        assert_eq!(prepared.title, "https://example.com");
    }

    #[test]
    fn static_preparation_defers_scripts_to_the_persistent_embedder_runtime() {
        let prepared = prepare_document_static(
            "<p id='value'>before</p><script>document.getElementById('value').textContent='after'</script>",
            "https://example.test/",
            &[],
            800,
            600,
        );
        assert_eq!(prepared.runtime.scripts_executed, 0);
        assert!(prepared.rendered_text.contains("before"));
        assert!(!prepared.rendered_text.contains("after"));
    }

    #[test]
    fn live_preparation_preserves_identity_and_refreshes_pixels() {
        let mut live = prepare_live_document(
            "<p id='message'>before</p>",
            "https://example.test/",
            &[],
            800,
        );
        let node = live.get_element_by_id("message").unwrap();
        let before = live.render_state().revision;
        live.set_text_content(node, "after").unwrap();
        assert_eq!(live.get_element_by_id("message"), Some(node));
        let render = live.refresh();
        assert!(render.revision > before);
        assert!(render
            .display_list
            .items
            .iter()
            .any(|item| matches!(item, crate::paint::DisplayItem::TextRun { content, .. } if content.contains("after"))));
    }
}
