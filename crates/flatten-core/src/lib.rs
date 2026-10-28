//! FlattenPDF core.
//!
//! - [`flatten`]: render every page to an image and write an image-only PDF
//! - [`to_images`]: export pages as PNG files into a folder or a ZIP archive
//! - [`watermark`]: stamp an image or rasterized text onto selected pages
//!
//! Rendering runs through one bounded pipeline (see `pipeline.rs`), so memory
//! stays flat no matter how many pages a document has.

mod flatten;
mod images;
mod pdf_writer;
mod pipeline;
mod raw;
mod watermark;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pdfium_render::prelude::*;

pub use flatten::{flatten, FlattenOptions};
pub use images::{parse_page_ranges, to_images, ImageOptions, Packaging, Rotation};
pub use pdf_writer::{ColorSpace, ImagePage, ImagePdfWriter};
pub use pdfium_render::prelude::Pdfium;
pub use watermark::{
    watermark, watermark_preview, PageSelection, Position, Stamp, WatermarkOptions,
};

#[derive(Debug, Clone, Copy)]
pub enum Progress {
    Started { total_pages: usize },
    PageDone { done: usize, total_pages: usize },
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub pages: usize,
    /// Size of the output file, or total size of all files written.
    pub output_bytes: u64,
    /// Where the result went (file, folder or archive).
    pub output: PathBuf,
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
    #[error("PDFium could not {0}")]
    PdfiumFailed(&'static str),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding failed on page {page}: {message}")]
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

    // A second set of bindings feeds `raw.rs`; both must be bound before
    // `Pdfium::new` publishes the first set.
    let mut errors = Vec::new();
    for dir in &candidates {
        let lib = Pdfium::pdfium_platform_library_name_at_path(dir);
        if !lib.exists() {
            continue;
        }
        match Pdfium::bind_to_library(&lib).and_then(|a| Ok((a, Pdfium::bind_to_library(&lib)?))) {
            Ok((bindings, raw_bindings)) => {
                raw::install(raw_bindings);
                return Ok(Pdfium::new(bindings));
            }
            Err(err) => errors.push(format!("{}: {err:?}", lib.display())),
        }
    }
    match Pdfium::bind_to_system_library().and_then(|a| Ok((a, Pdfium::bind_to_system_library()?)))
    {
        Ok((bindings, raw_bindings)) => {
            raw::install(raw_bindings);
            Ok(Pdfium::new(bindings))
        }
        Err(err) => {
            errors.push(format!("system library: {err:?}"));
            Err(FlattenError::PdfiumNotFound(errors.join("; ")))
        }
    }
}

fn worker_threads(requested: usize) -> usize {
    match requested {
        0 => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        n => n,
    }
}

fn ensure_not_input(input: &Path, output: &Path) -> Result<(), FlattenError> {
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

fn validate_dpi(dpi: f32) -> Result<(), FlattenError> {
    if !(36.0..=600.0).contains(&dpi) {
        return Err(FlattenError::InvalidOptions(
            "DPI must be between 36 and 600".into(),
        ));
    }
    Ok(())
}

/// `out.pdf` -> `out.pdf.part`: results are written there and renamed only on
/// success, so a failure or cancellation never leaves a half-written output.
fn part_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    output.with_file_name(name)
}

fn remove_part(path: &Path) {
    let _ = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
}
