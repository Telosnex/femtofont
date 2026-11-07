use crate::layout::GlyphRasterConfig;
use crate::math::{Geometry, Line, Point, PreparedLine};
use crate::raster::Raster;
use crate::stroke::{StrokeGeometry, StrokeSettings};
use crate::unicode;
use crate::FontResult;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::*;
use core::cell::RefCell;
use core::f32::math::*;
use core::ops::Deref;
use ttf_parser::{Face, FaceParsingError, GlyphId, Tag};

/// Defines the bounds for a glyph's outline in subpixels. A glyph's outline is always contained in
/// its bitmap.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct OutlineBounds {
    /// Subpixel offset of the left-most edge of the glyph's outline.
    pub xmin: f32,
    /// Subpixel offset of the bottom-most edge of the glyph's outline.
    pub ymin: f32,
    /// The width of the outline in subpixels.
    pub width: f32,
    /// The height of the outline in subpixels.
    pub height: f32,
}

impl Default for OutlineBounds {
    fn default() -> Self {
        Self {
            xmin: 0.0,
            ymin: 0.0,
            width: 0.0,
            height: 0.0,
        }
    }
}

impl OutlineBounds {
    /// Scales the bounding box by the given factor.
    #[inline(always)]
    pub fn scale(&self, scale: f32) -> OutlineBounds {
        OutlineBounds {
            xmin: self.xmin * scale,
            ymin: self.ymin * scale,
            width: self.width * scale,
            height: self.height * scale,
        }
    }
}

/// Encapsulates all layout information associated with a glyph for a fixed scale.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Metrics {
    /// Whole pixel offset of the left-most edge of the bitmap. This may be negative to reflect the
    /// glyph is positioned to the left of the origin.
    pub xmin: i32,
    /// Whole pixel offset of the bottom-most edge of the bitmap. This may be negative to reflect
    /// the glyph is positioned below the baseline.
    pub ymin: i32,
    /// The width of the bitmap in whole pixels.
    pub width: usize,
    /// The height of the bitmap in whole pixels.
    pub height: usize,
    /// Advance width of the glyph in subpixels. Used in horizontal fonts.
    pub advance_width: f32,
    /// Advance height of the glyph in subpixels. Used in vertical fonts.
    pub advance_height: f32,
    /// The bounding box that contains the glyph's outline at the offsets specified by the font.
    /// This is always a smaller box than the bitmap bounds.
    pub bounds: OutlineBounds,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            xmin: 0,
            ymin: 0,
            width: 0,
            height: 0,
            advance_width: 0.0,
            advance_height: 0.0,
            bounds: OutlineBounds::default(),
        }
    }
}

/// Rasterized fill and stroke coverage sharing one bitmap coordinate system.
///
/// Both coverage vectors have `metrics.width * metrics.height` entries (or three entries per
/// physical pixel for subpixel methods). Paint `stroke` first and `fill` second to produce
/// conventional outlined text with independently chosen colors.
#[derive(Clone, Debug, PartialEq)]
pub struct StrokedGlyph {
    /// Sizing and positioning metadata for both aligned coverage layers.
    pub metrics: Metrics,
    /// Fill coverage aligned to the expanded stroke bitmap.
    pub fill: Vec<u8>,
    /// Centered stroke coverage aligned to the expanded stroke bitmap.
    pub stroke: Vec<u8>,
}

/// Metrics associated with line positioning.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct LineMetrics {
    /// The highest point that any glyph in the font extends to above the baseline. Typically
    /// positive.
    pub ascent: f32,
    /// The lowest point that any glyph in the font extends to below the baseline. Typically
    /// negative.
    pub descent: f32,
    /// The gap to leave between the descent of one line and the ascent of the next. This is of
    /// course only a guideline given by the font's designers.
    pub line_gap: f32,
    /// A precalculated value for the height or width of the line depending on if the font is laid
    /// out horizontally or vertically. It's calculated by: ascent - descent + line_gap.
    pub new_line_size: f32,
}

impl LineMetrics {
    /// Creates a new line metrics struct and computes the new line size.
    fn new(ascent: i16, descent: i16, line_gap: i16) -> LineMetrics {
        // Operations between this values can exceed i16, so we extend to i32 here.
        let (ascent, descent, line_gap) = (ascent as i32, descent as i32, line_gap as i32);
        LineMetrics {
            ascent: ascent as f32,
            descent: descent as f32,
            line_gap: line_gap as f32,
            new_line_size: (ascent - descent + line_gap) as f32,
        }
    }

