use std::sync::Arc;

use sylphra::paint::{DisplayItem, DisplayList, Rgba};
use sylphra::scene_compositor::{CpuCompositor, ImagePixels, RetainedScene, ScenePrimitive};

fn opaque_pixels(width: u32, height: u32, color: [u8; 4]) -> ImagePixels {
    let count = width as usize * height as usize;
    let mut rgba = Vec::with_capacity(count * 4);
    for _ in 0..count {
        rgba.extend_from_slice(&color);
    }
    ImagePixels {
        width,
        height,
        rgba: Arc::new(rgba),
    }
}

fn image_item(url: &str) -> DisplayItem {
    DisplayItem::Image {
        x: 2.0,
        y: 2.0,
        w: 4.0,
        h: 4.0,
        url: url.to_string(),
        alt: String::new(),
        cached: true,
    }
}

#[test]
fn bind_images_replaces_ready_placeholders_and_cpu_compositor_blits_pixels() {
    let list = DisplayList {
        items: vec![
            DisplayItem::Rect {
                x: 0.0,
                y: 0.0,
                w: 8.0,
                h: 8.0,
                color: Rgba::rgb(0.0, 0.0, 1.0),
            },
            image_item("https://a.com/red.png"),
        ],
        width: 8.0,
        height: 8.0,
        ..Default::default()
    };
    let mut scene = RetainedScene::from_display_list(1, &list).unwrap();
    let red = opaque_pixels(2, 2, [255, 0, 0, 255]);
    let bound = scene.bind_images(&|url| {
        if url == "https://a.com/red.png" {
            Some(red.clone())
        } else {
            None
        }
    });
    assert_eq!(bound, 1);
    let frame = CpuCompositor.render(&scene, 8, 8).unwrap();
    let center = (3usize * 8 + 3) * 4;
    assert_eq!(&frame.rgba[center..center + 4], &[255, 0, 0, 255]);
}

#[test]
fn cpu_compositor_alpha_blends_bound_images_over_background() {
    let list = DisplayList {
        items: vec![
            DisplayItem::Rect {
                x: 0.0,
                y: 0.0,
                w: 4.0,
                h: 4.0,
                color: Rgba::rgb(0.0, 0.0, 1.0),
            },
            image_item("https://a.com/veil.png"),
        ],
        width: 4.0,
        height: 4.0,
        ..Default::default()
    };
    let mut scene = RetainedScene::from_display_list(1, &list).unwrap();
    let veil = opaque_pixels(1, 1, [255, 0, 0, 128]);
    let bound = scene.bind_images(&|_| Some(veil.clone()));
    assert_eq!(bound, 1);
    let frame = CpuCompositor.render(&scene, 4, 4).unwrap();
    let center = (3usize * 4 + 3) * 4;
    assert_eq!(&frame.rgba[center..center + 4], &[128, 0, 127, 255]);
}

#[test]
fn bind_images_keeps_placeholder_when_scene_byte_budget_exceeded() {
    let list = DisplayList {
        items: vec![image_item("https://a.com/huge.png")],
        width: 8.0,
        height: 8.0,
        ..Default::default()
    };
    let mut scene = RetainedScene::from_display_list(1, &list).unwrap();
    let oversized = opaque_pixels(5100, 5100, [1, 2, 3, 255]);
    let bound = scene.bind_images(&|_| Some(oversized.clone()));
    assert_eq!(bound, 0);
    assert!(matches!(
        scene.primitives()[0],
        ScenePrimitive::ImagePlaceholder { ready: true, .. }
    ));
}

#[test]
fn bind_images_rejects_pixels_with_dimension_mismatch() {
    let list = DisplayList {
        items: vec![image_item("https://a.com/broken.png")],
        width: 8.0,
        height: 8.0,
        ..Default::default()
    };
    let mut scene = RetainedScene::from_display_list(1, &list).unwrap();
    let broken = ImagePixels {
        width: 4,
        height: 4,
        rgba: Arc::new(vec![0u8; 10]),
    };
    let bound = scene.bind_images(&|_| Some(broken.clone()));
    assert_eq!(bound, 0);
    assert!(matches!(
        scene.primitives()[0],
        ScenePrimitive::ImagePlaceholder { ready: true, .. }
    ));
}

#[test]
fn bind_images_bumps_revision_and_marks_damage() {
    let list = DisplayList {
        items: vec![image_item("https://a.com/late.png")],
        width: 8.0,
        height: 8.0,
        ..Default::default()
    };
    let mut scene = RetainedScene::from_display_list(1, &list).unwrap();
    scene.clear_damage();
    let base_revision = scene.revision;
    let late = opaque_pixels(2, 2, [0, 255, 0, 255]);
    let bound = scene.bind_images(&|_| Some(late.clone()));
    assert_eq!(bound, 1);
    assert_eq!(scene.revision, base_revision + 1);
    assert!(!scene.damage().is_empty());
}
