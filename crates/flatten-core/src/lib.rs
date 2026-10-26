//! FlattenPDF core: renders every page of a PDF to an image and writes a new
//! PDF that contains only those images.
//!
//! Pipeline (memory stays bounded no matter how many pages):
//!
//! ```text
//! [render thread: PDFium] --bitmaps--> [N encoder threads: JPEG] --jpegs--> [writer thread: in-order PDF]
//! ```
//!
//! PDFium is not thread-safe, so rendering happens on the calling thread only;
//! JPEG encoding, the other expensive step, runs in parallel.

mod pdf_writer;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::bounded;
use pdfium_render::prelude::*;

pub use pdf_writer::{ColorSpace, ImagePage, ImagePdfWriter};
pub use pdfium_render::prelude::Pdfium;

/// Largest bitmap edge we will render. Keeps huge drawings (A0 at high DPI)
/// within JPEG limits and a sane memory budget.
const MAX_PIXEL_DIMENSION: f32 = 14_000.0;

#[derive(Debug, Clone)]
pub struct Options {
    pub dpi: f32,
    /// JPEG quality, 1-100.
    pub jpeg_quality: u8,
    pub grayscale: bool,
    pub password: Option<String>,
    /// Number of JPEG encoder threads. 0 means "use all cores".
    pub threads: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dpi: 200.0,
            jpeg_quality: 85,
            grayscale: false,
            password: None,
            threads: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Progress {
    Started { total_pages: usize },
    PageDone { done: usize, total_pages: usize },
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub pages: usize,
    pub output_bytes: u64,
    pub elapsed: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum FlattenError {
    #[error("could not load the PDFium library: {0}")]
    PdfiumNotFound(String),
    #[error("the PDF is password protected; supply the correct password")]
    PasswordRequired,
    #[error("PDFium error: {0:?}")]
    Pdfium(PdfiumError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JPEG encoding failed on page {page}: {message}")]
    Encode { page: usize, message: String },
    #[error("invalid options: {0}")]
    InvalidOptions(String),
    #[error("cancelled")]
    Cancelled,
}

impl From<PdfiumError> for FlattenError {
    fn from(err: PdfiumError) -> Self {
        match err {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
                FlattenError::PasswordRequired
            }
            other => FlattenError::Pdfium(other),
        }
    }
}

/// Binds to the PDFium shared library. Looks next to the executable first,
/// then in `extra_dir`, then the current directory, then the system paths.
pub fn load_pdfium(extra_dir: Option<&Path>) -> Result<Pdfium, FlattenError> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        candidates.push(dir);
    }
    if let Some(dir) = extra_dir {
        candidates.push(dir.to_path_buf());
    }
    candidates.push(PathBuf::from("."));

    let mut errors = Vec::new();
    for dir in &candidates {
        let lib = Pdfium::pdfium_platform_library_name_at_path(dir);
        if !lib.exists() {
            continue;
        }
        match Pdfium::bind_to_library(&lib) {
            Ok(bindings) => return Ok(Pdfium::new(bindings)),
            Err(err) => errors.push(format!("{}: {err:?}", lib.display())),
        }
    }
    match Pdfium::bind_to_system_library() {
        Ok(bindings) => Ok(Pdfium::new(bindings)),
        Err(err) => {
            errors.push(format!("system library: {err:?}"));
            Err(FlattenError::PdfiumNotFound(errors.join("; ")))
        }
    }
}

struct RenderedPage {
    index: usize,
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    width_pt: f64,
    height_pt: f64,
}

struct EncodedPage {
    index: usize,
    width: u32,
    height: u32,
    jpeg: Vec<u8>,
    width_pt: f64,
    height_pt: f64,
}

