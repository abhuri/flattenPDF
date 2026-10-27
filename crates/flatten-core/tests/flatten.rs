//! End-to-end checks for flatten: known colors and page sizes in, then inspect
//! the output PDF and the JPEG inside it.

mod common;

use std::sync::atomic::AtomicBool;

use common::{assert_near, find, pdfium, pixel, red_blue_pdf, TempDir};
use flatten_core::{flatten, FlattenOptions};

/// Flattens the red/blue page and returns the bytes of the output PDF.
fn flatten_test_pdf(name: &str, page_attrs: &str, opts: &FlattenOptions) -> Option<Vec<u8>> {
    let pdfium = pdfium()?;
    let dir = TempDir::new(name);
    let input = dir.join("in.pdf");
    let output = dir.join("out.pdf");
    std::fs::write(&input, red_blue_pdf(page_attrs)).unwrap();

    flatten(
        pdfium,
        &input,
        &output,
        opts,
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    Some(std::fs::read(&output).unwrap())
}

/// Flattens the red/blue page and returns (pixels, width, components) of its JPEG.
fn flatten_and_decode(name: &str, grayscale: bool) -> Option<(Vec<u8>, usize, usize)> {
    let opts = FlattenOptions {
        dpi: 72.0,
        jpeg_quality: 95,
        grayscale,
        ..FlattenOptions::default()
    };
    let out = flatten_test_pdf(name, "/MediaBox [0 0 100 100]", &opts)?;
    let start = find(&out, b"stream\n", find(&out, b"/DCTDecode", 0)) + b"stream\n".len();
    let end = find(&out, b"\nendstream", start);
    let mut decoder = jpeg_decoder::Decoder::new(&out[start..end]);
    let pixels = decoder.decode().unwrap();
    let info = decoder.info().unwrap();
    let components = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => 1,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        other => panic!("unexpected pixel format {other:?}"),
    };
    Some((pixels, info.width as usize, components))
}

fn media_box(pdf: &[u8]) -> String {
    let at = find(pdf, b"/MediaBox [", 0) + b"/MediaBox [".len();
    let end = find(pdf, b"]", at);
    String::from_utf8(pdf[at..end].to_vec()).unwrap()
}

#[test]
fn colors_are_preserved() {
    let Some((pixels, width, components)) = flatten_and_decode("rgb", false) else {
        return;
    };
    assert_eq!(components, 3);
    assert_near(
        pixel(&pixels, width, components, 25, 50),
        &[255, 0, 0],
        "left half (red)",
    );
    assert_near(
        pixel(&pixels, width, components, 75, 50),
        &[0, 0, 255],
        "right half (blue)",
    );
}

#[test]
fn grayscale_uses_correct_channel_weights() {
    let Some((pixels, width, components)) = flatten_and_decode("gray", true) else {
        return;
    };
    assert_eq!(components, 1);
    // Rec. 601 luma: red = 0.299 * 255, blue = 0.114 * 255.
    assert_near(
        pixel(&pixels, width, components, 25, 50),
        &[76],
        "red as gray",
    );
    assert_near(
        pixel(&pixels, width, components, 75, 50),
        &[29],
        "blue as gray",
    );
}

#[test]
fn page_size_matches_original() {
    // 200 DPI so the pixel size differs from the point size; the page must
    // still come out at its original 100 x 100 pt.
    let opts = FlattenOptions {
        dpi: 200.0,
        ..FlattenOptions::default()
    };
    let Some(out) = flatten_test_pdf("size", "/MediaBox [0 0 100 100]", &opts) else {
        return;
    };
    assert_eq!(media_box(&out), "0 0 100 100");
}

#[test]
fn rotated_page_keeps_its_displayed_orientation() {
    // A 200 x 100 pt landscape MediaBox rotated 90 degrees displays as portrait.
    let Some(out) = flatten_test_pdf(
        "rotate",
        "/MediaBox [0 0 200 100] /Rotate 90",
        &FlattenOptions::default(),
    ) else {
        return;
    };
    assert_eq!(media_box(&out), "0 0 100 200");
    assert!(
        !out.windows(7).any(|w| w == b"/Rotate"),
        "rotation must be baked into the image"
    );
}
