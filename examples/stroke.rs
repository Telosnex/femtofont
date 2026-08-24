//! Renders independently colored fill and stroke layers in a true-color terminal.

use femtofont::{Font, FontSettings, LineJoin, StrokeSettings};

const CHARACTER: char = 'g';
const SIZE: f32 = 32.0;

// cargo run --example stroke --release
fn main() {
    let bytes = include_bytes!("../resources/fonts/Roboto-Regular.ttf") as &[u8];
    let font = Font::from_bytes(
        bytes,
        FontSettings {
            scale: SIZE,
            cachesize: 32,
            ..FontSettings::default()
        },
    )
    .unwrap();
    let glyph =
        font.rasterize_with_stroke(CHARACTER, SIZE, StrokeSettings::new(3.0).with_line_join(LineJoin::Round));

    for y in 0..glyph.metrics.height {
        for x in 0..glyph.metrics.width {
            let index = x + y * glyph.metrics.width;
            // Composite a blue stroke, then a white fill, over black.
            let stroke = glyph.stroke[index] as f32 / 255.0;
            let fill = glyph.fill[index] as f32 / 255.0;
            let mut rgb = [30.0 * stroke, 120.0 * stroke, 255.0 * stroke];
            for channel in &mut rgb {
                *channel += (255.0 - *channel) * fill;
            }
            print!("\x1B[48;2;{};{};{}m  ", rgb[0] as u8, rgb[1] as u8, rgb[2] as u8);
        }
        println!("\x1B[0m");
    }
}
