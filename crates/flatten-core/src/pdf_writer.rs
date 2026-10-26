//! Minimal streaming writer for image-only PDFs.
//!
//! Every page is a single JPEG XObject painted over the full MediaBox. Nothing
//! else is written: no Info dictionary, no XMP metadata, no fonts, no text, no
//! forms, no annotations. Pages are flushed to disk as they arrive, so memory
//! use stays flat regardless of page count.

use std::io::{self, Write};

const CATALOG_ID: u32 = 1;
const PAGES_ID: u32 = 2;

pub enum ColorSpace {
    Rgb,
    Gray,
}

impl ColorSpace {
    fn name(&self) -> &'static str {
        match self {
            ColorSpace::Rgb => "/DeviceRGB",
            ColorSpace::Gray => "/DeviceGray",
        }
    }
}

pub struct ImagePage<'a> {
    pub jpeg: &'a [u8],
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub color_space: ColorSpace,
    /// Page size in PDF points (1/72 inch).
    pub width_pt: f64,
    pub height_pt: f64,
}

pub struct ImagePdfWriter<W: Write> {
    out: W,
    pos: u64,
    /// Byte offset of each object, indexed by object id (index 0 is unused).
    offsets: Vec<u64>,
    page_ids: Vec<u32>,
}

impl<W: Write> ImagePdfWriter<W> {
    pub fn new(out: W) -> io::Result<Self> {
        let mut writer = Self {
            out,
            pos: 0,
            offsets: vec![0; PAGES_ID as usize + 1],
            page_ids: Vec::new(),
        };
        // The binary comment marks the file as containing 8-bit data.
        writer.write(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;
        Ok(writer)
    }

    pub fn add_page(&mut self, page: &ImagePage) -> io::Result<()> {
        let image_id = self.next_id();
        self.begin_object(image_id)?;
        self.write(
            format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {} \
                 /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                page.pixel_width,
                page.pixel_height,
                page.color_space.name(),
                page.jpeg.len()
            )
            .as_bytes(),
        )?;
        self.write(page.jpeg)?;
        self.write(b"\nendstream\nendobj\n")?;

        let content = format!(
            "q {} 0 0 {} 0 0 cm /Im0 Do Q",
            num(page.width_pt),
            num(page.height_pt)
        );
        let content_id = self.next_id();
        self.begin_object(content_id)?;
        self.write(
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n",
                content.len()
            )
            .as_bytes(),
        )?;

        let page_id = self.next_id();
        self.begin_object(page_id)?;
        self.write(
            format!(
                "<< /Type /Page /Parent {PAGES_ID} 0 R /MediaBox [0 0 {} {}] \
                 /Resources << /XObject << /Im0 {image_id} 0 R >> >> /Contents {content_id} 0 R >>\nendobj\n",
                num(page.width_pt),
                num(page.height_pt)
            )
            .as_bytes(),
        )?;
        self.page_ids.push(page_id);
        Ok(())
    }

    pub fn page_count(&self) -> usize {
        self.page_ids.len()
    }

    /// Writes the page tree, catalog, cross-reference table and trailer.
    pub fn finish(mut self) -> io::Result<W> {
        let kids: Vec<String> = self.page_ids.iter().map(|id| format!("{id} 0 R")).collect();
        self.begin_object(PAGES_ID)?;
        self.write(
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
                kids.join(" "),
                self.page_ids.len()
            )
            .as_bytes(),
        )?;
        self.begin_object(CATALOG_ID)?;
        self.write(format!("<< /Type /Catalog /Pages {PAGES_ID} 0 R >>\nendobj\n").as_bytes())?;

        let xref_pos = self.pos;
        let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", self.offsets.len());
        for offset in &self.offsets[1..] {
            xref.push_str(&format!("{offset:010} 00000 n \n"));
        }
        xref.push_str(&format!(
            "trailer\n<< /Size {} /Root {CATALOG_ID} 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n",
            self.offsets.len()
        ));
        self.write(xref.as_bytes())?;
        self.out.flush()?;
        Ok(self.out)
    }

    fn next_id(&mut self) -> u32 {
        self.offsets.push(0);
        (self.offsets.len() - 1) as u32
    }

    fn begin_object(&mut self, id: u32) -> io::Result<()> {
        self.offsets[id as usize] = self.pos;
        self.write(format!("{id} 0 obj\n").as_bytes())
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }
}

/// Formats a number for PDF output: at most 4 decimals, no trailing zeros.
fn num(value: f64) -> String {
    let s = format!("{value:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_compactly() {
        assert_eq!(num(595.0), "595");
        assert_eq!(num(841.889764), "841.8898");
        assert_eq!(num(0.0), "0");
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        // Stand-in image data; ASCII so the whole file can be inspected as text.
        let jpeg = *b"JPEG";
        let mut writer = ImagePdfWriter::new(Vec::new()).unwrap();
        for _ in 0..2 {
            writer
                .add_page(&ImagePage {
                    jpeg: &jpeg,
                    pixel_width: 1,
                    pixel_height: 1,
                    color_space: ColorSpace::Rgb,
                    width_pt: 612.0,
                    height_pt: 792.0,
                })
                .unwrap();
        }
        let bytes = writer.finish().unwrap();
        // Blank out the binary header comment; everything else is ASCII, so byte
        // offsets and string offsets stay identical.
        let text: String = bytes
            .iter()
            .map(|&b| if b.is_ascii() { b as char } else { ' ' })
            .collect();

        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(text[xref_at..].starts_with("xref\n0 9\n"));
        for (id, line) in text[xref_at..].lines().skip(3).take(8).enumerate() {
            let offset: usize = line[..10].parse().unwrap();
            assert!(
                bytes[offset..].starts_with(format!("{} 0 obj", id + 1).as_bytes()),
                "object {}",
                id + 1
            );
        }
        assert!(text.contains("/Count 2"));
        assert!(!text.contains("/Info"));
    }
}
