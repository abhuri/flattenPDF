//! End-to-end checks for combining PDFs and pictures into one PDF.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use common::{assert_near, pdfium, pixel, red_blue_pdf, TempDir};
use flatten_core::{
    combine, CombineInput, CombineOptions, FlattenError, FlattenOptions, ImagePageSize, Pdfium,
};
use pdfium_render::prelude::*;

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const WHITE: [u8; 3] = [255, 255, 255];

/// A 40 x 20 JPEG, left half red and right half blue, tagged with EXIF
/// orientation 6 ("rotate 90° clockwise to display"): shown 20 x 40 with red
/// on top.
fn rotated_jpeg() -> Vec<u8> {
    let (w, h) = (40u16, 20u16);
    let rgb: Vec<u8> = (0..h)
        .flat_map(|_| (0..w).flat_map(|x| if x < w / 2 { RED } else { BLUE }))
        .collect();
    // Big-endian TIFF header, one IFD entry: Orientation (0x0112), SHORT, 1, 6.
    let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
    exif.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
    let mut jpeg = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut jpeg, 95);
    encoder.add_app_segment(1, exif).unwrap();
    encoder
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    jpeg
}

/// A 30 x 10 PNG: left half fully transparent, right half green.
fn half_transparent_png() -> Vec<u8> {
    let (w, h) = (30u32, 10u32);
    let rgba: Vec<u8> = (0..h)
        .flat_map(|_| {
            (0..w).flat_map(|x| {
                if x < w / 2 {
                    [0, 0, 0, 0]
                } else {
                    [0, 255, 0, 255]
                }
            })
        })
        .collect();
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
    }
    png_bytes
}

/// Writes the three sample inputs into `dir`: a PDF, a JPEG and a PNG.
fn sample_inputs(dir: &TempDir) -> Vec<CombineInput> {
    let files: [(&str, Vec<u8>); 3] = [
        ("a.pdf", red_blue_pdf("/MediaBox [0 0 100 100]")),
        ("b.jpg", rotated_jpeg()),
        ("c.png", half_transparent_png()),
    ];
    files
        .into_iter()
        .map(|(name, bytes)| {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            CombineInput::from_path(path, None)
        })
        .collect()
}

fn run(
    pdfium: &Pdfium,
    inputs: &[CombineInput],
    output: &Path,
    opts: &CombineOptions,
) -> Result<flatten_core::Summary, FlattenError> {
    combine(
        pdfium,
        inputs,
        output,
        opts,
        &AtomicBool::new(false),
        &|_| {},
    )
}

/// Page sizes in points.
fn page_sizes(pdfium: &Pdfium, path: &Path) -> Vec<(f32, f32)> {
    let doc = pdfium.load_pdf_from_file(path, None).unwrap();
    doc.pages()
        .iter()
        .map(|p| (p.width().value, p.height().value))
        .collect()
}

/// Renders page `index` at `scale` x 72 DPI; returns (RGB pixels, width, height).
fn render(pdfium: &Pdfium, path: &Path, index: usize, scale: f32) -> (Vec<u8>, usize, usize) {
    let doc = pdfium.load_pdf_from_file(path, None).unwrap();
    let page = doc.pages().get(index as _).unwrap();
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .set_format(PdfBitmapFormat::BGRA)
        .set_reverse_byte_order(true)
        .set_clear_color(PdfColor::WHITE);
    let bitmap = page.render_with_config(&config).unwrap();
    let (w, h) = (bitmap.width() as usize, bitmap.height() as usize);
    let rgb = bitmap
        .as_raw_bytes()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    (rgb, w, h)
}

fn close(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5
}

