//! Watermark: stamp an image (a logo, or text the caller rasterized) onto
//! selected pages, optionally flattening the result so it cannot be removed.
//!
//! The stamp is built once as a form XObject and every placement references
//! it, so a watermark on hundreds of pages, or tiled across each page, stores
//! the image only once.

use std::ffi::c_int;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use pdfium_render::prelude::*;

use crate::images::{parse_page_ranges, rotate_to_rgb, Rotation};
use crate::raw::{self, RawDocument, RawPage};
use crate::{
    ensure_not_input, flatten, part_path, remove_part, FlattenError, FlattenOptions, Progress,
    Summary,
};

/// Largest stamp edge in pixels. Text rendered at this size is sharp well
/// beyond print resolution at any sensible watermark size.
const MAX_STAMP_PIXELS: u32 = 4096;

/// The watermark picture, with straight (not premultiplied) alpha.
#[derive(Debug, Clone)]
pub struct Stamp {
    pub width: u32,
    pub height: u32,
    /// Pixels in R, G, B, A byte order.
    pub rgba: Vec<u8>,
}

impl Stamp {
    /// Decodes a PNG (any bit depth or color type) into a stamp.
    pub fn from_png(bytes: &[u8]) -> Result<Self, FlattenError> {
        let invalid =
            |e: png::DecodingError| FlattenError::InvalidOptions(format!("watermark image: {e}"));
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(invalid)?;
        let mut buf = vec![
            0;
            reader.output_buffer_size().ok_or_else(|| {
                FlattenError::InvalidOptions("watermark image is too large".into())
            })?
        ];
        let info = reader.next_frame(&mut buf).map_err(invalid)?;
        buf.truncate(info.buffer_size());
        let (width, height) = (info.width, info.height);
        if width == 0 || height == 0 || width.max(height) > MAX_STAMP_PIXELS {
            return Err(FlattenError::InvalidOptions(format!(
                "watermark image must be between 1 and {MAX_STAMP_PIXELS} pixels on each side"
            )));
        }
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf
                .chunks_exact(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => buf
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => unreachable!("expanded by normalize_to_color8"),
        };
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

/// Where the stamp goes on each page, as seen when the page is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    /// Repeated across the whole page.
    Tile,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PageSelection {
    #[default]
    All,
    /// Only these pages, e.g. "1-3, 7".
    Only(String),
    /// Every page except these, e.g. "1, 10-".
    Except(String),
}

impl PageSelection {
    /// 0-based page indices this selection picks from a document.
    pub fn resolve(&self, page_count: usize) -> Result<Vec<usize>, FlattenError> {
        Ok(match self {
            PageSelection::All => (0..page_count).collect(),
            PageSelection::Only(spec) => parse_page_ranges(spec, page_count)?,
            PageSelection::Except(spec) if spec.trim().is_empty() => (0..page_count).collect(),
            PageSelection::Except(spec) => {
                let skip = parse_page_ranges(spec, page_count)?;
                (0..page_count).filter(|i| !skip.contains(i)).collect()
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct WatermarkOptions {
    pub stamp: Stamp,
    /// Stamp width as a fraction of the displayed page width (0.02-1.5).
    pub size: f32,
    /// Counter-clockwise rotation in degrees (-180 to 180).
    pub rotation: f32,
    /// 0 (invisible) to 1 (solid).
    pub opacity: f32,
    pub position: Position,
    /// Distance from the page edges as a fraction of the shorter side.
    pub margin: f32,
    pub pages: PageSelection,
    pub password: Option<String>,
    /// Flatten the result afterwards so the watermark cannot be removed.
    pub flatten: Option<FlattenOptions>,
}

impl WatermarkOptions {
    pub fn new(stamp: Stamp) -> Self {
        Self {
            stamp,
            size: 0.5,
            rotation: 0.0,
            opacity: 0.3,
            position: Position::Center,
            margin: 0.04,
            pages: PageSelection::All,
            password: None,
            flatten: None,
        }
    }

    fn validate(&self) -> Result<(), FlattenError> {
        let check = |ok: bool, what: &str| {
            if ok {
                Ok(())
            } else {
                Err(FlattenError::InvalidOptions(what.into()))
            }
        };
        check(
            (0.02..=1.5).contains(&self.size),
            "watermark size must be between 2% and 150% of the page width",
        )?;
        check(
            (-180.0..=180.0).contains(&self.rotation),
            "rotation must be between -180 and 180 degrees",
        )?;
        check(
            (0.0..=1.0).contains(&self.opacity),
            "opacity must be between 0 and 1",
        )?;
        check(
            (0.0..=0.4).contains(&self.margin),
            "margin must be between 0 and 0.4",
        )
    }
}

/// Stamps `input` and writes the result to `output`.
///
/// With `opts.flatten` set, the stamped document is flattened as well and
/// progress follows the flatten step; otherwise it follows the stamping.
/// `on_progress` may be called from a worker thread.
pub fn watermark(
    pdfium: &Pdfium,
    input: &Path,
    output: &Path,
    opts: &WatermarkOptions,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<Summary, FlattenError> {
    opts.validate()?;
    ensure_not_input(input, output)?;
    let started = Instant::now();
    // Distinct from flatten's own `<output>.part`, which it writes while
    // reading this file.
    let stamped_path = part_path(&part_path(output));
    let password = opts.password.as_deref().filter(|p| !p.is_empty());

    let result = (|| -> Result<Summary, FlattenError> {
        let doc = RawDocument::open(input, password)?;
        let quiet = |_: Progress| {};
        let stamp_progress: &(dyn Fn(Progress) + Sync) = if opts.flatten.is_some() {
            &quiet
        } else {
            on_progress
        };
        let pages = stamp_document(&doc, opts, cancel, stamp_progress)?;

        let Some(flatten_opts) = &opts.flatten else {
            doc.save_to_file(&stamped_path)?;
            drop(doc);
            fs::rename(&stamped_path, output)?;
            return Ok(Summary {
                pages,
                output_bytes: fs::metadata(output)?.len(),
                output: output.to_path_buf(),
                elapsed: started.elapsed(),
            });
        };

        // Flatten from an intermediate stamped copy (it keeps the input's
        // encryption, so it needs the same password).
        doc.save_to_file(&stamped_path)?;
        drop(doc);
        let flatten_opts = FlattenOptions {
            password: opts.password.clone(),
            ..flatten_opts.clone()
        };
        let mut summary = flatten(
            pdfium,
            &stamped_path,
            output,
            &flatten_opts,
            cancel,
            on_progress,
        )?;
        remove_part(&stamped_path);
        summary.elapsed = started.elapsed();
        Ok(summary)
    })();

    if result.is_err() {
        remove_part(&stamped_path);
    }
    result
}

/// Renders page `page_index` (0-based) of `input` with the watermark applied,
/// fitted within `max_pixels` on its longer side. Returns a PNG.
///
/// The page is stamped only if `opts.pages` selects it, so the preview also
/// shows when a page is left out. `opts.flatten` is ignored.
pub fn watermark_preview(
    pdfium: &Pdfium,
    input: &Path,
    page_index: usize,
    opts: &WatermarkOptions,
    max_pixels: u32,
) -> Result<Vec<u8>, FlattenError> {
    opts.validate()?;
    let password = opts.password.as_deref().filter(|p| !p.is_empty());
    let source = RawDocument::open(input, password)?;
    let page_count = source.page_count();
    if page_index >= page_count {
        return Err(FlattenError::InvalidOptions(format!(
            "page {} does not exist (the document has {page_count} pages)",
            page_index + 1
        )));
    }

    // Stamp a one-page copy so the preview never touches the source.
    let single = RawDocument::new_empty()?;
    single.import_page(&source, page_index)?;
    drop(source);
    if opts.pages.resolve(page_count)?.contains(&page_index) {
        let template = Template::new(&single, &opts.stamp, opts.opacity)?;
        stamp_page(&single.load_page(0)?, &template, opts)?;
    }
    let bytes = single.save(Vec::new())?;
    drop(single);

    let document = pdfium.load_pdf_from_byte_vec(bytes, None)?;
    let page = document.pages().get(0)?;
    let longest = page.width().value.max(page.height().value);
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(max_pixels as f32 / longest)
        .set_format(PdfBitmapFormat::BGRA)
        .set_reverse_byte_order(true)
        .set_clear_color(PdfColor::WHITE)
        .render_form_data(true)
        .render_annotations(true);
    let bitmap = page.render_with_config(&config)?;
    let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
    let (rgb, width, height) = rotate_to_rgb(&bitmap.as_raw_bytes(), width, height, Rotation::None);

    let fail = |e: png::EncodingError| FlattenError::Encode {
        page: page_index + 1,
        message: e.to_string(),
    };
    let mut png_bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(&rgb).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(png_bytes)
}

/// Stamps the selected pages of `doc` in place. Returns the page count.
fn stamp_document(
    doc: &RawDocument,
    opts: &WatermarkOptions,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<usize, FlattenError> {
    let page_count = doc.page_count();
    let selected = opts.pages.resolve(page_count)?;
    on_progress(Progress::Started {
        total_pages: selected.len(),
    });
    let template = Template::new(doc, &opts.stamp, opts.opacity)?;
    for (done, &index) in selected.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(FlattenError::Cancelled);
        }
        stamp_page(&doc.load_page(index)?, &template, opts)?;
        on_progress(Progress::PageDone {
            done: done + 1,
            total_pages: selected.len(),
        });
    }
    Ok(page_count)
}

fn stamp_page(
    page: &RawPage,
    template: &Template,
    opts: &WatermarkOptions,
) -> Result<(), FlattenError> {
    let (to_page, display_w, display_h) = display_to_page(page.visible_box(), page.quarter_turns());
    // Form space is the template page: one point per stamp pixel.
    let from_form = Affine::scale(1.0 / template.width as f64, 1.0 / template.height as f64);
    let aspect = template.height as f64 / template.width as f64;
    for placement in placements(display_w, display_h, aspect, opts) {
        let matrix = from_form.then(&placement).then(&to_page);
        page.insert(template.place(&matrix)?)?;
    }
    page.generate_content()
}

/// The stamp as a form XObject in one destination document.
struct Template {
    raw: &'static dyn PdfiumLibraryBindings,
    xobject: FPDF_XOBJECT,
    width: u32,
    height: u32,
}

impl Template {
    fn new(destination: &RawDocument, stamp: &Stamp, opacity: f32) -> Result<Self, FlattenError> {
        let raw = raw::bindings()?;
        // Draw the stamp on a page of its own, one point per pixel, then turn
        // that page into a form XObject inside the destination document.
        let scratch = RawDocument::new_empty()?;
        let (w, h) = (stamp.width, stamp.height);
        unsafe {
            let page = raw.FPDFPage_New(scratch.handle, 0, w as f64, h as f64);
            if page.is_null() {
                return Err(FlattenError::PdfiumFailed("create the watermark page"));
            }
            let result = (|| {
                let bitmap = raw.FPDFBitmap_Create(w as c_int, h as c_int, 1);
                if bitmap.is_null() {
                    return Err(FlattenError::PdfiumFailed("create the watermark bitmap"));
                }
                fill_bgra(raw, bitmap, stamp, opacity);
                let image = raw.FPDFPageObj_NewImageObj(scratch.handle);
                let set = raw.FPDFImageObj_SetBitmap(std::ptr::null_mut(), 0, image, bitmap);
                raw.FPDFBitmap_Destroy(bitmap);
                if !raw.is_true(set) {
                    raw.FPDFPageObj_Destroy(image);
                    return Err(FlattenError::PdfiumFailed("load the watermark image"));
                }
                raw.FPDFPageObj_SetMatrix(image, &Affine::scale(w as f64, h as f64).into());
                if !raw.is_true(raw.FPDFPage_InsertObject(page, image)) {
                    raw.FPDFPageObj_Destroy(image);
                    return Err(FlattenError::PdfiumFailed("place the watermark image"));
                }
                if !raw.is_true(raw.FPDFPage_GenerateContent(page)) {
                    return Err(FlattenError::PdfiumFailed("write the watermark page"));
                }
                Ok(())
            })();
            raw.FPDF_ClosePage(page);
            result?;
        }
        let xobject = unsafe { raw.FPDF_NewXObjectFromPage(destination.handle, scratch.handle, 0) };
        if xobject.is_null() {
            return Err(FlattenError::PdfiumFailed("create the watermark template"));
        }
        Ok(Self {
            raw,
            xobject,
            width: w,
            height: h,
        })
    }

    /// A new form object showing the stamp, transformed by `matrix`.
    fn place(&self, matrix: &Affine) -> Result<FPDF_PAGEOBJECT, FlattenError> {
        let object = unsafe { self.raw.FPDF_NewFormObjectFromXObject(self.xobject) };
        if object.is_null() {
            return Err(FlattenError::PdfiumFailed("place the watermark"));
        }
        unsafe { self.raw.FPDFPageObj_SetMatrix(object, &(*matrix).into()) };
        Ok(object)
    }
}

impl Drop for Template {
    fn drop(&mut self) {
        unsafe { self.raw.FPDF_CloseXObject(self.xobject) };
    }
}

/// Copies the stamp into a BGRA PDFium bitmap, scaling alpha by `opacity`.
unsafe fn fill_bgra(
    raw: &dyn PdfiumLibraryBindings,
    bitmap: FPDF_BITMAP,
    stamp: &Stamp,
    opacity: f32,
) {
    let stride = raw.FPDFBitmap_GetStride(bitmap) as usize;
    let buffer = raw.FPDFBitmap_GetBuffer(bitmap) as *mut u8;
    let row_bytes = stamp.width as usize * 4;
    for (y, row) in stamp.rgba.chunks_exact(row_bytes).enumerate() {
        let out = std::slice::from_raw_parts_mut(buffer.add(y * stride), row_bytes);
        for (src, dst) in row.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = (src[3] as f32 * opacity).round() as u8;
        }
    }
}

/// A 2D affine transform in PDF form: x' = a·x + c·y + e, y' = b·x + d·y + f.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Affine {
    const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    fn scale(x: f64, y: f64) -> Self {
        Self::new(x, 0.0, 0.0, y, 0.0, 0.0)
    }

    fn translate(x: f64, y: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, x, y)
    }

    /// Counter-clockwise rotation.
    fn rotate(radians: f64) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self::new(cos, sin, -sin, cos, 0.0, 0.0)
    }

    /// Applies `self`, then `outer`.
    fn then(&self, outer: &Affine) -> Affine {
        Affine::new(
            outer.a * self.a + outer.c * self.b,
            outer.b * self.a + outer.d * self.b,
            outer.a * self.c + outer.c * self.d,
            outer.b * self.c + outer.d * self.d,
            outer.a * self.e + outer.c * self.f + outer.e,
            outer.b * self.e + outer.d * self.f + outer.f,
        )
    }

    #[cfg(test)]
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

impl From<Affine> for FS_MATRIX {
    fn from(m: Affine) -> Self {
        FS_MATRIX {
            a: m.a as f32,
            b: m.b as f32,
            c: m.c as f32,
            d: m.d as f32,
            e: m.e as f32,
            f: m.f as f32,
        }
    }
}

/// Maps display space (origin at the bottom-left of the page as shown,
/// after /Rotate) to page space. Returns the transform and the displayed
/// width and height.
fn display_to_page(visible: (f64, f64, f64, f64), quarter_turns: i32) -> (Affine, f64, f64) {
    let (x0, y0, x1, y1) = visible;
    let (w, h) = (x1 - x0, y1 - y0);
    match quarter_turns {
        // Shown turned 90° clockwise: the page's left edge is at the top.
        1 => (Affine::new(0.0, 1.0, -1.0, 0.0, x0 + w, y0), h, w),
        2 => (Affine::new(-1.0, 0.0, 0.0, -1.0, x0 + w, y0 + h), w, h),
        3 => (Affine::new(0.0, -1.0, 1.0, 0.0, x0, y0 + h), h, w),
        _ => (Affine::translate(x0, y0), w, h),
    }
}

/// Transforms mapping the unit square onto each stamp, in display space.
/// `aspect` is the stamp's height / width.
fn placements(page_w: f64, page_h: f64, aspect: f64, opts: &WatermarkOptions) -> Vec<Affine> {
    let stamp_w = opts.size as f64 * page_w;
    let stamp_h = stamp_w * aspect;
    let angle = (opts.rotation as f64).to_radians();
    let (sin, cos) = (angle.sin().abs(), angle.cos().abs());
    // Size of the rotated stamp's bounding box.
    let (box_w, box_h) = (stamp_w * cos + stamp_h * sin, stamp_w * sin + stamp_h * cos);
    let at = |cx: f64, cy: f64| {
        Affine::scale(stamp_w, stamp_h)
            .then(&Affine::translate(-stamp_w / 2.0, -stamp_h / 2.0))
            .then(&Affine::rotate(angle))
            .then(&Affine::translate(cx, cy))
    };

    let margin = opts.margin as f64 * page_w.min(page_h);
    let left = margin + box_w / 2.0;
    let right = page_w - margin - box_w / 2.0;
    let top = page_h - margin - box_h / 2.0;
    let bottom = margin + box_h / 2.0;
    let (mid_x, mid_y) = (page_w / 2.0, page_h / 2.0);

    use Position::*;
    let center = match opts.position {
        TopLeft => (left, top),
        Top => (mid_x, top),
        TopRight => (right, top),
        Left => (left, mid_y),
        Center => (mid_x, mid_y),
        Right => (right, mid_y),
        BottomLeft => (left, bottom),
        Bottom => (mid_x, bottom),
        BottomRight => (right, bottom),
        Tile => {
            return tile(page_w, page_h, box_w, box_h)
                .map(|(x, y)| at(x, y))
                .collect()
        }
    };
    vec![at(center.0, center.1)]
}

/// Stamp centers covering the page in a brick pattern, centred on the page.
fn tile(page_w: f64, page_h: f64, box_w: f64, box_h: f64) -> impl Iterator<Item = (f64, f64)> {
    let step_x = box_w * 1.5;
    let step_y = box_h * 1.8;
    let cols = (page_w / step_x / 2.0).ceil() as i64 + 1;
    let rows = (page_h / step_y / 2.0).ceil() as i64 + 1;
    (-rows..=rows).flat_map(move |row| {
        // Every other row shifts by half a step.
        let shift = if row.rem_euclid(2) == 1 {
            step_x / 2.0
        } else {
            0.0
        };
        (-cols..=cols).filter_map(move |col| {
            let (x, y) = (
                page_w / 2.0 + col as f64 * step_x + shift,
                page_h / 2.0 + row as f64 * step_y,
            );
            // Skip stamps that would be entirely off the page.
            let visible = x + box_w / 2.0 > 0.0
                && x - box_w / 2.0 < page_w
                && y + box_h / 2.0 > 0.0
                && y - box_h / 2.0 < page_h;
            visible.then_some((x, y))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(position: Position, rotation: f32) -> WatermarkOptions {
        WatermarkOptions {
            position,
            rotation,
            size: 0.5,
            margin: 0.0,
            ..WatermarkOptions::new(Stamp {
                width: 2,
                height: 1,
                rgba: vec![0; 8],
            })
        }
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn places_corners_inside_the_page() {
        // 200 x 100 page, stamp 100 x 50.
        let m = placements(200.0, 100.0, 0.5, &opts(Position::TopLeft, 0.0))[0];
        assert!(close(m.apply(0.0, 0.0), (0.0, 50.0)), "bottom-left corner");
        assert!(close(m.apply(1.0, 1.0), (100.0, 100.0)), "top-right corner");

        let m = placements(200.0, 100.0, 0.5, &opts(Position::BottomRight, 0.0))[0];
        assert!(close(m.apply(1.0, 0.0), (200.0, 0.0)));
    }

    #[test]
    fn rotates_counter_clockwise_about_the_center() {
        let m = placements(200.0, 200.0, 0.5, &opts(Position::Center, 90.0))[0];
        // The stamp's right edge midpoint swings to the top.
        assert!(close(m.apply(1.0, 0.5), (100.0, 150.0)));
        assert!(close(m.apply(0.5, 0.5), (100.0, 100.0)));
    }

    #[test]
    fn display_space_follows_page_rotation() {
        // A 200 x 100 page shown turned 90° clockwise is 100 wide, 200 tall.
        let (m, w, h) = display_to_page((0.0, 0.0, 200.0, 100.0), 1);
        assert_eq!((w, h), (100.0, 200.0));
        // The displayed top-left corner is the page's bottom-left corner.
        assert!(close(m.apply(0.0, 200.0), (0.0, 0.0)));
        assert!(close(m.apply(100.0, 200.0), (0.0, 100.0)));

        let (m, _, _) = display_to_page((10.0, 20.0, 210.0, 120.0), 0);
        assert!(close(m.apply(0.0, 0.0), (10.0, 20.0)), "crop box offset");
    }

    #[test]
    fn tiles_cover_the_page() {
        let centers: Vec<_> = tile(600.0, 800.0, 100.0, 40.0).collect();
        assert!(centers.len() > 10);
        assert!(
            centers.iter().any(|&(x, y)| x < 100.0 && y < 100.0),
            "bottom-left area"
        );
        assert!(
            centers.iter().any(|&(x, y)| x > 500.0 && y > 700.0),
            "top-right area"
        );
    }

    #[test]
    fn page_selection() {
        assert_eq!(PageSelection::All.resolve(3).unwrap(), [0, 1, 2]);
        assert_eq!(
            PageSelection::Only("2-3".into()).resolve(4).unwrap(),
            [1, 2]
        );
        assert_eq!(
            PageSelection::Except("1, 4".into()).resolve(4).unwrap(),
            [1, 2]
        );
        assert_eq!(
            PageSelection::Except(" ".into()).resolve(2).unwrap(),
            [0, 1]
        );
        assert!(PageSelection::Only("9".into()).resolve(4).is_err());
    }

    #[test]
    fn decodes_png_with_and_without_alpha() {
        let mut rgb = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut rgb, 1, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .unwrap()
                .write_image_data(&[10, 20, 30])
                .unwrap();
        }
        let stamp = Stamp::from_png(&rgb).unwrap();
        assert_eq!(stamp.rgba, [10, 20, 30, 255]);
        assert!(Stamp::from_png(b"not a png").is_err());
    }
}