    /// Scales the line metrics by the given factor.
    #[inline(always)]
    fn scale(&self, scale: f32) -> LineMetrics {
        LineMetrics {
            ascent: self.ascent * scale,
            descent: self.descent * scale,
            line_gap: self.line_gap * scale,
            new_line_size: self.new_line_size * scale,
        }
    }
}

/// Stores compiled geometry and metric information.
#[derive(Clone)]
pub(crate) struct Glyph {
    pub(crate) lines: GlyphLines,
    advance_width: f32,
    advance_height: f32,
    pub bounds: OutlineBounds,
    pub contour_points: Vec<Point>,
    pub contour_ends: Vec<usize>,
}

impl Default for Glyph {
    fn default() -> Self {
        Glyph {
            lines: GlyphLines::default(),
            advance_width: 0.0,
            advance_height: 0.0,
            bounds: OutlineBounds::default(),
            contour_points: Vec::new(),
            contour_ends: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) enum GlyphLines {
    Prepared {
        v: Vec<PreparedLine>,
        m: Vec<PreparedLine>,
    },
    Compact {
        v: Vec<Line>,
        m: Vec<Line>,
    },
}

impl Default for GlyphLines {
    fn default() -> Self {
        Self::Compact {
            v: Vec::new(),
            m: Vec::new(),
        }
    }
}

/// Storage strategy for temporary and cached fill-outline edges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GeometryStorage {
    /// Retain raster-ready edge parameters for maximum draw throughput.
    #[default]
    Prepared,
    /// Retain only edge coordinates and derive raster parameters while drawing.
    /// This reduces edge storage from 64 to 16 bytes.
    Compact,
}

/// Settings for controlling specific font and layout behavior.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FontSettings {
    /// The index of the font to use if parsing a font collection. (Default 0)
    pub collection_index: u32,
    /// The scale in px the font geometry is optimized for. Fonts rendered at
    /// the scale defined here will be the most optimal in terms of looks and performance. Glyphs
    /// rendered smaller than this scale will look the same but perform slightly worse, while
    /// glyphs rendered larger than this will looks worse but perform slightly better. The units of
    /// the scale are pixels per Em unit. (Default 40)
    pub scale: f32,
    /// The size of the cache, completely uncached if 0 (Default 0)
    pub cachesize: usize,
    /// Representation used for fill-outline edges. (Default Prepared)
    pub geometry_storage: GeometryStorage,
    /// Whether to retain dense ordered contours required by vector-stroke APIs.
    /// Fill rasterization does not use them. When false, `rasterize_with_stroke`
    /// and its indexed variants still return the normal fill but a fully
    /// transparent stroke layer. (Default true)
    pub retain_stroke_geometry: bool,
}

impl Default for FontSettings {
    fn default() -> FontSettings {
        FontSettings {
            collection_index: 0,
            scale: 40.0,
            cachesize: 0,
            geometry_storage: GeometryStorage::Prepared,
            retain_stroke_geometry: true,
        }
    }
}

#[derive(Clone)]
struct GlyphCache {
    /// Number of times the cache has been indexed
    age: usize,
    /// Glyph index, glyph, age of creation
    cache: Vec<(u16, Arc<Glyph>, usize)>,
    cap: usize,
}

impl GlyphCache {
    fn new(cap: usize) -> Self {
        Self {
            age: 0,
            cache: Vec::with_capacity(cap),
            cap,
        }
    }

    /// Accesses the cache while also mutating the age
    fn search(&mut self, idx: u16) -> Option<Arc<Glyph>> {
        self.age = self.age.wrapping_add(1);
        for entry in &mut self.cache {
            if entry.0 == idx {
                entry.2 = self.age;
                return Some(Arc::clone(&entry.1));
            }
        }
        None
    }

    /// Attempts to access the cache but without mutating its LRU state.
    #[allow(dead_code)]
    fn silent_search(&self, idx: u16) -> Option<Arc<Glyph>> {
        self.cache.iter().find(|entry| entry.0 == idx).map(|entry| Arc::clone(&entry.1))
    }

    fn insert(&mut self, idx: u16, g: Arc<Glyph>) {
        if self.cache.len() < self.cap {
            self.cache.push((idx, g, self.age));
        } else {
            let mut oldest_idx = 0;
            let mut oldest_age = usize::MAX;
            for i in 0..self.cache.len() {
                if self.cache[i].2 < oldest_age {
                    oldest_age = self.cache[i].2;
                    oldest_idx = i;
                }
            }

            self.cache[oldest_idx] = (idx, g, self.age);
        }
    }
}

enum GlyphRef {
    Owned(Glyph),
    Cached(Arc<Glyph>),
}