#[test]
fn combines_pdfs_and_pictures_in_order() {
    let Some(pdfium) = pdfium() else { return };
    let dir = TempDir::new("combine-order");
    let inputs = sample_inputs(&dir);
    let output = dir.join("out.pdf");
    let opts = CombineOptions {
        image_page_size: ImagePageSize::Fit,
        flatten: None,
    };
    let summary = run(pdfium, &inputs, &output, &opts).unwrap();
    assert_eq!(summary.pages, 3);

    // 96 px per inch: 20 x 40 px -> 15 x 30 pt, 30 x 10 px -> 22.5 x 7.5 pt.
    let sizes = page_sizes(pdfium, &output);
    assert!(close(sizes[0], (100.0, 100.0)), "{sizes:?}");
    assert!(
        close(sizes[1], (15.0, 30.0)),
        "JPEG turned upright: {sizes:?}"
    );
    assert!(close(sizes[2], (22.5, 7.5)), "{sizes:?}");
    // The JPEG is embedded as is, not re-encoded.
    let bytes = std::fs::read(&output).unwrap();
    common::find(&bytes, b"/DCTDecode", 0);

    // The PDF page is copied unchanged.
    let (px, w, _) = render(pdfium, &output, 0, 1.0);
    assert_near(pixel(&px, w, 3, 25, 50), &RED, "pdf left");
    assert_near(pixel(&px, w, 3, 75, 50), &BLUE, "pdf right");

    // EXIF orientation 6: the left (red) half of the stored JPEG is on top.
    let (px, w, h) = render(pdfium, &output, 1, 8.0);
    assert_near(pixel(&px, w, 3, w / 2, h / 4), &RED, "jpeg top");
    assert_near(pixel(&px, w, 3, w / 2, h * 3 / 4), &BLUE, "jpeg bottom");

    // Transparent pixels show the white page.
    let (px, w, h) = render(pdfium, &output, 2, 8.0);
    assert_near(
        pixel(&px, w, 3, w / 4, h / 2),
        &WHITE,
        "png transparent half",
    );
    assert_near(pixel(&px, w, 3, w * 3 / 4, h / 2), &GREEN, "png green half");
}

#[test]
fn fits_pictures_on_turned_paper() {
    let Some(pdfium) = pdfium() else { return };
    let dir = TempDir::new("combine-a4");
    let inputs = sample_inputs(&dir);
    let output = dir.join("out.pdf");
    let opts = CombineOptions::default(); // A4
    run(pdfium, &inputs[1..], &output, &opts).unwrap();

    let sizes = page_sizes(pdfium, &output);
    assert!(
        close(sizes[0], (595.3, 841.9)),
        "portrait picture: {sizes:?}"
    );
    assert!(
        close(sizes[1], (841.9, 595.3)),
        "landscape picture: {sizes:?}"
    );

    // The 1:2 picture fills the page height and is centred across.
    let (px, w, h) = render(pdfium, &output, 0, 0.5);
    assert_near(pixel(&px, w, 3, w / 2, h / 8), &RED, "top");
    assert_near(pixel(&px, w, 3, w / 2, h * 7 / 8), &BLUE, "bottom");
    assert_near(pixel(&px, w, 3, 4, h / 2), &WHITE, "margin");
}

#[test]
fn flattens_the_combined_document() {
    let Some(pdfium) = pdfium() else { return };
    let dir = TempDir::new("combine-flatten");
    let inputs = sample_inputs(&dir);
    let output = dir.join("out.pdf");
    let opts = CombineOptions {
        image_page_size: ImagePageSize::Fit,
        flatten: Some(FlattenOptions {
            dpi: 150.0,
            ..FlattenOptions::default()
        }),
    };
    let summary = run(pdfium, &inputs, &output, &opts).unwrap();
    assert_eq!(summary.pages, 3);
    assert_eq!(page_sizes(pdfium, &output).len(), 3);

    // Every page is now a single image.
    let doc = pdfium.load_pdf_from_file(&output, None).unwrap();
    for page in doc.pages().iter() {
        let objects: Vec<_> = page.objects().iter().collect();
        assert_eq!(objects.len(), 1);
        assert!(objects[0].as_image_object().is_some());
    }
    drop(doc);

    let (px, w, h) = render(pdfium, &output, 1, 8.0);
    assert_near(
        pixel(&px, w, 3, w / 2, h / 4),
        &RED,
        "jpeg top after flatten",
    );

    // Nothing but the inputs and the output is left behind.
    let mut names: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["a.pdf", "b.jpg", "c.png", "out.pdf"]);
}

#[test]
fn reports_which_file_failed() {
    let Some(pdfium) = pdfium() else { return };
    let dir = TempDir::new("combine-broken");
    let mut inputs = sample_inputs(&dir);
    let broken: PathBuf = dir.join("broken.pdf");
    std::fs::write(&broken, b"not a pdf").unwrap();
    inputs.insert(1, CombineInput::from_path(&broken, None));
    let output = dir.join("out.pdf");

    let err = run(pdfium, &inputs, &output, &CombineOptions::default()).unwrap_err();
    match &err {
        FlattenError::InFile { file, .. } => assert_eq!(file, &broken),
        other => panic!("expected InFile, got {other:?}"),
    }
    assert!(err.to_string().starts_with("broken.pdf: "), "{err}");
    assert!(!output.exists());
    assert!(!dir.join("out.pdf.part.part").exists());

    // The output may not overwrite an input.
    let err = run(
        pdfium,
        &inputs[..1],
        inputs[0].path(),
        &CombineOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, FlattenError::InvalidOptions(_)), "{err:?}");
}
