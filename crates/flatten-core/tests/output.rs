//! End-to-end checks: flatten a PDF with known colors and page size, then
//! inspect the output PDF and the JPEG inside it.
//!
//! Needs the PDFium library in `<repo>/pdfium` (see scripts/fetch-pdfium.sh);
//! the tests are skipped with a message when it is missing.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use flatten_core::{flatten, load_pdfium, Options, Pdfium};

/// PDFium can only be bound once per process, so all tests share it.
fn pdfium() -> Option<&'static Pdfium> {
    static PDFIUM: OnceLock<Option<Pdfium>> = OnceLock::new();
    PDFIUM
        .get_or_init(|| {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pdfium");
            load_pdfium(Some(&dir))
                .map_err(|e| eprintln!("skipping: {e}"))
                .ok()
        })
        .as_ref()
}

/// A page with the left 50 pt filled pure red and the next 50 pt pure blue.
/// `page_attrs` sets the page geometry, e.g. `/MediaBox [0 0 100 100]`.
fn red_blue_pdf(page_attrs: &str) -> Vec<u8> {
    let content = "1 0 0 rg 0 0 50 100 re f 0 0 1 rg 50 0 50 100 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R {page_attrs} /Contents 4 0 R >>"),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> usize {
    from + haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("pattern not found")
}

/// Flattens the red/blue page and returns the bytes of the output PDF.
fn flatten_test_pdf(name: &str, page_attrs: &str, opts: &Options) -> Option<Vec<u8>> {
    let pdfium = pdfium()?;
    let dir: PathBuf =
        std::env::temp_dir().join(format!("flattenpdf-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
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

    let out = std::fs::read(&output).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    Some(out)
}

/// Flattens the red/blue page and returns (pixels, width, components) of its JPEG.
fn flatten_and_decode(name: &str, grayscale: bool) -> Option<(Vec<u8>, usize, usize)> {
    let opts = Options {
        dpi: 72.0,
        jpeg_quality: 95,
        grayscale,
        ..Options::default()
    };
    let out = flatten_test_pdf(name, "/MediaBox [0 0 100 100]", &opts)?;
    let start = find(&out, b"stream\n", find(&out, b"/DCTDecode", 0)) + b"stream\n".len();
    let end = find(&out, b"\nendstream", start);
    let mut decoder = jpeg_decoder::Decoder::new(&out[start..end]);
    let pixels = decoder.decode().unwrap();
    let info = decoder.info().unwrap();
    Some((pixels, info.width as usize, pixels_per(info.pixel_format)))
}

fn pixels_per(format: jpeg_decoder::PixelFormat) -> usize {
    match format {
        jpeg_decoder::PixelFormat::L8 => 1,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        other => panic!("unexpected pixel format {other:?}"),
    }
}

fn pixel(pixels: &[u8], width: usize, components: usize, x: usize, y: usize) -> &[u8] {
    let i = (y * width + x) * components;
    &pixels[i..i + components]
}

fn assert_near(actual: &[u8], expected: &[u8], what: &str) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(*e) <= 12);
    assert!(close, "{what}: expected about {expected:?}, got {actual:?}");
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
    let opts = Options {
        dpi: 200.0,
        ..Options::default()
    };
    let Some(out) = flatten_test_pdf("size", "/MediaBox [0 0 100 100]", &opts) else {
        return;
    };
    let at = find(&out, b"/MediaBox [", 0) + b"/MediaBox [".len();
    let end = find(&out, b"]", at);
    let media_box = std::str::from_utf8(&out[at..end]).unwrap();
    assert_eq!(media_box, "0 0 100 100");
}

#[test]
fn rotated_page_keeps_its_displayed_orientation() {
    // A 200 x 100 pt landscape MediaBox rotated 90 degrees displays as portrait.
    let Some(out) = flatten_test_pdf(
        "rotate",
        "/MediaBox [0 0 200 100] /Rotate 90",
        &Options::default(),
    ) else {
        return;
    };
    let at = find(&out, b"/MediaBox [", 0) + b"/MediaBox [".len();
    let end = find(&out, b"]", at);
    let media_box = std::str::from_utf8(&out[at..end]).unwrap();
    assert_eq!(media_box, "0 0 100 200");
    assert!(
        !out.windows(7).any(|w| w == b"/Rotate"),
        "rotation must be baked into the image"
    );
}
