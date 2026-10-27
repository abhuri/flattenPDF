//! Flatten: every page becomes a JPEG in a new, image-only PDF.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use crate::pdf_writer::{ColorSpace, ImagePage, ImagePdfWriter};
use crate::pipeline::{self, RenderedPage};
use crate::{
    ensure_not_input, part_path, remove_part, validate_dpi, worker_threads, Engine, FlattenError,
    Progress, Summary,
};

#[derive(Debug, Clone)]
pub struct FlattenOptions {
    pub dpi: f32,
    /// JPEG quality, 1-100.
    pub jpeg_quality: u8,
    pub grayscale: bool,
    pub password: Option<String>,
    /// Number of JPEG encoder threads. 0 means "use all cores".
    pub threads: usize,
}

impl Default for FlattenOptions {
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

struct EncodedPage {
    width: u32,
    height: u32,
    jpeg: Vec<u8>,
    width_pt: f64,
    height_pt: f64,
}

/// Flattens `input` into an image-only PDF at `output`.
///
/// `on_progress` may be called from a worker thread.
pub fn flatten(
    engine: &Engine,
    input: &Path,
    output: &Path,
    opts: &FlattenOptions,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<Summary, FlattenError> {
    validate_dpi(opts.dpi)?;
    if !(1..=100).contains(&opts.jpeg_quality) {
        return Err(FlattenError::InvalidOptions(
            "JPEG quality must be between 1 and 100".into(),
        ));
    }
    ensure_not_input(input, output)?;
    let started = Instant::now();

    let document = engine
        .pdfium()
        .load_pdf_from_file(input, opts.password.as_deref())?;
    let total_pages = document.pages().len() as usize;
    on_progress(Progress::Started { total_pages });

    let temp_path = part_path(output);
    let result = (|| -> Result<(), FlattenError> {
        let file = BufWriter::with_capacity(1 << 20, File::create(&temp_path)?);
        let mut pdf = ImagePdfWriter::new(file)?;
        let color_space = || {
            if opts.grayscale {
                ColorSpace::Gray
            } else {
                ColorSpace::Rgb
            }
        };
        let page_indices: Vec<usize> = (0..total_pages).collect();
        pipeline::run(
            &document,
            &page_indices,
            opts.dpi,
            worker_threads(opts.threads),
            cancel,
            &|page| encode_page(page, opts.jpeg_quality, opts.grayscale),
            &mut |page: EncodedPage| {
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
                Ok(())
            },
        )?;
        pdf.finish()?
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            fs::rename(&temp_path, output)?;
            Ok(Summary {
                pages: total_pages,
                output_bytes: fs::metadata(output)?.len(),
                output: output.to_path_buf(),
                elapsed: started.elapsed(),
            })
        }
        Err(err) => {
            remove_part(&temp_path);
            Err(err)
        }
    }
}

fn encode_page(
    page: RenderedPage,
    quality: u8,
    grayscale: bool,
) -> Result<EncodedPage, FlattenError> {
    use jpeg_encoder::{ColorType, Encoder};

    let fail = |e: jpeg_encoder::EncodingError| FlattenError::Encode {
        page: page.page_number,
        message: e.to_string(),
    };
    let mut jpeg = Vec::with_capacity(page.rgba.len() / 10);
    let encoder = Encoder::new(&mut jpeg, quality);
    if grayscale {
        let luma: Vec<u8> = page
            .rgba
            .chunks_exact(4)
            .map(|px| ((px[0] as u32 * 299 + px[1] as u32 * 587 + px[2] as u32 * 114) / 1000) as u8)
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
                &page.rgba,
                page.width as u16,
                page.height as u16,
                ColorType::Rgba,
            )
            .map_err(fail)?;
    }
    Ok(EncodedPage {
        width: page.width,
        height: page.height,
        jpeg,
        width_pt: page.width_pt,
        height_pt: page.height_pt,
    })
}