impl Deref for GlyphRef {
    type Target = Glyph;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(glyph) => glyph,
            Self::Cached(glyph) => glyph,
        }
    }
}

/// Represents a font. Fonts are immutable after creation and owns its own copy of the font data.
#[derive(Clone)]
pub struct Font<'a> {
    /// The font face, `Arc`'d so that a single font face can become multiple `Font`'s
    pub face: Arc<Face<'a>>,
    vertical_line_metrics: Option<LineMetrics>,
    scale: f32,
    cachesize: usize,
    cache: RefCell<GlyphCache>,
    geometry_storage: GeometryStorage,
    retain_stroke_geometry: bool,
}

impl<'a> core::fmt::Debug for Font<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Font").finish()
    }
}

/// Converts a ttf-parser FaceParsingError into a string.
fn convert_error(error: FaceParsingError) -> &'static str {
    use FaceParsingError::*;
    match error {
        MalformedFont => "An attempt to read out of bounds detected.",
        UnknownMagic => "Face data must start with 0x00010000, 0x74727565, 0x4F54544F or 0x74746366.",
        FaceIndexOutOfBounds => "The face index is larger than the number of faces in the font.",
        NoHeadTable => "The head table is missing or malformed.",
        NoHheaTable => "The hhea table is missing or malformed.",
        NoMaxpTable => "The maxp table is missing or malformed.",
    }
}

impl<'a> Font<'a> {
    /// Constructs a font from an array of bytes.
    pub fn from_bytes(data: &'a [u8], settings: FontSettings) -> FontResult<Font<'a>> {
        let face = match Face::parse(data, settings.collection_index) {
            Ok(f) => f,
            Err(e) => return Err(convert_error(e)),
        };

        Self::from_face(Arc::new(face), settings)
    }

    /// Constructs a font from a ttf_parser `Face`
    pub fn from_face(face: Arc<Face<'a>>, settings: FontSettings) -> FontResult<Font<'a>> {
        // New line metrics.
        let vertical_line_metrics = face.vertical_ascender().map(|ascender| {
            LineMetrics::new(
                ascender,
                face.vertical_descender().unwrap_or(0),
                face.vertical_line_gap().unwrap_or(0),
            )
        });

        Ok(Font {
            face,
            vertical_line_metrics,
            cache: RefCell::new(GlyphCache::new(settings.cachesize)),
            cachesize: settings.cachesize,
            scale: settings.scale,
            geometry_storage: settings.geometry_storage,
            retain_stroke_geometry: settings.retain_stroke_geometry,
        })
    }

    /// Constructs a font from an array of bytes and a weight.
    ///
    /// For more precise control, see `ttf_parser` `Face::set_variation` and `Font::from_face`
    pub fn from_bytes_and_weight(
        data: &'a [u8],
        weight: f32,
        settings: FontSettings,
    ) -> FontResult<Font<'a>> {
        let mut face = match Face::parse(data, settings.collection_index) {
            Ok(f) => f,
            Err(e) => return Err(convert_error(e)),
        };

        face.set_variation(Tag::from_bytes(b"wght"), weight);

        Self::from_face(Arc::new(face), settings)
    }

    fn generate_new_glyph(&self, index: u16) -> Result<Glyph, &'static str> {
        // A cmap may map a character past the glyph count. Draw .notdef,
        // as for a character the font does not have.
        let index = if index >= self.face.number_of_glyphs() {
            0
        } else {
            index
        };

        let mut glyph = Glyph::default();
        let glyph_id = GlyphId(index);
        if let Some(advance_width) = self.face.glyph_hor_advance(glyph_id) {
            glyph.advance_width = advance_width as f32;
        }
        if let Some(advance_height) = self.face.glyph_ver_advance(glyph_id) {
            glyph.advance_height = advance_height as f32;
        }

        let mut geometry =
            Geometry::new(self.scale, self.face.units_per_em() as f32, self.retain_stroke_geometry);
        self.face.outline_glyph(glyph_id, &mut geometry);
        geometry.finalize(&mut glyph, self.geometry_storage);
        Ok(glyph)
    }

