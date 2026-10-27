//! To images: export pages as PNG files, into a folder or a ZIP archive.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use pdfium_render::prelude::Pdfium;

use crate::pipeline::{self, RenderedPage};
use crate::{
    part_path, remove_part, validate_dpi, worker_threads, FlattenError, Progress, Summary,
};

/// Clockwise rotation applied to every exported image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    None,
    /// 90 degrees clockwise.
    Right,
    /// 90 degrees counter-clockwise.
    Left,
    Half,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Packaging {
    /// A folder with one PNG per page.
    #[default]
    Folder,
    /// A single .zip archive with one PNG per page.
    Zip,
}

#[derive(Debug, Clone)]
pub struct ImageOptions {
    pub dpi: f32,
    pub rotation: Rotation,
    pub packaging: Packaging,
    /// Pages to export, e.g. "1-3, 7, 10-". Empty means all pages.
    pub pages: String,
    pub password: Option<String>,
    /// Number of PNG encoder threads. 0 means "use all cores".
    pub threads: usize,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            dpi: 200.0,
            rotation: Rotation::None,
            packaging: Packaging::Folder,
            pages: String::new(),
            password: None,
            threads: 0,
        }
    }
}

/// Exports pages of `input` as PNG images to `output`, which is the folder to
/// create or the .zip file to write, depending on `opts.packaging`.
///
/// `on_progress` may be called from a worker thread.
pub fn to_images(
    pdfium: &Pdfium,
    input: &Path,
    output: &Path,
    opts: &ImageOptions,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<Summary, FlattenError> {
    validate_dpi(opts.dpi)?;
    if output.exists() {
        return Err(FlattenError::InvalidOptions(format!(
            "{} already exists",
            output.display()
        )));
    }
    let started = Instant::now();

    let document = pdfium.load_pdf_from_file(input, opts.password.as_deref())?;
    let page_count = document.pages().len() as usize;
    let page_indices = parse_page_ranges(&opts.pages, page_count)?;
    let total_pages = page_indices.len();
    on_progress(Progress::Started { total_pages });

    // page-001.png ... wide enough for the last page number, at least 3 digits.
    let digits = page_count.to_string().len().max(3);
    let file_name = |page_number: usize| format!("page-{page_number:0digits$}.png");

    let temp_path = part_path(output);
    let mut sink = Sink::create(&temp_path, opts.packaging)?;
    let mut done = 0;
    let rendered = pipeline::run(
        &document,
        &page_indices,
        opts.dpi,
        worker_threads(opts.threads),
        cancel,
        &|page| encode_png(page, opts.rotation),
        &mut |(page_number, png): (usize, Vec<u8>)| {
            sink.add(&file_name(page_number), &png)?;
            done += 1;
            on_progress(Progress::PageDone { done, total_pages });
            Ok(())
        },
    );
    let result = rendered.and_then(|()| sink.finish());

    match result {
        Ok(output_bytes) => {
            fs::rename(&temp_path, output)?;
            Ok(Summary {
                pages: total_pages,
                output_bytes,
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

/// Parses "1-3, 7, 10-" into 0-based page indices, in the order given.
/// An empty spec (or "all") selects every page.
pub fn parse_page_ranges(spec: &str, page_count: usize) -> Result<Vec<usize>, FlattenError> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
        return Ok((0..page_count).collect());
    }
    let invalid = |part: &str| {
        FlattenError::InvalidOptions(format!(
            "invalid page range \"{part}\" (the document has {page_count} pages)"
        ))
    };
    let mut pages = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let number = |s: &str| -> Result<usize, FlattenError> {
            match s.trim().parse::<usize>() {
                Ok(n) if (1..=page_count).contains(&n) => Ok(n),
                _ => Err(invalid(part)),
            }
        };
        let (first, last) = match part.split_once('-') {
            Some((a, b)) => (
                if a.trim().is_empty() { 1 } else { number(a)? },
                if b.trim().is_empty() {
                    page_count
                } else {
                    number(b)?
                },
            ),
            None => (number(part)?, number(part)?),
        };
        if first > last {
            return Err(invalid(part));
        }
        pages.extend(first - 1..last);
    }
    if pages.is_empty() {
        return Err(invalid(spec));
    }
    Ok(pages)
}

fn encode_png(page: RenderedPage, rotation: Rotation) -> Result<(usize, Vec<u8>), FlattenError> {
    let (rgb, width, height) = rotate_to_rgb(&page.rgba, page.width, page.height, rotation);
    let fail = |e: png::EncodingError| FlattenError::Encode {
        page: page.page_number,
        message: e.to_string(),
    };
    let mut png_bytes = Vec::with_capacity(rgb.len() / 8);
    let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    // Record the resolution so the image prints at the page's real size.
    let pixels_per_meter = (page.width as f64 / page.width_pt * 72.0 / 0.0254).round() as u32;
    encoder.set_pixel_dims(Some(png::PixelDimensions {
        xppu: pixels_per_meter,
        yppu: pixels_per_meter,
        unit: png::Unit::Meter,
    }));
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(&rgb).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok((page.page_number, png_bytes))
}

/// Drops the alpha channel and rotates clockwise by `rotation`.
fn rotate_to_rgb(rgba: &[u8], width: u32, height: u32, rotation: Rotation) -> (Vec<u8>, u32, u32) {
    let (w, h) = (width as usize, height as usize);
    let mut rgb = vec![0u8; w * h * 3];
    let (out_w, out_h) = match rotation {
        Rotation::Left | Rotation::Right => (h, w),
        Rotation::None | Rotation::Half => (w, h),
    };
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match rotation {
                Rotation::None => (x, y),
                Rotation::Right => (h - 1 - y, x),
                Rotation::Left => (y, w - 1 - x),
                Rotation::Half => (w - 1 - x, h - 1 - y),
            };
            let src = (y * w + x) * 4;
            let dst = (ny * out_w + nx) * 3;
            rgb[dst..dst + 3].copy_from_slice(&rgba[src..src + 3]);
        }
    }
    (rgb, out_w as u32, out_h as u32)
}

/// Where the PNGs go while exporting.
enum Sink {
    Folder { dir: std::path::PathBuf, bytes: u64 },
    Zip(Box<zip::ZipWriter<BufWriter<File>>>),
}

impl Sink {
    fn create(path: &Path, packaging: Packaging) -> Result<Self, FlattenError> {
        remove_part(path);
        Ok(match packaging {
            Packaging::Folder => {
                fs::create_dir_all(path)?;
                Sink::Folder {
                    dir: path.to_path_buf(),
                    bytes: 0,
                }
            }
            Packaging::Zip => Sink::Zip(Box::new(zip::ZipWriter::new(BufWriter::with_capacity(
                1 << 20,
                File::create(path)?,
            )))),
        })
    }

    fn add(&mut self, name: &str, data: &[u8]) -> Result<(), FlattenError> {
        match self {
            Sink::Folder { dir, bytes } => {
                fs::write(dir.join(name), data)?;
                *bytes += data.len() as u64;
            }
            Sink::Zip(zip) => {
                // PNG is already compressed; storing avoids burning CPU for nothing.
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .large_file(data.len() as u64 >= u32::MAX as u64);
                zip.start_file(name, options).map_err(zip_error)?;
                zip.write_all(data)?;
            }
        }
        Ok(())
    }

    /// Returns the total bytes written.
    fn finish(self) -> Result<u64, FlattenError> {
        match self {
            Sink::Folder { bytes, .. } => Ok(bytes),
            Sink::Zip(zip) => {
                let file = zip
                    .finish()
                    .map_err(zip_error)?
                    .into_inner()
                    .map_err(|e| e.into_error())?;
                file.sync_all()?;
                Ok(file.metadata()?.len())
            }
        }
    }
}

fn zip_error(err: zip::result::ZipError) -> FlattenError {
    match err {
        zip::result::ZipError::Io(io) => FlattenError::Io(io),
        other => FlattenError::Encode {
            page: 0,
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_ranges() {
        assert_eq!(parse_page_ranges("", 4).unwrap(), vec![0, 1, 2, 3]);
        assert_eq!(parse_page_ranges("all", 3).unwrap(), vec![0, 1, 2]);
        assert_eq!(parse_page_ranges("1-2, 4", 5).unwrap(), vec![0, 1, 3]);
        assert_eq!(parse_page_ranges("3-", 5).unwrap(), vec![2, 3, 4]);
        assert_eq!(parse_page_ranges("-2", 5).unwrap(), vec![0, 1]);
        assert!(parse_page_ranges("0", 5).is_err());
        assert!(parse_page_ranges("6", 5).is_err());
        assert!(parse_page_ranges("4-2", 5).is_err());
        assert!(parse_page_ranges("abc", 5).is_err());
    }

    /// 2x1 image: red pixel on the left, blue on the right.
    const RED_BLUE: [u8; 8] = [255, 0, 0, 255, 0, 0, 255, 255];

    #[test]
    fn rotates_clockwise_and_counter_clockwise() {
        let (rgb, w, h) = rotate_to_rgb(&RED_BLUE, 2, 1, Rotation::Right);
        assert_eq!((w, h), (1, 2));
        assert_eq!(rgb, [255, 0, 0, 0, 0, 255], "right: red on top");

        let (rgb, w, h) = rotate_to_rgb(&RED_BLUE, 2, 1, Rotation::Left);
        assert_eq!((w, h), (1, 2));
        assert_eq!(rgb, [0, 0, 255, 255, 0, 0], "left: blue on top");

        let (rgb, w, h) = rotate_to_rgb(&RED_BLUE, 2, 1, Rotation::Half);
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb, [0, 0, 255, 255, 0, 0], "half: mirrored both ways");
    }
}
