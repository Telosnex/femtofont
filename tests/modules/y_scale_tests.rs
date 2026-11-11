use femtofont::{Font, FontSettings, StrokeSettings};

fn roboto() -> Font<'static> {
    Font::from_bytes(
        include_bytes!("../../resources/fonts/Roboto-Regular.ttf") as &[u8],
        FontSettings::default(),
    )
    .unwrap()
}

#[test]
fn unit_y_scale_matches_the_uniform_path() {
    let font = roboto();
    let settings = StrokeSettings::new(2.0);
    for character in ['x', 'H', 'g', 'é'] {
        let index = font.lookup_glyph_index(character);
        for px in [11.0, 17.5, 32.0] {
            assert_eq!(font.rasterize_indexed_y_scaled(index, px, 1.0), font.rasterize_indexed(index, px));
            assert_eq!(font.metrics_indexed_y_scaled(index, px, 1.0), font.metrics_indexed(index, px));
            assert_eq!(
                font.rasterize_indexed_with_stroke_y_scaled(index, px, 1.0, settings),
                font.rasterize_indexed_with_stroke(index, px, settings),
            );
        }
    }
}

#[test]
fn y_scale_changes_only_the_height() {
    let font = roboto();
    let index = font.lookup_glyph_index('x');
    let px = 15.0;
    let plain = font.metrics_indexed(index, px);
    let x_height = plain.bounds.height;
    // Scale the x-height to the next whole pixel.
    let y_scale = x_height.ceil() / x_height;
    assert!(y_scale > 1.0);
    let (scaled, coverage) = font.rasterize_indexed_y_scaled(index, px, y_scale);
    assert_eq!(scaled, font.metrics_indexed_y_scaled(index, px, y_scale));
    assert_eq!(scaled.advance_width, plain.advance_width);
    assert_eq!(scaled.bounds.width, plain.bounds.width);
    assert_eq!(scaled.xmin, plain.xmin);
    assert!((scaled.bounds.height - x_height.ceil()).abs() < 1e-3);
    // 'x' sits on the baseline, so a whole-pixel x-height fills its top row.
    assert_eq!(scaled.ymin, 0);
    assert_eq!(scaled.height as f32, x_height.ceil());
    let top_row = &coverage[..scaled.width];
    assert!(top_row.iter().any(|&value| value > 200), "top row {top_row:?}");
}

#[test]
fn y_scaled_stroke_shares_the_fill_bitmap() {
    let font = roboto();
    let index = font.lookup_glyph_index('o');
    let glyph = font.rasterize_indexed_with_stroke_y_scaled(index, 20.0, 1.1, StrokeSettings::new(2.0));
    let len = glyph.metrics.width * glyph.metrics.height;
    assert_eq!(glyph.fill.len(), len);
    assert_eq!(glyph.stroke.len(), len);
    let plain = font.rasterize_indexed_with_stroke(index, 20.0, StrokeSettings::new(2.0));
    assert!(glyph.metrics.bounds.height > plain.metrics.bounds.height);
    assert_eq!(glyph.metrics.bounds.width, plain.metrics.bounds.width);
}

#[test]
fn invalid_y_scale_is_ignored() {
    let font = roboto();
    let index = font.lookup_glyph_index('a');
    for y_scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            font.rasterize_indexed_y_scaled(index, 14.0, y_scale),
            font.rasterize_indexed(index, 14.0)
        );
    }
}
