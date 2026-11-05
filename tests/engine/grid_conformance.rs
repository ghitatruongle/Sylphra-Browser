use sylphra::css_parser::{parse_css, parse_grid_tracks, GridTrackSize};
use sylphra::layout::{create_layout_tree, LayoutNode};
use sylphra::parser::parse_html;

fn build_layout(html: &str, css: &str, width: u32) -> LayoutNode {
    let rules = parse_css(css);
    let dom = parse_html(html);
    create_layout_tree(&dom, &rules, width).unwrap()
}

fn find_by_class<'a>(node: &'a LayoutNode, class: &str) -> Option<&'a LayoutNode> {
    if node.element.get_attr("class").map(String::as_str) == Some(class) {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_by_class(child, class))
}

#[test]
fn grid_track_parser_reads_fr_px_percent_and_repeat() {
    let tracks = parse_grid_tracks("1fr 2fr").unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].max, GridTrackSize::Fr(1.0));
    assert_eq!(tracks[1].max, GridTrackSize::Fr(2.0));

    let fixed = parse_grid_tracks("repeat(2, 100px)").unwrap();
    assert_eq!(fixed.len(), 2);
    assert_eq!(fixed[0].max, GridTrackSize::Pixels(100.0));

    let auto = parse_grid_tracks("repeat(auto-fit, 1fr)").unwrap();
    assert_eq!(auto.len(), 1);
    assert!(auto[0].auto_repeat);

    let minmax = parse_grid_tracks("minmax(100px, 1fr)").unwrap();
    assert_eq!(minmax[0].min, GridTrackSize::Pixels(100.0));
    assert_eq!(minmax[0].max, GridTrackSize::Fr(1.0));

    assert!(parse_grid_tracks("").is_none());
    assert!(parse_grid_tracks("banana").is_none());
}

#[test]
fn grid_tracks_fr_distribute_leftover() {
    let layout = build_layout(
        "<div class='grid'><p>a</p><p>b</p><p>c</p></div>",
        ".grid{display:grid;grid-template-columns:1fr 2fr 1fr;gap:10px;width:300px}",
        800,
    );
    let grid = find_by_class(&layout, "grid").unwrap();
    let (a, b, c) = (
        &grid.children[0].rect,
        &grid.children[1].rect,
        &grid.children[2].rect,
    );
    assert!(
        ((c.x - b.x) - (b.x - a.x) - 70.0).abs() < 2.0,
        "c.x={} b.x={} a.x={}",
        c.x,
        b.x,
        a.x
    );
}

#[test]
fn grid_fixed_tracks_keep_explicit_sizes() {
    let layout = build_layout(
        "<div class='grid'><p>a</p><p>b</p></div>",
        ".grid{display:grid;grid-template-columns:100px 50%;width:200px}",
        800,
    );
    let grid = find_by_class(&layout, "grid").unwrap();
    let (a, b) = (&grid.children[0].rect, &grid.children[1].rect);
    assert!((b.x - a.x - 100.0).abs() < 1.0, "b.x={} a.x={}", b.x, a.x);
}

#[test]
fn grid_repeat_minmax_expands() {
    let layout = build_layout(
        "<div class='grid'><p>a</p><p>b</p><p>c</p></div>",
        ".grid{display:grid;grid-template-columns:repeat(3, minmax(50px, 1fr));width:300px}",
        800,
    );
    let grid = find_by_class(&layout, "grid").unwrap();
    let (a, b, c) = (
        &grid.children[0].rect,
        &grid.children[1].rect,
        &grid.children[2].rect,
    );
    assert!((b.x - a.x - 100.0).abs() < 1.0, "b.x={} a.x={}", b.x, a.x);
    assert!((c.x - b.x - 100.0).abs() < 1.0, "c.x={} b.x={}", c.x, b.x);
}

#[test]
fn grid_auto_fit_tracks_expand_with_children() {
    let layout = build_layout(
        "<div class='grid'><p>a</p><p>b</p><p>c</p></div>",
        ".grid{display:grid;grid-template-columns:repeat(auto-fit, 1fr);width:300px}",
        800,
    );
    let grid = find_by_class(&layout, "grid").unwrap();
    let (a, b) = (&grid.children[0].rect, &grid.children[1].rect);
    assert!((b.x - a.x - 100.0).abs() < 1.0, "b.x={} a.x={}", b.x, a.x);
}

#[test]
fn grid_column_span_stretches_item_across_tracks() {
    let layout = build_layout(
        "<div class='grid'><p>a</p><p class='wide'>b</p><p>c</p></div>",
        ".grid{display:grid;grid-template-columns:1fr 1fr;width:200px}.wide{grid-column:span 2}",
        800,
    );
    let grid = find_by_class(&layout, "grid").unwrap();
    let wide = find_by_class(grid, "wide").unwrap();
    assert!(
        wide.rect.x <= grid.rect.x + 1.0,
        "wide starts at the first column"
    );
    assert!(wide.rect.width > 150.0, "wide stretches across both tracks");
    assert!(
        wide.rect.y > grid.children[0].rect.y,
        "wide wraps to a new row"
    );
}

#[test]
fn grid_row_span_distributes_height() {
    let layout = build_layout(
        "<div class='grid'><p>a</p><p class='tall'>b</p><p>c</p></div>",
        ".grid{display:grid;grid-template-columns:1fr 1fr}.tall{grid-row:span 2;height:200px}",
        800,
    );
    let grid = find_by_class(&layout, "grid").unwrap();
    let (a, c) = (&grid.children[0].rect, &grid.children[2].rect);
    assert!(
        grid.rect.height > 150.0,
        "row span pulls the grid extent to the tall item"
    );
    assert!(c.y > a.y, "second row starts below the first");
}

#[test]
fn grid_falls_back_to_two_columns_without_template() {
    let layout = build_layout(
        "<div class='grid'><p>a</p><p>b</p><p>c</p></div>",
        ".grid{display:grid;width:200px}",
        800,
    );
    let grid = find_by_class(&layout, "grid").unwrap();
    let (a, b, c) = (
        &grid.children[0].rect,
        &grid.children[1].rect,
        &grid.children[2].rect,
    );
    assert!(b.x > a.x, "second item sits beside the first");
    assert!(c.y >= b.y, "third item wraps below");
}
