//! Helpers shared by the end-to-end tests.
//!
//! The tests need the PDFium library in `<repo>/pdfium` (see
//! scripts/fetch-pdfium.sh); they are skipped with a message when it is missing.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::OnceLock;

use flatten_core::{load_pdfium, Pdfium};

/// PDFium can only be bound once per process, so all tests share one instance.
pub fn pdfium() -> Option<&'static Pdfium> {
    static PDFIUM: OnceLock<Option<Pdfium>> = OnceLock::new();
    PDFIUM
        .get_or_init(|| {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pdfium");
            load_pdfium(Some(&dir))
                .map_err(|e| eprintln!("skipping: {e}"))
                .ok()
        })
        .as_ref()
}

/// A fresh, empty temporary directory, removed when the guard drops.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("flattenpdf-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A page with the left 50 pt filled pure red and the next 50 pt pure blue.
/// `page_attrs` sets the page geometry, e.g. `/MediaBox [0 0 100 100]`.
pub fn red_blue_pdf(page_attrs: &str) -> Vec<u8> {
    let content = "1 0 0 rg 0 0 50 100 re f 0 0 1 rg 50 0 50 100 re f";
    assemble(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R {page_attrs} /Contents 4 0 R >>"),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ])
}

/// A PDF of `count` identical pages with the given geometry (e.g.
/// `/MediaBox [0 0 300 100]`), each showing its own page number.
pub fn multi_page_pdf(page_attrs: &str, count: usize) -> Vec<u8> {
    // obj 1: catalog, obj 2: pages, then a page + content object per page.
    let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", 3 + i * 2)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {count} >>",
            kids.join(" ")
        ),
    ];
    for i in 0..count {
        let content = format!("1 0 0 rg 10 10 {} 30 re f", 10 + i * 10);
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R {page_attrs} /Contents {} 0 R >>",
            4 + i * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    assemble(&objects)
}

/// Writes `objects` (1-based) into a PDF with an xref table and trailer.
fn assemble(objects: &[String]) -> Vec<u8> {
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

pub fn find(haystack: &[u8], needle: &[u8], from: usize) -> usize {
    from + haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("pattern not found")
}

pub fn pixel(pixels: &[u8], width: usize, components: usize, x: usize, y: usize) -> &[u8] {
    let i = (y * width + x) * components;
    &pixels[i..i + components]
}

pub fn assert_near(actual: &[u8], expected: &[u8], what: &str) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(*e) <= 12);
    assert!(close, "{what}: expected about {expected:?}, got {actual:?}");
}
