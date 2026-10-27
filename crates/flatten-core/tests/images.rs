//! End-to-end checks for exporting pages as PNG images.

mod common;

use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use common::{assert_near, multi_page_pdf, pdfium, pixel, red_blue_pdf, TempDir};
use flatten_core::{to_images, ImageOptions, Packaging, Rotation};

/// Decodes a PNG into (RGB pixels, width, height).
fn decode_png(bytes: &[u8]) -> (Vec<u8>, usize, usize) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    buf.truncate(info.buffer_size());
    (buf, info.width as usize, info.height as usize)
}

fn export(input: &Path, output: &Path, opts: &ImageOptions) -> bool {
    let Some(pdfium) = pdfium() else { return false };
    to_images(
        pdfium,
        input,
        output,
        opts,
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    true
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn exports_pages_into_a_folder() {
    let dir = TempDir::new("folder");
    let input = dir.join("in.pdf");
    std::fs::write(&input, red_blue_pdf("/MediaBox [0 0 100 100]")).unwrap();
    let output = dir.join("in_images");
    let opts = ImageOptions {
        dpi: 72.0,
        ..ImageOptions::default()
    };
    if !export(&input, &output, &opts) {
        return;
    }

    assert_eq!(file_names(&output), ["page-001.png"]);
    let (pixels, width, height) = decode_png(&std::fs::read(output.join("page-001.png")).unwrap());
    assert_eq!((width, height), (100, 100));
    assert_near(
        pixel(&pixels, width, 3, 25, 50),
        &[255, 0, 0],
        "left half (red)",
    );
    assert_near(
        pixel(&pixels, width, 3, 75, 50),
        &[0, 0, 255],
        "right half (blue)",
    );
    assert!(!dir.join("in_images.part").exists());
}

#[test]
fn rotates_images() {
    // 100 x 50 pt page: red on the left, blue on the right.
    let dir = TempDir::new("rotate");
    let input = dir.join("in.pdf");
    std::fs::write(&input, red_blue_pdf("/MediaBox [0 0 100 50]")).unwrap();

    let cases = [
        (Rotation::Right, (50, 100), [255, 0, 0], "right: red on top"),
        (Rotation::Left, (50, 100), [0, 0, 255], "left: blue on top"),
        (
            Rotation::Half,
            (100, 50),
            [0, 0, 255],
            "180: blue on the left",
        ),
    ];
    for (rotation, size, top_left, what) in cases {
        let output = dir.join(&format!("{rotation:?}"));
        let opts = ImageOptions {
            dpi: 72.0,
            rotation,
            ..ImageOptions::default()
        };
        if !export(&input, &output, &opts) {
            return;
        }
        let (pixels, width, height) =
            decode_png(&std::fs::read(output.join("page-001.png")).unwrap());
        assert_eq!((width, height), size, "{what}");
        assert_near(pixel(&pixels, width, 3, 10, 10), &top_left, what);
    }
}

#[test]
fn exports_selected_pages_into_a_zip() {
    let dir = TempDir::new("zip");
    let input = dir.join("in.pdf");
    std::fs::write(&input, multi_page_pdf("/MediaBox [0 0 300 100]", 3)).unwrap();
    let output = dir.join("pages.zip");
    let opts = ImageOptions {
        dpi: 72.0,
        packaging: Packaging::Zip,
        pages: "2".into(),
        ..ImageOptions::default()
    };
    if !export(&input, &output, &opts) {
        return;
    }

    let mut archive = zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
    assert_eq!(archive.len(), 1);
    let mut entry = archive.by_index(0).unwrap();
    assert_eq!(entry.name().unwrap(), "page-002.png");
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    let (_, width, height) = decode_png(&bytes);
    assert_eq!((width, height), (300, 100));
}

#[test]
fn refuses_to_overwrite_existing_output() {
    let Some(pdfium) = pdfium() else { return };
    let dir = TempDir::new("exists");
    let input = dir.join("in.pdf");
    std::fs::write(&input, red_blue_pdf("/MediaBox [0 0 100 100]")).unwrap();
    let output = dir.join("in_images");
    std::fs::create_dir(&output).unwrap();

    let result = to_images(
        pdfium,
        &input,
        &output,
        &ImageOptions::default(),
        &AtomicBool::new(false),
        &|_| {},
    );
    assert!(result.is_err());
}