    fn get_glyph(&self, idx: u16) -> Result<GlyphRef, &'static str> {
        if self.cachesize == 0 {
            return self.generate_new_glyph(idx).map(GlyphRef::Owned);
        }
        let mut cache = self.cache.borrow_mut();
        if let Some(glyph) = cache.search(idx) {
            Ok(GlyphRef::Cached(glyph))
        } else {
            let generated = Arc::new(self.generate_new_glyph(idx)?);
            cache.insert(idx, Arc::clone(&generated));
            Ok(GlyphRef::Cached(generated))
        }
    }

    fn get_kern_value(&self, left_glyph: u16, right_glyph: u16) -> Option<i16> {
        let kern = self.face.tables().kern?;

        // Iterate through all kern subtables
        Some(
            kern.subtables
                .into_iter()
                .filter(|subtable| {
                    // Only use horizontal, non-state-machine subtables
                    subtable.horizontal && !subtable.has_cross_stream && !subtable.has_state_machine
                })
                .filter_map(|subtable| subtable.glyphs_kerning(GlyphId(left_glyph), GlyphId(right_glyph)))
                .sum(),
        ) // Sum values from all matching subtables
    }

    /// Returns the font's face name at a certain ID if it has one.
    /// See <https://learn.microsoft.com/en-us/typography/opentype/spec/name#name-ids> for more info.
    pub fn name_with_id(&self, id: u16) -> Option<String> {
        if let Some(name) = self.face.names().get(id) {
            return Some(unicode::decode_utf16(name.name));
        }
        None
    }

    /// Returns the font's face name if it has one. It is from `Name ID 4` (Full Name) in the name table.
    /// See <https://learn.microsoft.com/en-us/typography/opentype/spec/name#name-ids> for more info.
    pub fn full_name(&self) -> Option<String> {
        self.name_with_id(4)
    }

    /// New line metrics for fonts that append characters to lines horizontally, and append new
    /// lines vertically (above or below the current line). Only populated for fonts with the
    /// appropriate metrics, none if it's missing.
    /// # Arguments
    ///
    /// * `px` - The size to scale the line metrics by. The units of the scale are pixels per Em
    /// unit.
    pub fn horizontal_line_metrics(&self, px: f32) -> LineMetrics {
        let metrics = LineMetrics::new(self.face.ascender(), self.face.descender(), self.face.line_gap());
        metrics.scale(self.scale_factor(px))
    }

    /// New line metrics for fonts that append characters to lines vertically, and append new
    /// lines horizontally (left or right of the current line). Only populated for fonts with the
    /// appropriate metrics, none if it's missing.
    /// # Arguments
    ///
    /// * `px` - The size to scale the line metrics by. The units of the scale are pixels per Em
    /// unit.
    pub fn vertical_line_metrics(&self, px: f32) -> Option<LineMetrics> {
        let metrics = self.vertical_line_metrics?;
        Some(metrics.scale(self.scale_factor(px)))
    }

    /// Calculates the glyph's outline scale factor for a given px size. The units of the scale are
    /// pixels per Em unit.
    #[inline(always)]
    pub fn scale_factor(&self, px: f32) -> f32 {
        px / self.face.units_per_em() as f32
    }

    /// Retrieves the horizontal scaled kerning value for two adjacent characters.
    /// # Arguments
    ///
    /// * `left` - The character on the left hand side of the pairing.
    /// * `right` - The character on the right hand side of the pairing.
    /// * `px` - The size to scale the kerning value for. The units of the scale are pixels per Em
    /// unit.
    /// # Returns
    ///
    /// * `Option<f32>` - The horizontal scaled kerning value if one is present in the font for the
    /// given left and right pair, None otherwise.
    #[inline(always)]
    pub fn horizontal_kern(&self, left: char, right: char, px: f32) -> Option<f32> {
        self.horizontal_kern_indexed(self.lookup_glyph_index(left), self.lookup_glyph_index(right), px)
    }

    /// Retrieves the horizontal scaled kerning value for two adjacent glyph indicies.
    /// # Arguments
    ///
    /// * `left` - The glyph index on the left hand side of the pairing.
    /// * `right` - The glyph index on the right hand side of the pairing.
    /// * `px` - The size to scale the kerning value for. The units of the scale are pixels per Em
    /// unit.
    /// # Returns
    ///
    /// * `Option<f32>` - The horizontal scaled kerning value if one is present in the font for the
    /// given left and right pair, None otherwise.
    #[inline(always)]
    pub fn horizontal_kern_indexed(&self, left: u16, right: u16, px: f32) -> Option<f32> {
        let scale = self.scale_factor(px);
        let value = self.get_kern_value(left, right)?;
        Some((value as f32) * scale)
    }

    /// Retrieves the scaled horizontal pen advance for a character without
    /// generating or caching its outline geometry. Missing characters use the
    /// font's default glyph, matching [`Self::metrics`].
    ///
    /// # Arguments
    ///
    /// * `character` - The character whose glyph advance should be measured.
    /// * `px` - The size to scale the advance by, in pixels per Em unit.
    #[inline(always)]
    pub fn horizontal_advance(&self, character: char, px: f32) -> f32 {
        self.horizontal_advance_indexed(self.lookup_glyph_index(character), px)
    }

    /// Retrieves the scaled horizontal pen advance for a glyph index without
    /// generating or caching its outline geometry. You normally want
    /// [`Self::horizontal_advance`] unless glyphs are already indexed.
    ///
    /// # Panics
    ///
    /// Panics when `index` is outside the font's glyph range, matching
    /// [`Self::metrics_indexed`].
    #[inline(always)]
    pub fn horizontal_advance_indexed(&self, index: u16, px: f32) -> f32 {
        assert!(index < self.face.number_of_glyphs(), "Invalid Index");
        let advance = self.face.glyph_hor_advance(GlyphId(index)).unwrap_or(0);
        f32::from(advance) * self.scale_factor(px)
    }

    /// Retrieves the layout metrics for the given character. If the character isn't present in the
    /// font, then the layout for the font's default character is returned instead.
    /// # Arguments
    ///
    /// * `index` - The character in the font to to generate the layout metrics for.
    /// * `px` - The size to generate the layout metrics for the character at. Cannot be negative.
    /// The units of the scale are pixels per Em unit.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the glyph.
    #[inline]
    pub fn metrics(&self, character: char, px: f32) -> Metrics {
        self.metrics_indexed(self.lookup_glyph_index(character), px)
    }

    /// Retrieves the layout metrics at the given index. You normally want to be using
    /// metrics(char, f32) instead, unless your glyphs are pre-indexed.
    /// # Arguments
    ///
    /// * `index` - The glyph index in the font to to generate the layout metrics for.
    /// * `px` - The size to generate the layout metrics for the glyph at. Cannot be negative. The
    /// units of the scale are pixels per Em unit.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the glyph.
    pub fn metrics_indexed(&self, index: u16, px: f32) -> Metrics {
        let glyph = self.get_glyph(index).expect("Invalid Index");
        let scale = self.scale_factor(px);
        let (metrics, _, _) = self.metrics_raw(scale, &glyph, 0.0);
        metrics
    }

    /// Internal function to generate the metrics, offset_x, and offset_y of the glyph.
    fn metrics_raw(&self, scale: f32, glyph: &Glyph, offset: f32) -> (Metrics, f32, f32) {
        let bounds = glyph.bounds.scale(scale);
        let mut offset_x = fract(bounds.xmin + offset);
        let mut offset_y = fract(1.0 - fract(bounds.height) - fract(bounds.ymin));
        if offset_x < 0.0 {
            offset_x += 1.0;
        }
        if offset_y < 0.0 {
            offset_y += 1.0;
        }
        let (width, height) =
            limit_glyph_pixels(ceil(bounds.width + offset_x) as usize, ceil(bounds.height + offset_y) as usize);
        let metrics = Metrics {
            xmin: floor(bounds.xmin) as i32,
            ymin: floor(bounds.ymin) as i32,
            width,
            height,
            advance_width: scale * glyph.advance_width,
            advance_height: scale * glyph.advance_height,
            bounds,
        };
        (metrics, offset_x, offset_y)
    }

    /// Rasterizes aligned fill and stroke layers for a layout raster configuration.
    #[inline]
    pub fn rasterize_config_with_stroke(
        &self,
        config: GlyphRasterConfig,
        settings: StrokeSettings,
    ) -> StrokedGlyph {
        self.rasterize_indexed_with_stroke(config.glyph_index, config.px, settings)
    }

    /// Rasterizes aligned fill and stroke layers for a character.
    #[inline]
    pub fn rasterize_with_stroke(&self, character: char, px: f32, settings: StrokeSettings) -> StrokedGlyph {
        self.rasterize_indexed_with_stroke(self.lookup_glyph_index(character), px, settings)
    }

    /// Rasterizes aligned fill and stroke layers for a glyph index.
    pub fn rasterize_indexed_with_stroke(
        &self,
        index: u16,
        px: f32,
        settings: StrokeSettings,
    ) -> StrokedGlyph {
        self.rasterize_indexed_stroke_impl(index, px, settings, false, true)
    }

    /// Rasterizes only stroke coverage for a layout raster configuration.
    #[inline]
    pub fn rasterize_config_stroke(
        &self,
        config: GlyphRasterConfig,
        settings: StrokeSettings,
    ) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed_stroke(config.glyph_index, config.px, settings)
    }

    /// Rasterizes only stroke coverage for a character.
    #[inline]
    pub fn rasterize_stroke(&self, character: char, px: f32, settings: StrokeSettings) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed_stroke(self.lookup_glyph_index(character), px, settings)
    }

    /// Rasterizes only stroke coverage for a glyph index.
    pub fn rasterize_indexed_stroke(
        &self,
        index: u16,
        px: f32,
        settings: StrokeSettings,
    ) -> (Metrics, Vec<u8>) {
        let rasterized = self.rasterize_indexed_stroke_impl(index, px, settings, false, false);
        (rasterized.metrics, rasterized.stroke)
    }

    /// Rasterizes aligned RGB-subpixel fill and stroke layers for a layout raster configuration.
    #[inline]
    pub fn rasterize_config_with_stroke_subpixel(
        &self,
        config: GlyphRasterConfig,
        settings: StrokeSettings,
    ) -> StrokedGlyph {
        self.rasterize_indexed_with_stroke_subpixel(config.glyph_index, config.px, settings)
    }

    /// Rasterizes aligned RGB-subpixel fill and stroke layers for a character.
    #[inline]
    pub fn rasterize_with_stroke_subpixel(
        &self,
        character: char,
        px: f32,
        settings: StrokeSettings,
    ) -> StrokedGlyph {
        self.rasterize_indexed_with_stroke_subpixel(self.lookup_glyph_index(character), px, settings)
    }

    /// Rasterizes aligned RGB-subpixel fill and stroke layers for a glyph index.
    pub fn rasterize_indexed_with_stroke_subpixel(
        &self,
        index: u16,
        px: f32,
        settings: StrokeSettings,
    ) -> StrokedGlyph {
        self.rasterize_indexed_stroke_impl(index, px, settings, true, true)
    }

    /// Rasterizes only RGB-subpixel stroke coverage for a layout raster configuration.
    #[inline]
    pub fn rasterize_config_stroke_subpixel(
        &self,
        config: GlyphRasterConfig,
        settings: StrokeSettings,
    ) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed_stroke_subpixel(config.glyph_index, config.px, settings)
    }

    /// Rasterizes only RGB-subpixel stroke coverage for a character.
    #[inline]
    pub fn rasterize_stroke_subpixel(
        &self,
        character: char,
        px: f32,
        settings: StrokeSettings,
    ) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed_stroke_subpixel(self.lookup_glyph_index(character), px, settings)
    }

    /// Rasterizes only RGB-subpixel stroke coverage for a glyph index.
    pub fn rasterize_indexed_stroke_subpixel(
        &self,
        index: u16,
        px: f32,
        settings: StrokeSettings,
    ) -> (Metrics, Vec<u8>) {
        let rasterized = self.rasterize_indexed_stroke_impl(index, px, settings, true, false);
        (rasterized.metrics, rasterized.stroke)
    }

    /// Retrieves the layout rasterized bitmap for the given raster config. If the raster config's
    /// character isn't present in the font, then the layout and bitmap for the font's default
    /// character's raster is returned instead.
    /// # Arguments
    ///
    /// * `config` - The settings to render the character at.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the rasterized glyph.
    /// * `Vec<u8>` - Coverage vector for the glyph. Coverage is a linear scale where 0 represents
    /// 0% coverage of that pixel by the glyph and 255 represents 100% coverage. The vec starts at
    /// the top left corner of the glyph.
    #[inline]
    pub fn rasterize_config(&self, config: GlyphRasterConfig) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed(config.glyph_index, config.px)
    }

    /// Retrieves the layout metrics and rasterized bitmap for the given character. If the
    /// character isn't present in the font, then the layout and bitmap for the font's default
    /// character is returned instead.
    /// # Arguments
    ///
    /// * `character` - The character to rasterize.
    /// * `px` - The size to render the character at. Cannot be negative. The units of the scale
    /// are pixels per Em unit.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the rasterized glyph.
    /// * `Vec<u8>` - Coverage vector for the glyph. Coverage is a linear scale where 0 represents
    /// 0% coverage of that pixel by the glyph and 255 represents 100% coverage. The vec starts at
    /// the top left corner of the glyph.
    #[inline]
    pub fn rasterize(&self, character: char, px: f32) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed(self.lookup_glyph_index(character), px)
    }

    /// Retrieves the layout rasterized bitmap for the given raster config. If the raster config's
    /// character isn't present in the font, then the layout and bitmap for the font's default
    /// character's raster is returned instead.
    ///
    /// This will perform the operation with the width multiplied by 3, as to simulate subpixels.
    /// Taking these as RGB values will perform subpixel anti aliasing.
    /// # Arguments
    ///
    /// * `config` - The settings to render the character at.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the rasterized glyph.
    /// * `Vec<u8>` - Swizzled RGB coverage vector for the glyph. Coverage is a linear scale where 0
    /// represents 0% coverage of that subpixel by the glyph and 255 represents 100% coverage. The
    /// vec starts at the top left corner of the glyph.
    #[inline]
    pub fn rasterize_config_subpixel(&self, config: GlyphRasterConfig) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed_subpixel(config.glyph_index, config.px)
    }

    /// Retrieves the layout metrics and rasterized bitmap for the given character. If the
    /// character isn't present in the font, then the layout and bitmap for the font's default
    /// character is returned instead.
    ///
    /// This will perform the operation with the width multiplied by 3, as to simulate subpixels.
    /// Taking these as RGB values will perform subpixel anti aliasing.
    /// # Arguments
    ///
    /// * `character` - The character to rasterize.
    /// * `px` - The size to render the character at. Cannot be negative. The units of the scale
    /// are pixels per Em unit.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the rasterized glyph.
    /// * `Vec<u8>` - Swizzled RGB coverage vector for the glyph. Coverage is a linear scale where 0
    /// represents 0% coverage of that subpixel by the glyph and 255 represents 100% coverage. The
    /// vec starts at the top left corner of the glyph.
    #[inline]
    pub fn rasterize_subpixel(&self, character: char, px: f32) -> (Metrics, Vec<u8>) {
        self.rasterize_indexed_subpixel(self.lookup_glyph_index(character), px)
    }

    /// Retrieves the layout metrics and rasterized bitmap at the given index. You normally want to
    /// be using rasterize(char, f32) instead, unless your glyphs are pre-indexed.
    /// # Arguments
    ///
    /// * `index` - The glyph index in the font to rasterize.
    /// * `px` - The size to render the character at. Cannot be negative. The units of the scale
    /// are pixels per Em unit.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the rasterized glyph.
    /// * `Vec<u8>` - Coverage vector for the glyph. Coverage is a linear scale where 0 represents
    /// 0% coverage of that pixel by the glyph and 255 represents 100% coverage. The vec starts at
    /// the top left corner of the glyph.
    pub fn rasterize_indexed(&self, index: u16, px: f32) -> (Metrics, Vec<u8>) {
        if !px.is_finite() || px <= 0.0 {
            return (Metrics::default(), Vec::new());
        }
        let glyph = &self.get_glyph(index).expect("Invalid Index");
        let scale = self.scale_factor(px);
        let (metrics, offset_x, offset_y) = self.metrics_raw(scale, glyph, 0.0);
        let mut canvas = Raster::new(metrics.width, metrics.height);
        if metrics.width > 0 && metrics.height > 0 {
            canvas.draw(glyph, scale, scale, offset_x, offset_y);
        }
        (metrics, canvas.get_bitmap())
    }

    /// Retrieves the layout metrics and rasterized bitmap at the given index. You normally want to
    /// be using rasterize(char, f32) instead, unless your glyphs are pre-indexed.
    ///
    /// This will perform the operation with the width multiplied by 3, as to simulate subpixels.
    /// Taking these as RGB values will perform subpixel anti aliasing.
    /// # Arguments
    ///
    /// * `index` - The glyph index in the font to rasterize.
    /// * `px` - The size to render the character at. Cannot be negative. The units of the scale
    /// are pixels per Em unit.
    /// # Returns
    ///
    /// * `Metrics` - Sizing and positioning metadata for the rasterized glyph.
    /// * `Vec<u8>` - Swizzled RGB coverage vector for the glyph. Coverage is a linear scale where 0
    /// represents 0% coverage of that subpixel by the glyph and 255 represents 100% coverage. The
    /// vec starts at the top left corner of the glyph.
    pub fn rasterize_indexed_subpixel(&self, index: u16, px: f32) -> (Metrics, Vec<u8>) {
        if !px.is_finite() || px <= 0.0 {
            return (Metrics::default(), Vec::new());
        }
        let glyph = &self.get_glyph(index).expect("Invalid Index");
        let scale = self.scale_factor(px);
        let (metrics, offset_x, offset_y) = self.metrics_raw(scale, glyph, 0.0);
        let mut canvas = Raster::new(metrics.width * 3, metrics.height);
        if metrics.width > 0 && metrics.height > 0 {
            canvas.draw(glyph, scale * 3.0, scale, offset_x, offset_y);
        }
        (metrics, canvas.get_bitmap())
    }

    fn rasterize_indexed_stroke_impl(
        &self,
        index: u16,
        px: f32,
        settings: StrokeSettings,
        subpixel: bool,
        include_fill: bool,
    ) -> StrokedGlyph {
        if !px.is_finite() || px <= 0.0 {
            return StrokedGlyph {
                metrics: Metrics::default(),
                fill: Vec::new(),
                stroke: Vec::new(),
            };
        }

        let glyph = self.get_glyph(index).expect("Invalid Index");
        let scale = self.scale_factor(px);
        let stroke_geometry = StrokeGeometry::new(&glyph, scale, settings);
        let fill_bounds = glyph.bounds.scale(scale);
        let stroke_is_empty = stroke_geometry.v_lines.is_empty() && stroke_geometry.m_lines.is_empty();
        let bounds = if stroke_is_empty {
            if include_fill {
                fill_bounds
            } else {
                OutlineBounds::default()
            }
        } else if include_fill {
            union_bounds(fill_bounds, stroke_geometry.bounds)
        } else {
            stroke_geometry.bounds
        };
        let metrics = metrics_for_bounds(bounds, &glyph, scale);
        let x_multiplier = if subpixel {
            3
        } else {
            1
        };
        let raster_width = metrics.width * x_multiplier;
        let bitmap_len = raster_width * metrics.height;
        let canvas_left = metrics.xmin as f32;
        let canvas_top = metrics.ymin as f32 + metrics.height as f32;

        let fill = if include_fill {
            let mut canvas = Raster::new(raster_width, metrics.height);
            if metrics.width > 0 && metrics.height > 0 {
                let fill_top = fill_bounds.ymin + fill_bounds.height;
                canvas.draw(
                    &glyph,
                    scale * x_multiplier as f32,
                    scale,
                    (fill_bounds.xmin - canvas_left) * x_multiplier as f32,
                    canvas_top - fill_top,
                );
            }
            canvas.get_bitmap()
        } else {
            Vec::new()
        };

        let stroke = if stroke_is_empty || metrics.width == 0 || metrics.height == 0 {
            vec![0; bitmap_len]
        } else {
            let mut canvas = Raster::new(raster_width, metrics.height);
            let stroke_top = stroke_geometry.bounds.ymin + stroke_geometry.bounds.height;
            canvas.draw_lines(
                &stroke_geometry.v_lines,
                &stroke_geometry.m_lines,
                x_multiplier as f32,
                1.0,
                (stroke_geometry.bounds.xmin - canvas_left) * x_multiplier as f32,
                canvas_top - stroke_top,
            );
            canvas.get_bitmap()
        };

        StrokedGlyph {
            metrics,
            fill,
            stroke,
        }
    }

    /// Checks if the font has a glyph for the given character.
    #[inline]
    pub fn has_glyph(&self, character: char) -> bool {
        self.lookup_glyph_index(character) != 0
    }

    /// Finds the internal glyph index for the given character. If the character is not present in
    /// the font then 0 is returned.
    #[inline]
    pub fn lookup_glyph_index(&self, character: char) -> u16 {
        self.face.glyph_index(character).map(|id| id.0).unwrap_or(0)
    }
}