/// Flattens `input` into an image-only PDF at `output`.
///
/// The result is written to a temporary `*.part` file next to `output` and
/// renamed only on success, so a failure or cancellation never leaves a
/// half-written PDF behind. `on_progress` may be called from a worker thread.
pub fn flatten(
    pdfium: &Pdfium,
    input: &Path,
    output: &Path,
    opts: &Options,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<Summary, FlattenError> {
    validate(input, output, opts)?;
    let started = Instant::now();

    let document = pdfium.load_pdf_from_file(input, opts.password.as_deref())?;
    let pages = document.pages();
    let total_pages = pages.len() as usize;
    on_progress(Progress::Started { total_pages });

    let threads = match opts.threads {
        0 => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        n => n,
    };
    let temp_path = part_path(output);

    let result = std::thread::scope(|scope| -> Result<(), FlattenError> {
        // Bounded queues are what cap memory: at most ~2 bitmaps per thread in flight.
        let (render_tx, render_rx) = bounded::<RenderedPage>(threads * 2);
        let (encoded_tx, encoded_rx) = bounded::<Result<EncodedPage, FlattenError>>(threads * 2);

        for _ in 0..threads {
            let render_rx = render_rx.clone();
            let encoded_tx = encoded_tx.clone();
            let quality = opts.jpeg_quality;
            let grayscale = opts.grayscale;
            scope.spawn(move || {
                for page in render_rx {
                    if encoded_tx
                        .send(encode_page(page, quality, grayscale))
                        .is_err()
                    {
                        break; // writer gave up
                    }
                }
            });
        }
        drop(render_rx);
        drop(encoded_tx);

        let temp_path = &temp_path;
        let writer = scope.spawn(move || -> Result<(), FlattenError> {
            let file = BufWriter::with_capacity(1 << 20, File::create(temp_path)?);
            let mut pdf = ImagePdfWriter::new(file)?;
            let color_space = || {
                if opts.grayscale {
                    ColorSpace::Gray
                } else {
                    ColorSpace::Rgb
                }
            };
            // Encoders finish out of order; hold early pages until their turn.
            let mut pending: BTreeMap<usize, EncodedPage> = BTreeMap::new();
            for encoded in encoded_rx {
                let encoded = encoded?;
                pending.insert(encoded.index, encoded);
                while let Some(page) = pending.remove(&pdf.page_count()) {
                    pdf.add_page(&ImagePage {
                        jpeg: &page.jpeg,
                        pixel_width: page.width,
                        pixel_height: page.height,
                        color_space: color_space(),
                        width_pt: page.width_pt,
                        height_pt: page.height_pt,
                    })?;
                    on_progress(Progress::PageDone {
                        done: pdf.page_count(),
                        total_pages,
                    });
                }
            }
            if pdf.page_count() != total_pages {
                return Err(FlattenError::Cancelled);
            }
            pdf.finish()?
                .into_inner()
                .map_err(|e| e.into_error())?
                .sync_all()?;
            Ok(())
        });

        let render_result = (|| -> Result<(), FlattenError> {
            for index in 0..total_pages {
                if cancel.load(Ordering::Relaxed) {
                    return Err(FlattenError::Cancelled);
                }
                let page = pages.get(index as _)?;
                let rendered = render_page(&page, index, opts.dpi)?;
                if render_tx.send(rendered).is_err() {
                    break; // writer failed; its error is reported below
                }
            }
            Ok(())
        })();
        drop(render_tx);

        let writer_result = writer.join().expect("writer thread panicked");
        render_result.and(writer_result)
    });

    match result {
        Ok(()) => {
            fs::rename(&temp_path, output)?;
            Ok(Summary {
                pages: total_pages,
                output_bytes: fs::metadata(output)?.len(),
                elapsed: started.elapsed(),
            })
        }
        Err(err) => {
            let _ = fs::remove_file(&temp_path);
            Err(err)
        }
    }
}

fn validate(input: &Path, output: &Path, opts: &Options) -> Result<(), FlattenError> {
    if !(36.0..=600.0).contains(&opts.dpi) {
        return Err(FlattenError::InvalidOptions(
            "DPI must be between 36 and 600".into(),
        ));
    }
    if !(1..=100).contains(&opts.jpeg_quality) {
        return Err(FlattenError::InvalidOptions(
            "JPEG quality must be between 1 and 100".into(),
        ));
    }
    let same_file = match (input.canonicalize(), output.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same_file {
        return Err(FlattenError::InvalidOptions(
            "output must not overwrite the input file".into(),
        ));
    }
    Ok(())
}

fn part_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    output.with_file_name(name)
}

fn render_page(page: &PdfPage, index: usize, dpi: f32) -> Result<RenderedPage, FlattenError> {
    let mut scale = dpi / 72.0;
    let longest_pt = page.width().value.max(page.height().value);
    if longest_pt * scale > MAX_PIXEL_DIMENSION {
        scale = MAX_PIXEL_DIMENSION / longest_pt;
    }
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .set_format(PdfBitmapFormat::BGRA)
        .set_clear_color(PdfColor::WHITE)
        .render_form_data(true)
        .render_annotations(true);
    let bitmap = page.render_with_config(&config)?;
    let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
    // Derive the page size from the bitmap so page rotation is already applied.
    Ok(RenderedPage {
        index,
        width,
        height,
        bgra: bitmap.as_raw_bytes(),
        width_pt: width as f64 * 72.0 / scale as f64,
        height_pt: height as f64 * 72.0 / scale as f64,
    })
}

fn encode_page(
    page: RenderedPage,
    quality: u8,
    grayscale: bool,
) -> Result<EncodedPage, FlattenError> {
    use jpeg_encoder::{ColorType, Encoder};

    let fail = |e: jpeg_encoder::EncodingError| FlattenError::Encode {
        page: page.index + 1,
        message: e.to_string(),
    };
    let mut jpeg = Vec::with_capacity(page.bgra.len() / 10);
    let encoder = Encoder::new(&mut jpeg, quality);
    if grayscale {
        let luma: Vec<u8> = page
            .bgra
            .chunks_exact(4)
            .map(|px| ((px[2] as u32 * 299 + px[1] as u32 * 587 + px[0] as u32 * 114) / 1000) as u8)
            .collect();
        encoder
            .encode(
                &luma,
                page.width as u16,
                page.height as u16,
                ColorType::Luma,
            )
            .map_err(fail)?;
    } else {
        encoder
            .encode(
                &page.bgra,
                page.width as u16,
                page.height as u16,
                ColorType::Bgra,
            )
            .map_err(fail)?;
    }
    Ok(EncodedPage {
        index: page.index,
        width: page.width,
        height: page.height,
        jpeg,
        width_pt: page.width_pt,
        height_pt: page.height_pt,
    })
}
