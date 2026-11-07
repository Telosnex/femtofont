use femtofont::{Font, FontSettings, StrokeSettings, MAX_GLYPH_PIXELS};

fn roboto() -> Font<'static> {
    Font::from_bytes(&super::FONTS[0][..], FontSettings::default()).unwrap()
}

#[test]
fn an_index_past_the_glyph_count_draws_notdef() {
    let font = roboto();
    assert_eq!(font.rasterize_indexed(u16::MAX, 24.0), font.rasterize_indexed(0, 24.0));
    assert_eq!(font.metrics_indexed(u16::MAX, 24.0), font.metrics_indexed(0, 24.0));
    let stroked = font.rasterize_indexed_with_stroke(u16::MAX, 24.0, StrokeSettings::new(2.0));
    let notdef = font.rasterize_indexed_with_stroke(0, 24.0, StrokeSettings::new(2.0));
    assert_eq!((stroked.metrics, stroked.fill), (notdef.metrics, notdef.fill));
}

#[test]
fn a_glyph_past_the_pixel_limit_is_blank() {
    let font = roboto();
    let index = font.lookup_glyph_index('W');
    for px in [100_000.0, f32::MAX] {
        let (metrics, bitmap) = font.rasterize_indexed(index, px);
        assert_eq!((metrics.width, metrics.height, bitmap.len()), (0, 0, 0), "{px}");
        let stroked = font.rasterize_indexed_with_stroke(index, px, StrokeSettings::new(2.0));
        assert_eq!((stroked.fill.len(), stroked.stroke.len()), (0, 0), "{px}");
    }
    let (metrics, bitmap) = font.rasterize_indexed(index, 400.0);
    assert!(metrics.width * metrics.height <= MAX_GLYPH_PIXELS);
    assert!(!bitmap.is_empty());
}
