use sylphra::css_parser::parse_css;
use sylphra::paint::DisplayItem;
use sylphra::LiveDocument;

fn red_fill_alphas(items: &[DisplayItem]) -> Vec<f32> {
    items
        .iter()
        .filter_map(|item| {
            let color = match item {
                DisplayItem::Rect { color, .. } => color,
                DisplayItem::RoundedRect { color, .. } => color,
                DisplayItem::GradientRect { stops, .. } => &stops.first()?.color,
                _ => return None,
            };
            if color.r > 0.9 {
                Some(color.a)
            } else {
                None
            }
        })
        .collect()
}

fn find_rect_with_red_fill(items: &[DisplayItem]) -> Option<[u8; 4]> {
    red_fill_alphas(items)
        .last()
        .map(|alpha| [255, 0, 0, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8])
}

#[test]
fn keyframes_parser_reads_from_to_and_percent_steps() {
    let rules = parse_css(
        "@keyframes pulse { from { opacity: 0; } 50% { opacity: 0.5; } to { opacity: 1; } }",
    );
    let keyframes = rules
        .iter()
        .filter_map(|rule| rule.keyframes.as_ref())
        .find(|keyframes| keyframes.name == "pulse")
        .unwrap();
    assert_eq!(keyframes.steps.len(), 3);
    assert!((keyframes.steps[0].offset - 0.0).abs() < f32::EPSILON);
    assert!((keyframes.steps[1].offset - 0.5).abs() < f32::EPSILON);
    assert!((keyframes.steps[2].offset - 1.0).abs() < f32::EPSILON);
    assert!(keyframes.steps[1]
        .declarations
        .iter()
        .any(|(property, value)| property == "opacity" && value == "0.5"));
}

#[test]
fn named_keyframes_animation_interpolates_opacity() {
    let rules = parse_css(
        "@keyframes fade-up { from { opacity: 0; } to { opacity: 1; } } \
         .box { background-color: #ff0000; animation: fade-up 100ms 1; }",
    );
    let mut document = LiveDocument::parse("<div class='box'>x</div>", rules, 800);
    document.advance_animations(50);
    let state = document.render_state();
    assert!(state.dynamic.active_animations > 0);
    let alphas = red_fill_alphas(&state.display_list.items);
    assert!(
        alphas.iter().any(|alpha| (alpha - 0.5).abs() < 0.15),
        "mid-animation alpha should be near 0.5, got {:?}",
        alphas
    );

    document.advance_animations(100);
    let settled = document.render_state();
    let alphas = red_fill_alphas(&settled.display_list.items);
    assert!(
        alphas.iter().any(|alpha| (alpha - 1.0).abs() < 0.01),
        "final sample restores full opacity, got {:?}",
        alphas
    );
}

#[test]
fn keyframes_transform_translation_moves_layout() {
    let rules = parse_css(
        "@keyframes slide-right { from { transform: translate(0px, 0px); } to { transform: translate(40px, 0px); } } \
         .mover { width: 40px; height: 20px; background-color: #0000ff; animation: slide-right 100ms 1; }",
    );
    let mut document = LiveDocument::parse("<div class='mover'>y</div>", rules, 800);
    let base_x = document.render_state().layout.clone().unwrap().rect.x;
    document.advance_animations(50);
    let mid_x = document.render_state().layout.clone().unwrap().rect.x;
    assert!(
        (mid_x - base_x - 20.0).abs() < 3.0,
        "mid translation should be near 20px, moved {}",
        mid_x - base_x
    );
}

#[test]
fn missing_keyframes_name_stays_inert() {
    let rules = parse_css(
        "@keyframes unrelated { from { opacity: 0; } to { opacity: 1; } } \
         .box { background-color: #ff0000; animation: absent 100ms 1; }",
    );
    let mut document = LiveDocument::parse("<div class='box'>z</div>", rules, 800);
    document.advance_animations(50);
    let state = document.render_state();
    assert_eq!(state.dynamic.active_animations, 0);
    let fill = find_rect_with_red_fill(&state.display_list.items).unwrap();
    assert_eq!(fill[3], 255);
}

#[test]
fn keyframes_with_unknown_properties_still_interpolate_opacity() {
    let rules = parse_css(
        "@keyframes mixed { from { opacity: 0.2; color: red; } to { opacity: 0.8; color: blue; } } \
         .box { background-color: #ff0000; animation: mixed 100ms 1; }",
    );
    let mut document = LiveDocument::parse("<div class='box'>w</div>", rules, 800);
    document.advance_animations(50);
    let mid = document.render_state();
    let mid_alphas = red_fill_alphas(&mid.display_list.items);
    assert!(
        mid_alphas.iter().any(|alpha| (alpha - 0.5).abs() < 0.1),
        "mid interpolation blends 0.2 toward 0.8 near 0.5, got {:?}",
        mid_alphas
    );
    document.advance_animations(100);
    let settled = document.render_state();
    let alphas = red_fill_alphas(&settled.display_list.items);
    assert!(
        alphas.iter().any(|alpha| (alpha - 1.0).abs() < 0.01),
        "without fill-mode the element returns to its base opacity, got {:?}",
        alphas
    );
}
