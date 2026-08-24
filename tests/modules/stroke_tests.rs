use femtofont::{Font, FontSettings, LineJoin, StrokeSettings};

use crate::modules::FONTS;

#[test]
fn stroke_layers_are_aligned_and_expand_ink_bounds() {
    let font = Font::from_bytes(FONTS[0], FontSettings::default()).unwrap();
    let (fill_metrics, fill) = font.rasterize('O', 40.0);
    let rasterized = font.rasterize_with_stroke('O', 40.0, StrokeSettings::new(6.0));

    assert_eq!(rasterized.fill.len(), rasterized.metrics.width * rasterized.metrics.height);
    assert_eq!(rasterized.stroke.len(), rasterized.fill.len());
    assert!(rasterized.metrics.xmin <= fill_metrics.xmin);
    assert!(rasterized.metrics.ymin <= fill_metrics.ymin);
    assert!(rasterized.metrics.width >= fill_metrics.width);
    assert!(rasterized.metrics.height >= fill_metrics.height);
    assert!(rasterized.stroke.iter().any(|coverage| *coverage != 0));

    let x_offset = (fill_metrics.xmin - rasterized.metrics.xmin) as usize;
    let old_top = fill_metrics.ymin + fill_metrics.height as i32;
    let new_top = rasterized.metrics.ymin + rasterized.metrics.height as i32;
    let y_offset = (new_top - old_top) as usize;
    for y in 0..fill_metrics.height {
        let old_row = &fill[y * fill_metrics.width..(y + 1) * fill_metrics.width];
        let start = (y + y_offset) * rasterized.metrics.width + x_offset;
        assert_eq!(old_row, &rasterized.fill[start..start + fill_metrics.width]);
    }
}

#[test]
fn subpixel_and_stroke_only_apis_have_consistent_dimensions() {
    let font = Font::from_bytes(FONTS[1], FontSettings::default()).unwrap();
    let settings = StrokeSettings::new(3.5).with_line_join(LineJoin::Miter).with_miter_limit(3.0);
    let layered = font.rasterize_with_stroke_subpixel('W', 36.0, settings);
    let (metrics, stroke) = font.rasterize_stroke_subpixel('W', 36.0, settings);

    assert_eq!(layered.fill.len(), layered.metrics.width * layered.metrics.height * 3);
    assert_eq!(layered.stroke.len(), layered.fill.len());
    assert_eq!(stroke.len(), metrics.width * metrics.height * 3);
    assert_eq!(metrics, layered.metrics);
    assert_eq!(stroke, layered.stroke);
}

#[test]
fn invalid_stroke_width_is_empty_without_affecting_fill() {
    let font = Font::from_bytes(FONTS[0], FontSettings::default()).unwrap();
    let (metrics, fill) = font.rasterize('e', 24.0);
    for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let rasterized = font.rasterize_with_stroke('e', 24.0, StrokeSettings::new(width));
        assert_eq!(rasterized.metrics, metrics);
        assert_eq!(rasterized.fill, fill);
        assert!(rasterized.stroke.iter().all(|coverage| *coverage == 0));
        let (stroke_metrics, stroke) = font.rasterize_stroke('e', 24.0, StrokeSettings::new(width));
        assert_eq!(stroke_metrics.width, 0);
        assert_eq!(stroke_metrics.height, 0);
        assert!(stroke.is_empty());
    }
}

#[test]
fn all_bundled_glyphs_stroke_without_invalid_buffers() {
    for bytes in FONTS {
        let font = Font::from_bytes(bytes, FontSettings::default()).unwrap();
        for index in 0..font.face.number_of_glyphs() {
            let (metrics, bitmap) = font.rasterize_indexed_stroke(index, 18.0, StrokeSettings::new(2.0));
            assert_eq!(bitmap.len(), metrics.width * metrics.height, "glyph index {index}");
        }
    }
}

#[test]
fn join_styles_render_acute_glyphs() {
    let font = Font::from_bytes(FONTS[0], FontSettings::default()).unwrap();
    for character in ['M', 'V', 'W', 'g', 'e'] {
        for join in [LineJoin::Round, LineJoin::Bevel, LineJoin::Miter] {
            for width in [0.5, 4.0, 16.0] {
                let settings = StrokeSettings::new(width).with_line_join(join).with_miter_limit(4.0);
                let (metrics, bitmap) = font.rasterize_stroke(character, 40.0, settings);
                assert_eq!(bitmap.len(), metrics.width * metrics.height);
                assert!(bitmap.iter().any(|coverage| *coverage != 0));
            }
        }
    }
}

#[test]
fn collapsed_counters_do_not_reopen_at_larger_widths() {
    let font = Font::from_bytes(FONTS[0], FontSettings::default()).unwrap();
    let fill_bounds = font.metrics('O', 80.0).bounds;
    let center_x = fill_bounds.xmin + fill_bounds.width * 0.5;
    let center_y = fill_bounds.ymin + fill_bounds.height * 0.5;
    let center_coverage = |width| {
        let (metrics, bitmap) = font.rasterize_stroke('O', 80.0, StrokeSettings::new(width));
        let x = (center_x - metrics.xmin as f32).floor() as usize;
        let top = metrics.ymin as f32 + metrics.height as f32;
        let y = (top - center_y).floor() as usize;
        bitmap[y * metrics.width + x]
    };

    assert_eq!(center_coverage(4.0), 0);
    for width in [40.0, 60.0, 100.0] {
        assert!(center_coverage(width) > 0, "counter reopened at width {width}");
    }
}
