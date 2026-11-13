//! Combine: PDFs and pictures, in the order given, into one PDF, optionally
//! flattening the result.
//!
//! PDF pages are copied as they are (text, links and forms keep working
//! unless the result is flattened). Each picture becomes one page. JPEG files
//! are embedded byte for byte, so photos lose no quality and stay small;
//! other formats are decoded and stored losslessly.

use std::ffi::c_int;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use pdfium_render::prelude::*;

use crate::raw::{self, MemoryAccess, RawDocument};
use crate::watermark::Affine;
use crate::{
    ensure_not_input, flatten, part_path, remove_part, FlattenError, FlattenOptions, Progress,
    Summary,
};

/// File extensions (lower case) of the pictures [`combine`] accepts.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff"];

/// A4 and US Letter in points.
const A4: (f64, f64) = (595.276, 841.89);
const LETTER: (f64, f64) = (612.0, 792.0);
/// Pictures without a known resolution are placed at 96 pixels per inch.
const FIT_POINTS_PER_PIXEL: f64 = 72.0 / 96.0;
/// PDF viewers cap pages at 200 inches.
const MAX_PAGE_POINTS: f64 = 14_400.0;

/// True when `path` has one of the [`IMAGE_EXTENSIONS`].
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

/// Reads just enough of a picture to check it can be opened. Returns its
/// stored size in pixels.
pub fn picture_size(path: &Path) -> Result<(u32, u32), FlattenError> {
    ImageReader::open(path)?
        .with_guessed_format()?
        .into_dimensions()
        .map_err(|e| FlattenError::InvalidOptions(format!("picture: {e}")))
}

/// One file to combine.
#[derive(Debug, Clone)]
pub enum CombineInput {
    Pdf {
        path: PathBuf,
        password: Option<String>,
    },
    Image {
        path: PathBuf,
    },
}

impl CombineInput {
    /// A picture if the extension says so, otherwise a PDF.
    pub fn from_path(path: impl Into<PathBuf>, password: Option<String>) -> Self {
        let path = path.into();
        if is_image_path(&path) {
            CombineInput::Image { path }
        } else {
            CombineInput::Pdf { path, password }
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            CombineInput::Pdf { path, .. } | CombineInput::Image { path } => path,
        }
    }
}

/// The page a picture is placed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImagePageSize {
    /// A4, turned to match the picture, with the picture fitted inside.
    #[default]
    A4,
    /// US Letter, turned to match the picture, with the picture fitted inside.
    Letter,
    /// Exactly the picture's size, at 96 pixels per inch.
    Fit,
}

#[derive(Debug, Clone, Default)]
pub struct CombineOptions {
    pub image_page_size: ImagePageSize,
    /// Flatten the combined document afterwards.
    pub flatten: Option<FlattenOptions>,
}

/// Combines `inputs`, in order, into one PDF at `output`.
///
/// Errors about one input are wrapped in [`FlattenError::InFile`]. With
/// `opts.flatten` set, progress follows the flatten step; otherwise it counts
/// inputs. `on_progress` may be called from a worker thread.
pub fn combine(
    pdfium: &Pdfium,
    inputs: &[CombineInput],
    output: &Path,
    opts: &CombineOptions,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<Summary, FlattenError> {
    if inputs.is_empty() {
        return Err(FlattenError::InvalidOptions("no files to combine".into()));
    }
    for input in inputs {
        ensure_not_input(input.path(), output)?;
    }
    let started = Instant::now();
    // Distinct from flatten's own `<output>.part`, which it writes while
    // reading this file.
    let combined_path = part_path(&part_path(output));

    let result = (|| -> Result<Summary, FlattenError> {
        let quiet = |_: Progress| {};
        let step_progress: &(dyn Fn(Progress) + Sync) = if opts.flatten.is_some() {
            &quiet
        } else {
            on_progress
        };
        let doc = RawDocument::new_empty()?;
        step_progress(Progress::Started {
            total_pages: inputs.len(),
        });
        for (done, input) in inputs.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(FlattenError::Cancelled);
            }
            add_input(&doc, input, opts.image_page_size).map_err(|err| FlattenError::InFile {
                file: input.path().to_path_buf(),
                error: Box::new(err),
            })?;
            step_progress(Progress::PageDone {
                done: done + 1,
                total_pages: inputs.len(),
            });
        }
        let pages = doc.page_count();
        doc.save_to_file(&combined_path)?;
        drop(doc);

        let Some(flatten_opts) = &opts.flatten else {
            fs::rename(&combined_path, output)?;
            return Ok(Summary {
                pages,
                output_bytes: fs::metadata(output)?.len(),
                output: output.to_path_buf(),
                elapsed: started.elapsed(),
            });
        };
        // The combined copy is never encrypted.
        let flatten_opts = FlattenOptions {
            password: None,
            ..flatten_opts.clone()
        };
        let mut summary = flatten(
            pdfium,
            &combined_path,
            output,
            &flatten_opts,
            cancel,
            on_progress,
        )?;
        remove_part(&combined_path);
        summary.elapsed = started.elapsed();
        Ok(summary)
    })();

    if result.is_err() {
        remove_part(&combined_path);
    }
    result
}

