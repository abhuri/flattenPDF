//! Shared render pipeline:
//!
//! ```text
//! [calling thread: PDFium render] --bitmaps--> [N workers: encode] --results--> [sink thread: in order]
//! ```
//!
//! PDFium is not thread-safe, so rendering stays on the calling thread; the
//! expensive encoding step runs in parallel. Bounded channels cap how many
//! bitmaps are in flight, which is what keeps memory flat.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::bounded;
use pdfium_render::prelude::*;

use crate::FlattenError;

/// Largest bitmap edge we will render. Keeps huge drawings (A0 at high DPI)
/// within encoder limits and a sane memory budget.
const MAX_PIXEL_DIMENSION: f32 = 14_000.0;

pub(crate) struct RenderedPage {
    /// 1-based page number in the source document.
    pub page_number: usize,
    pub width: u32,
    pub height: u32,
    /// Pixels in R, G, B, A byte order.
    pub rgba: Vec<u8>,
    /// Original page size in points, in the rendered orientation.
    pub width_pt: f64,
    pub height_pt: f64,
}

/// Renders `page_indices` (0-based) of `document`, encodes each page with
/// `encode` on `threads` workers and hands the results to `sink` in page order.
///
/// Returns [`FlattenError::Cancelled`] if `cancel` was set before every page
/// reached the sink.
pub(crate) fn run<T: Send>(
    document: &PdfDocument,
    page_indices: &[usize],
    dpi: f32,
    threads: usize,
    cancel: &AtomicBool,
    encode: &(dyn Fn(RenderedPage) -> Result<T, FlattenError> + Sync),
    sink: &mut (dyn FnMut(T) -> Result<(), FlattenError> + Send),
) -> Result<(), FlattenError> {
    let pages = document.pages();
    std::thread::scope(|scope| {
        let (render_tx, render_rx) = bounded::<(usize, RenderedPage)>(threads * 2);
        let (encoded_tx, encoded_rx) = bounded::<(usize, Result<T, FlattenError>)>(threads * 2);

        for _ in 0..threads {
            let render_rx = render_rx.clone();
            let encoded_tx = encoded_tx.clone();
            scope.spawn(move || {
                for (seq, page) in render_rx {
                    if encoded_tx.send((seq, encode(page))).is_err() {
                        break; // sink gave up
                    }
                }
            });
        }
        drop(render_rx);
        drop(encoded_tx);

        let total = page_indices.len();
        let sink_thread = scope.spawn(move || -> Result<(), FlattenError> {
            // Workers finish out of order; hold early pages until their turn.
            let mut pending = BTreeMap::new();
            let mut next = 0;
            for (seq, encoded) in encoded_rx {
                pending.insert(seq, encoded?);
                while let Some(item) = pending.remove(&next) {
                    sink(item)?;
                    next += 1;
                }
            }
            if next == total {
                Ok(())
            } else {
                Err(FlattenError::Cancelled)
            }
        });

        let render_result = (|| -> Result<(), FlattenError> {
            for (seq, &index) in page_indices.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(FlattenError::Cancelled);
                }
                let page = pages.get(index as _)?;
                if render_tx
                    .send((seq, render_page(&page, index, dpi)?))
                    .is_err()
                {
                    break; // sink failed; its error is reported below
                }
            }
            Ok(())
        })();
        drop(render_tx);

        let sink_result = sink_thread.join().expect("sink thread panicked");
        render_result.and(sink_result)
    })
}

fn render_page(page: &PdfPage, index: usize, dpi: f32) -> Result<RenderedPage, FlattenError> {
    let mut scale = dpi / 72.0;
    let longest_pt = page.width().value.max(page.height().value);
    if longest_pt * scale > MAX_PIXEL_DIMENSION {
        scale = MAX_PIXEL_DIMENSION / longest_pt;
    }
    // A BGRA bitmap with FPDF_REVERSE_BYTE_ORDER holds R, G, B, A in memory.
    // Set the flag explicitly: pdfium-render enables it by default, and getting
    // the order wrong swaps red and blue in the output.
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .set_format(PdfBitmapFormat::BGRA)
        .set_reverse_byte_order(true)
        .set_clear_color(PdfColor::WHITE)
        .render_form_data(true)
        .render_annotations(true);
    let bitmap = page.render_with_config(&config)?;
    let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
    // Keep the exact original page size (pixel counts are rounded). The bitmap
    // already has page rotation applied, so follow its orientation.
    let (mut width_pt, mut height_pt) = (page.width().value as f64, page.height().value as f64);
    if (width > height) != (width_pt > height_pt) {
        std::mem::swap(&mut width_pt, &mut height_pt);
    }
    Ok(RenderedPage {
        page_number: index + 1,
        width,
        height,
        rgba: bitmap.as_raw_bytes(),
        width_pt,
        height_pt,
    })
}
