//! Generates grayscale output for Fontdue. There are also examples for doing a similar operation in
//! Rusttype.

use std::fs::File;
use std::io::Write;

// Scratch pad for glyphs: ⅞ g ₹
const CHARACTER: char = '₹';
const SIZE: f32 = 200.0;
const PATH: &[u8] = include_bytes!("../resources/fonts/GreatVibes-Regular.otf") as &[u8];

// cargo run --example simple --release
pub fn main() {
    generate_femtofont();
}

pub fn generate_femtofont() {
    // Read the font data.
    let font = PATH;
    // Setup the configuration for how the font will be parsed.
    let settings = femtofont::FontSettings {
        scale: SIZE,
        ..femtofont::FontSettings::default()
    };
    // Parse it into the font type.
    let font = femtofont::Font::from_bytes(font, settings).unwrap();
    // Rasterize and get the layout metrics for the character at a size.
    let (metrics, bitmap) = font.rasterize(CHARACTER, SIZE);

    // Output
    let mut o = File::create("femtofont.pgm").unwrap();
    let _ = o.write(format!("P5\n{} {}\n255\n", metrics.width, metrics.height).as_bytes());
    let _ = o.write(&bitmap);
}

pub fn generate_rusttype() {
    // Loading and rasterization
    let font = PATH;
    let font = rusttype::Font::try_from_bytes(font).unwrap();
    let glyph =
        font.glyph(CHARACTER).scaled(rusttype::Scale::uniform(SIZE)).positioned(rusttype::point(0.0, 0.0));
    let (height, width) = if let Some(rect) = glyph.pixel_bounding_box() {
        (rect.height(), rect.width())
    } else {
        (0, 0)
    };
    let mut bitmap = vec![0u8; (width * height) as usize];
    glyph.draw(|x, y, v| {
        bitmap[(x as usize) + (y as usize) * width as usize] = (v * 255.0) as u8;
    });

    // Output
    let mut o = File::create("rusttype.pgm").unwrap();
    let _ = o.write(format!("P5\n{} {}\n255\n", width, height).as_bytes());
    let _ = o.write(&bitmap);
}
