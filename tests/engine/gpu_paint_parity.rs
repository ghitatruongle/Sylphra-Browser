use sylphra::css_parser::{parse_css, parse_linear_gradient, Transform2D};
use sylphra::paint::{
    gradient_color_at, parse_box_shadow, DisplayItem, DisplayList, GradientStop, Rgba,
};
use sylphra::scene_compositor::{CpuCompositor, RetainedScene, ScenePrimitive};

fn render_scene(items: Vec<DisplayItem>, width: u32, height: u32) -> Vec<u8> {
    let list = DisplayList {
        items,
        width: width as f32,
        height: height as f32,
        ..Default::default()
    };
    let scene = RetainedScene::from_display_list(1, &list).unwrap();
    let frame = CpuCompositor.render(&scene, width, height).unwrap();
    frame.rgba
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = (y as usize * width as usize + x as usize) * 4;
    [
        rgba[offset],
        rgba[offset + 1],
        rgba[offset + 2],
        rgba[offset + 3],
    ]
}

fn rounded_item() -> DisplayItem {
    DisplayItem::RoundedRect {
        x: 0.0,
        y: 0.0,
        w: 10.0,
        h: 10.0,
        color: Rgba::rgb(1.0, 0.0, 0.0),
        radius: 5.0,
        rotation: 0.0,
    }
}

#[test]
fn border_radius_limits_fill_to_rounded_region() {
    let rgba = render_scene(vec![rounded_item()], 10, 10);
    assert_eq!(pixel(&rgba, 10, 5, 5), [255, 0, 0, 255]);
    let corner = pixel(&rgba, 10, 0, 0);
    assert_eq!(corner[3], 0, "corner outside the radius stays transparent");
}

#[test]
fn box_shadow_falls_off_with_distance() {
    let shadow = DisplayItem::ShadowRect {
        x: 0.0,
        y: 0.0,
        w: 20.0,
        h: 20.0,
        color: Rgba::rgb(0.0, 0.0, 0.0),
        radius: 2.0,
        blur: 5.0,
        rotation: 0.0,
    };
    let rgba = render_scene(vec![shadow], 30, 30);
    let near = pixel(&rgba, 30, 10, 16)[3];
    let far = pixel(&rgba, 30, 10, 26)[3];
    assert!(near > 0, "pixel just past the shape carries shadow alpha");
    assert_eq!(far, 0, "pixel beyond the blur radius stays transparent");
}

#[test]
fn linear_gradient_interpolates_between_stops() {
    let gradient = DisplayItem::GradientRect {
        x: 0.0,
        y: 0.0,
        w: 10.0,
        h: 10.0,
        angle: 0.0,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: Rgba::rgb(1.0, 0.0, 0.0),
            },
            GradientStop {
                offset: 1.0,
                color: Rgba::rgb(0.0, 0.0, 1.0),
            },
        ],
        rotation: 0.0,
    };
    let rgba = render_scene(vec![gradient], 10, 10);
    let bottom = pixel(&rgba, 10, 5, 9);
    let top = pixel(&rgba, 10, 5, 0);
    assert!(bottom[0] > 200, "bottom approaches the first stop red");
    assert!(bottom[2] < 60, "bottom has little blue");
    assert!(top[2] > 200, "top approaches the last stop blue");
    assert!(top[0] < 60, "top has little red");
}

#[test]
fn rotation_moves_fill_off_axis_aligned_corners() {
    let rotated = DisplayItem::RoundedRect {
        x: 0.0,
        y: 0.0,
        w: 20.0,
        h: 10.0,
        color: Rgba::rgb(1.0, 0.0, 0.0),
        radius: 0.0,
        rotation: 90.0,
    };
    let rgba = render_scene(vec![rotated], 20, 10);
    let kept = pixel(&rgba, 20, 5, 9);
    assert_eq!(kept, [255, 0, 0, 255], "mapped interior stays painted");
    let displaced = pixel(&rgba, 20, 19, 9);
    assert_eq!(
        displaced[3], 0,
        "corner whose rotated position falls outside stays transparent"
    );
}