/// The most pixels one glyph bitmap may have. Huge outline coordinates or
/// a tiny units-per-em in a hostile font can otherwise ask for a bitmap of
/// any size. A glyph past this limit is drawn blank, with zero size.
pub const MAX_GLYPH_PIXELS: usize = 1 << 20;

fn limit_glyph_pixels(width: usize, height: usize) -> (usize, usize) {
    if width.saturating_mul(height) > MAX_GLYPH_PIXELS {
        (0, 0)
    } else {
        (width, height)
    }
}

fn metrics_for_bounds(bounds: OutlineBounds, glyph: &Glyph, scale: f32) -> Metrics {
    let xmax = bounds.xmin + bounds.width;
    let ymax = bounds.ymin + bounds.height;
    let xmin = floor(bounds.xmin) as i32;
    let ymin = floor(bounds.ymin) as i32;
    let right = ceil(xmax) as i32;
    let top = ceil(ymax) as i32;
    let (width, height) =
        limit_glyph_pixels(right.saturating_sub(xmin).max(0) as usize, top.saturating_sub(ymin).max(0) as usize);
    Metrics {
        xmin,
        ymin,
        width,
        height,
        advance_width: scale * glyph.advance_width,
        advance_height: scale * glyph.advance_height,
        bounds,
    }
}

fn union_bounds(a: OutlineBounds, b: OutlineBounds) -> OutlineBounds {
    let xmin = a.xmin.min(b.xmin);
    let ymin = a.ymin.min(b.ymin);
    let xmax = (a.xmin + a.width).max(b.xmin + b.width);
    let ymax = (a.ymin + a.height).max(b.ymin + b.height);
    OutlineBounds {
        xmin,
        ymin,
        width: xmax - xmin,
        height: ymax - ymin,
    }
}
