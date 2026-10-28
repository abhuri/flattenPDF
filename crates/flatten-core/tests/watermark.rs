//! End-to-end checks for watermarks: stamp blank pages with a solid blue
//! square, render the result and look at where the blue ended up.

mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use common::{assert_near, blank_pdf, pdfium, pixel, TempDir};
use flatten_core::{
    watermark, watermark_preview, FlattenOptions, PageSelection, Pdfium, Position, Stamp,
    WatermarkOptions,
};
use pdfium_render::prelude::*;

const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

/// A solid blue 8 x 8 stamp.
fn blue_stamp() -> Stamp {
    Stamp {
        width: 8,
        height: 8,
        rgba: [0, 0, 255, 255].repeat(64),
    }
}

fn options(position: Position) -> WatermarkOptions {
    WatermarkOptions {
        position,
        size: 0.25,
        opacity: 1.0,
        margin: 0.0,
        ..WatermarkOptions::new(blue_stamp())
    }
}

/// Renders page `index` at 72 DPI; returns (RGB pixels, width, height).
fn render(pdfium: &Pdfium, path: &Path, index: usize) -> (Vec<u8>, usize, usize) {
    let doc = pdfium.load_pdf_from_file(path, None).unwrap();
    let page = doc.pages().get(index as _).unwrap();
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(1.0)
        .set_format(PdfBitmapFormat::BGRA)
        .set_reverse_byte_order(true);
    let bitmap = page.render_with_config(&config).unwrap();
    let (w, h) = (bitmap.width() as usize, bitmap.height() as usize);
    let rgb = bitmap
        .as_raw_bytes()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    (rgb, w, h)
}

/// Stamps a blank document and returns the output path (inside `dir`).
fn stamp(
    dir: &TempDir,
    page_attrs: &str,
    pages: usize,
    opts: &WatermarkOptions,
) -> Option<std::path::PathBuf> {
    let pdfium = pdfium()?;
    let input = dir.join("in.pdf");
    let output = dir.join("out.pdf");
    std::fs::write(&input, blank_pdf(page_attrs, pages)).unwrap();
    watermark(
        pdfium,
        &input,
        &output,
        opts,
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    Some(output)
}

#[test]
fn stamps_the_requested_corner() {
    let dir = TempDir::new("wm-corner");
    let Some(out) = stamp(
        &dir,
        "/MediaBox [0 0 200 100]",
        1,
        &options(Position::TopLeft),
    ) else {
        return;
    };
    let (px, w, _) = render(pdfium().unwrap(), &out, 0);
    // Stamp is 50 x 50 pt in the top-left corner.
    assert_near(pixel(&px, w, 3, 10, 10), &BLUE, "top-left is stamped");
    assert_near(pixel(&px, w, 3, 40, 40), &BLUE, "inside the stamp");
    assert_near(pixel(&px, w, 3, 190, 90), &WHITE, "bottom-right is clear");
    assert_near(
        pixel(&px, w, 3, 100, 10),
        &WHITE,
        "right of the stamp is clear",
    );
}

#[test]
fn applies_opacity() {
    let dir = TempDir::new("wm-opacity");
    let opts = WatermarkOptions {
        opacity: 0.5,
        ..options(Position::Center)
    };
    let Some(out) = stamp(&dir, "/MediaBox [0 0 200 200]", 1, &opts) else {
        return;
    };
    let (px, w, _) = render(pdfium().unwrap(), &out, 0);
    // Half-transparent blue over white.
    assert_near(pixel(&px, w, 3, 100, 100), &[128, 128, 255], "blended");
}

#[test]
fn follows_page_rotation() {
    // Landscape media box shown turned 90° clockwise: displayed 100 x 200.
    let dir = TempDir::new("wm-rotate");
    let Some(out) = stamp(
        &dir,
        "/MediaBox [0 0 200 100] /Rotate 90",
        1,
        &options(Position::TopLeft),
    ) else {
        return;
    };
    let (px, w, h) = render(pdfium().unwrap(), &out, 0);
    assert_eq!((w, h), (100, 200));
    // 25% of the displayed width: a 25 x 25 pt stamp at the displayed top-left.
    assert_near(
        pixel(&px, w, 3, 5, 5),
        &BLUE,
        "displayed top-left is stamped",
    );
    assert_near(
        pixel(&px, w, 3, 90, 190),
        &WHITE,
        "displayed bottom-right is clear",
    );
}

#[test]
fn stamps_only_selected_pages_and_stores_the_image_once() {
    let dir = TempDir::new("wm-pages");
    let opts = WatermarkOptions {
        pages: PageSelection::Except("2".into()),
        position: Position::Tile,
        ..options(Position::Tile)
    };
    let Some(out) = stamp(&dir, "/MediaBox [0 0 200 200]", 3, &opts) else {
        return;
    };
    let pdfium = pdfium().unwrap();
    let blue_pixels = |index| {
        let (px, _, _) = render(pdfium, &out, index);
        px.chunks_exact(3)
            .filter(|p| p[2] > 200 && p[0] < 60)
            .count()
    };
    assert!(blue_pixels(0) > 1000, "page 1 is stamped");
    assert_eq!(blue_pixels(1), 0, "page 2 is skipped");
    assert!(blue_pixels(2) > 1000, "page 3 is stamped");

    // Many tiles on two pages, but the stamp is stored once: one image plus
    // its alpha mask (/SMask is an image too).
    let bytes = std::fs::read(&out).unwrap();
    let count = |needle: &[u8]| bytes.windows(needle.len()).filter(|w| *w == needle).count();
    assert!(
        count(b"/Image") <= 2,
        "image stored once, found {}",
        count(b"/Image")
    );
}

#[test]
fn flattens_after_stamping() {
    let dir = TempDir::new("wm-flatten");
    let opts = WatermarkOptions {
        flatten: Some(FlattenOptions {
            dpi: 72.0,
            jpeg_quality: 95,
            ..FlattenOptions::default()
        }),
        ..options(Position::TopLeft)
    };
    let Some(out) = stamp(&dir, "/MediaBox [0 0 200 100]", 2, &opts) else {
        return;
    };
    let bytes = std::fs::read(&out).unwrap();
    assert!(
        bytes.windows(10).any(|w| w == b"/DCTDecode"),
        "pages are JPEG images"
    );
    assert!(
        !bytes.windows(5).any(|w| w == b"/Form"),
        "no separate watermark object left"
    );
    let (px, w, _) = render(pdfium().unwrap(), &out, 1);
    assert_near(
        pixel(&px, w, 3, 10, 10),
        &BLUE,
        "watermark baked into the page",
    );
    assert!(
        !dir.join("out.pdf.part").exists(),
        "flatten temp file removed"
    );
    assert!(
        !dir.join("out.pdf.part.part").exists(),
        "stamped copy removed"
    );
}

#[test]
fn preview_renders_a_stamped_page() {
    let Some(pdfium) = pdfium() else { return };
    let dir = TempDir::new("wm-preview");
    let input = dir.join("in.pdf");
    std::fs::write(&input, blank_pdf("/MediaBox [0 0 200 100]", 2)).unwrap();

    let png = watermark_preview(pdfium, &input, 0, &options(Position::TopLeft), 400).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (400, 200));
    assert_near(pixel(&buf, 400, 3, 20, 20), &BLUE, "stamp in the preview");

    // The source is untouched.
    assert!(!std::fs::read(&input)
        .unwrap()
        .windows(6)
        .any(|w| w == b"/Image"));
}