#[test]
fn transform_parser_accepts_rotation_and_rejects_unknown_functions() {
    let rotate = Transform2D::parse("rotate(90deg)").unwrap();
    assert!((rotate.rotation_deg - 90.0).abs() < f64::EPSILON);
    let radians = Transform2D::parse("rotate(1.5707963rad)").unwrap();
    assert!((radians.rotation_deg - 90.0).abs() < 0.001);
    assert!(Transform2D::parse("unknown(1px)").is_none());
}

#[test]
fn linear_gradient_parser_reads_angle_and_stops() {
    let gradient = parse_linear_gradient("linear-gradient(90deg, red, blue)").unwrap();
    assert!((gradient.angle_deg - 90.0).abs() < f32::EPSILON);
    assert_eq!(gradient.stops.len(), 2);
    assert!((gradient.stops[0].offset - 0.0).abs() < f32::EPSILON);
    assert!((gradient.stops[1].offset - 1.0).abs() < f32::EPSILON);
    assert_eq!(
        gradient_color_at(&gradient.stops, 0.5),
        Rgba {
            r: 0.5,
            g: 0.0,
            b: 0.5,
            a: 1.0
        }
    );
    assert!(parse_linear_gradient("linear-gradient(red)").is_none());
}

#[test]
fn box_shadow_parser_reads_offsets_blur_and_color() {
    let shadow = parse_box_shadow("0px 4px 12px rgba(0, 0, 0, 0.5)").unwrap();
    assert_eq!(shadow.offset_x, 0.0);
    assert_eq!(shadow.offset_y, 4.0);
    assert_eq!(shadow.blur, 12.0);
    assert!((shadow.color.a - 0.5).abs() < 0.01);
    assert!(parse_box_shadow("none").is_none());
    assert!(parse_box_shadow("4px 8px").is_none());
}

#[test]
fn display_list_renders_mixed_parity_items_without_budget_faults() {
    let items = vec![
        DisplayItem::ShadowRect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 20.0,
            color: Rgba::rgb(0.0, 0.0, 0.0),
            radius: 4.0,
            blur: 6.0,
            rotation: 0.0,
        },
        DisplayItem::GradientRect {
            x: 2.0,
            y: 2.0,
            w: 36.0,
            h: 16.0,
            angle: 90.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Rgba::rgb(1.0, 1.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: Rgba::rgb(0.0, 0.0, 0.0),
                },
            ],
            rotation: 0.0,
        },
        rounded_item(),
    ];
    let rgba = render_scene(items, 40, 40);
    assert!(rgba.chunks_exact(4).any(|p| p[3] > 0));
}

#[test]
fn css_rules_with_new_properties_flow_through_cascade() {
    let rules = parse_css(
        ".card { border-radius: 8px; box-shadow: 0px 2px 6px rgba(0,0,0,0.3); background-image: linear-gradient(45deg, white, black); transform: rotate(15deg); }",
    );
    assert_eq!(rules.len(), 1);
    assert!(rules[0]
        .declarations
        .iter()
        .any(|d| d.property == "border-radius"));
    assert!(rules[0]
        .declarations
        .iter()
        .any(|d| d.property == "box-shadow"));
    assert!(rules[0]
        .declarations
        .iter()
        .any(|d| d.property == "background-image"));
    assert!(rules[0]
        .declarations
        .iter()
        .any(|d| d.property == "transform"));
}

#[test]
fn scene_estimate_counts_gradient_stops() {
    let list = DisplayList {
        items: vec![DisplayItem::GradientRect {
            x: 0.0,
            y: 0.0,
            w: 8.0,
            h: 8.0,
            angle: 0.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Rgba::WHITE,
                },
                GradientStop {
                    offset: 1.0,
                    color: Rgba::BLACK,
                },
            ],
            rotation: 0.0,
        }],
        width: 8.0,
        height: 8.0,
        ..Default::default()
    };
    let scene = RetainedScene::from_display_list(1, &list).unwrap();
    assert!(matches!(
        scene.primitives()[0],
        ScenePrimitive::GradientRect { .. }
    ));
    assert!(scene.estimated_bytes() > 0);
}