fn add_input(
    doc: &RawDocument,
    input: &CombineInput,
    page_size: ImagePageSize,
) -> Result<(), FlattenError> {
    match input {
        CombineInput::Pdf { path, password } => {
            let password = password.as_deref().filter(|p| !p.is_empty());
            let source = RawDocument::open(path, password)?;
            doc.import_all(&source)
        }
        CombineInput::Image { path } => add_image_page(doc, &fs::read(path)?, page_size),
    }
}

/// Adds one page showing the picture in `bytes`.
fn add_image_page(
    doc: &RawDocument,
    bytes: &[u8],
    page_size: ImagePageSize,
) -> Result<(), FlattenError> {
    let invalid = |e: image::ImageError| FlattenError::InvalidOptions(format!("picture: {e}"));
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(FlattenError::Io)?;
    let is_jpeg = reader.format() == Some(ImageFormat::Jpeg);
    let mut decoder = reader.into_decoder().map_err(invalid)?;
    let (stored_w, stored_h) = decoder.dimensions();
    if stored_w == 0 || stored_h == 0 {
        return Err(FlattenError::InvalidOptions("picture is empty".into()));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let (shown_w, shown_h) = if swaps_sides(orientation) {
        (stored_h, stored_w)
    } else {
        (stored_w, stored_h)
    };

    let (page_w, page_h, rect) = layout(shown_w as f64, shown_h as f64, page_size);
    let page = doc.new_page(page_w, page_h)?;
    let raw = raw::bindings()?;
    let object = unsafe { raw.FPDFPageObj_NewImageObj(doc.handle) };
    if object.is_null() {
        return Err(FlattenError::PdfiumFailed("create an image"));
    }
    let loaded = if is_jpeg {
        // Embedded as is; the orientation is applied by the placement matrix.
        let mut access = MemoryAccess::new(bytes.to_vec())?;
        drop(decoder);
        let ok = unsafe {
            raw.FPDFImageObj_LoadJpegFileInline(std::ptr::null_mut(), 0, object, &mut access.base)
        };
        raw.is_true(ok)
    } else {
        let mut image = DynamicImage::from_decoder(decoder).map_err(invalid)?;
        image.apply_orientation(orientation);
        set_bitmap(raw, object, &image.into_rgba8())?
    };
    let orientation = if is_jpeg {
        orientation
    } else {
        Orientation::NoTransforms
    };
    if !loaded {
        unsafe { raw.FPDFPageObj_Destroy(object) };
        return Err(FlattenError::PdfiumFailed("load the picture"));
    }
    let (x, y, w, h) = rect;
    let matrix = orientation_matrix(orientation, w, h).then(&Affine::translate(x, y));
    unsafe { raw.FPDFPageObj_SetMatrix(object, &matrix.into()) };
    page.insert(object)?;
    page.generate_content()
}

/// Copies RGBA pixels into a PDFium bitmap and sets it on `object`. Opaque
/// pictures get no alpha channel, so the PDF stores no soft mask.
fn set_bitmap(
    raw: &dyn PdfiumLibraryBindings,
    object: FPDF_PAGEOBJECT,
    rgba: &image::RgbaImage,
) -> Result<bool, FlattenError> {
    let (w, h) = rgba.dimensions();
    let opaque = rgba.pixels().all(|p| p[3] == 255);
    unsafe {
        let bitmap = raw.FPDFBitmap_Create(w as c_int, h as c_int, (!opaque) as c_int);
        if bitmap.is_null() {
            return Err(FlattenError::PdfiumFailed(
                "create a bitmap for the picture",
            ));
        }
        let stride = raw.FPDFBitmap_GetStride(bitmap) as usize;
        let buffer = raw.FPDFBitmap_GetBuffer(bitmap) as *mut u8;
        let row_bytes = w as usize * 4;
        for (y, row) in rgba.as_raw().chunks_exact(row_bytes).enumerate() {
            let out = std::slice::from_raw_parts_mut(buffer.add(y * stride), row_bytes);
            for (src, dst) in row.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
                dst[0] = src[2];
                dst[1] = src[1];
                dst[2] = src[0];
                dst[3] = src[3];
            }
        }
        let ok = raw.FPDFImageObj_SetBitmap(std::ptr::null_mut(), 0, object, bitmap);
        raw.FPDFBitmap_Destroy(bitmap);
        Ok(raw.is_true(ok))
    }
}

/// Page size and where the picture goes on it: (page w, page h, (x, y, w, h)).
fn layout(
    shown_w: f64,
    shown_h: f64,
    page_size: ImagePageSize,
) -> (f64, f64, (f64, f64, f64, f64)) {
    let paper = match page_size {
        ImagePageSize::A4 => A4,
        ImagePageSize::Letter => LETTER,
        ImagePageSize::Fit => {
            let mut scale = FIT_POINTS_PER_PIXEL;
            let longest = shown_w.max(shown_h) * scale;
            if longest > MAX_PAGE_POINTS {
                scale *= MAX_PAGE_POINTS / longest;
            }
            let (w, h) = (shown_w * scale, shown_h * scale);
            return (w, h, (0.0, 0.0, w, h));
        }
    };
    // Turn the paper to match the picture.
    let (page_w, page_h) = if shown_w > shown_h {
        (paper.1, paper.0)
    } else {
        paper
    };
    let scale = (page_w / shown_w).min(page_h / shown_h);
    let (w, h) = (shown_w * scale, shown_h * scale);
    (
        page_w,
        page_h,
        ((page_w - w) / 2.0, (page_h - h) / 2.0, w, h),
    )
}

/// True for the EXIF orientations that show the picture turned on its side.
fn swaps_sides(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// Maps the image unit square (u to the right, v up, as PDF draws images)
/// onto a `w` x `h` box at the origin, showing the stored picture with its
/// EXIF `orientation` applied.
fn orientation_matrix(orientation: Orientation, w: f64, h: f64) -> Affine {
    match orientation {
        Orientation::NoTransforms => Affine::new(w, 0.0, 0.0, h, 0.0, 0.0),
        Orientation::Rotate90 => Affine::new(0.0, -h, w, 0.0, 0.0, h),
        Orientation::Rotate180 => Affine::new(-w, 0.0, 0.0, -h, w, h),
        Orientation::Rotate270 => Affine::new(0.0, h, -w, 0.0, w, 0.0),
        Orientation::FlipHorizontal => Affine::new(-w, 0.0, 0.0, h, w, 0.0),
        Orientation::FlipVertical => Affine::new(w, 0.0, 0.0, -h, 0.0, h),
        Orientation::Rotate90FlipH => Affine::new(0.0, -h, -w, 0.0, w, h),
        Orientation::Rotate270FlipH => Affine::new(0.0, h, w, 0.0, 0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Orientation; 8] = [
        Orientation::NoTransforms,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
        Orientation::FlipHorizontal,
        Orientation::FlipVertical,
        Orientation::Rotate90FlipH,
        Orientation::Rotate270FlipH,
    ];

    /// Every stored pixel must land where the `image` crate puts it when it
    /// applies the same orientation.
    #[test]
    fn orientation_matrix_matches_the_image_crate() {
        let (w, h) = (3u32, 2u32);
        let stored =
            image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        for orientation in ALL {
            let mut shown = DynamicImage::ImageRgba8(stored.clone());
            shown.apply_orientation(orientation);
            let shown = shown.into_rgba8();
            let (sw, sh) = shown.dimensions();
            assert_eq!(swaps_sides(orientation), sw != w, "{orientation:?}");

            let m = orientation_matrix(orientation, sw as f64, sh as f64);
            for (x, y, px) in stored.enumerate_pixels() {
                // Pixel centre in the unit square; row 0 is the top (v = 1).
                let u = (x as f64 + 0.5) / w as f64;
                let v = 1.0 - (y as f64 + 0.5) / h as f64;
                let (dx, dy_up) = m.apply(u, v);
                let (col, row) = (dx.floor() as u32, (sh as f64 - dy_up).floor() as u32);
                assert_eq!(
                    shown.get_pixel(col, row),
                    px,
                    "{orientation:?} pixel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn layout_turns_and_centres_on_paper() {
        // Landscape picture on A4: the page turns, the picture fills its width.
        let (pw, ph, (x, y, w, h)) = layout(400.0, 200.0, ImagePageSize::A4);
        assert_eq!((pw, ph), (A4.1, A4.0));
        assert!((w - A4.1).abs() < 1e-9 && (h - A4.1 / 2.0).abs() < 1e-9);
        assert!(x.abs() < 1e-9 && (y - (A4.0 - h) / 2.0).abs() < 1e-9);

        let (pw, ph, rect) = layout(960.0, 480.0, ImagePageSize::Fit);
        assert_eq!((pw, ph), (720.0, 360.0));
        assert_eq!(rect, (0.0, 0.0, 720.0, 360.0));

        let (pw, _, _) = layout(100_000.0, 10.0, ImagePageSize::Fit);
        assert!((pw - MAX_PAGE_POINTS).abs() < 1e-6);
    }

    #[test]
    fn recognises_pictures_by_extension() {
        assert!(is_image_path(Path::new("a/photo.JPG")));
        assert!(is_image_path(Path::new("scan.tiff")));
        assert!(!is_image_path(Path::new("doc.pdf")));
        assert!(!is_image_path(Path::new("noext")));
        assert!(matches!(
            CombineInput::from_path("x.png", Some("pw".into())),
            CombineInput::Image { .. }
        ));
    }
}
