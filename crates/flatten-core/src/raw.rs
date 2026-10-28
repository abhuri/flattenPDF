//! Direct PDFium calls for the few things `pdfium-render` does not expose,
//! mainly sharing one form XObject across many pages when stamping.
//!
//! [`crate::load_pdfium`] binds the library twice: once for `pdfium-render`
//! and once for these raw bindings. Both go through `pdfium-render`'s
//! process-wide lock (the `thread_safe` feature), so they can be mixed freely.

use std::ffi::{c_int, c_uchar, c_ulong, c_void};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::OnceLock;

use pdfium_render::prelude::*;

use crate::FlattenError;

/// fpdfview.h: FPDF_GetLastError() value for a wrong or missing password.
const FPDF_ERR_PASSWORD: c_ulong = 4;

pub(crate) struct RawBindings(Box<dyn PdfiumLibraryBindings>);

// SAFETY: with the `thread_safe` feature every call is serialized through
// pdfium-render's global PdfiumThreadMarshall mutex.
unsafe impl Send for RawBindings {}
unsafe impl Sync for RawBindings {}

static RAW: OnceLock<RawBindings> = OnceLock::new();

pub(crate) fn install(bindings: Box<dyn PdfiumLibraryBindings>) {
    let _ = RAW.set(RawBindings(bindings));
}

pub(crate) fn bindings() -> Result<&'static dyn PdfiumLibraryBindings, FlattenError> {
    RAW.get()
        .map(|raw| raw.0.as_ref())
        .ok_or(FlattenError::PdfiumFailed("start (call load_pdfium first)"))
}

/// An FPDF_DOCUMENT opened or created through the raw bindings. Closed on drop.
pub(crate) struct RawDocument {
    raw: &'static dyn PdfiumLibraryBindings,
    pub(crate) handle: FPDF_DOCUMENT,
    // PDFium reads from this for as long as the document is open.
    _access: Option<Box<FileAccess>>,
}

#[repr(C)]
struct FileAccess {
    base: FPDF_FILEACCESS,
    file: File,
}

impl RawDocument {
    pub(crate) fn open(path: &Path, password: Option<&str>) -> Result<Self, FlattenError> {
        let raw = bindings()?;
        let file = File::open(path)?;
        let len = c_ulong::try_from(file.metadata()?.len())
            .map_err(|_| FlattenError::InvalidOptions("file is too large (over 4 GB)".into()))?;
        let mut access = Box::new(FileAccess {
            base: FPDF_FILEACCESS {
                m_FileLen: len,
                m_GetBlock: Some(read_block),
                m_Param: std::ptr::null_mut(),
            },
            file,
        });
        access.base.m_Param = &mut *access as *mut FileAccess as *mut c_void;

        let handle = unsafe { raw.FPDF_LoadCustomDocument(&mut access.base, password) };
        if handle.is_null() {
            return Err(match unsafe { raw.FPDF_GetLastError() } {
                FPDF_ERR_PASSWORD => FlattenError::PasswordRequired,
                _ => FlattenError::PdfiumFailed("open this file as a PDF"),
            });
        }
        Ok(Self {
            raw,
            handle,
            _access: Some(access),
        })
    }

    pub(crate) fn new_empty() -> Result<Self, FlattenError> {
        let raw = bindings()?;
        let handle = unsafe { raw.FPDF_CreateNewDocument() };
        if handle.is_null() {
            return Err(FlattenError::PdfiumFailed("create a document"));
        }
        Ok(Self {
            raw,
            handle,
            _access: None,
        })
    }

    pub(crate) fn page_count(&self) -> usize {
        unsafe { self.raw.FPDF_GetPageCount(self.handle).max(0) as usize }
    }

    /// Copies page `index` (0-based) of `source` to the end of this document.
    pub(crate) fn import_page(
        &self,
        source: &RawDocument,
        index: usize,
    ) -> Result<(), FlattenError> {
        let range = (index + 1).to_string();
        let at = self.page_count() as c_int;
        let ok = unsafe {
            self.raw
                .FPDF_ImportPages(self.handle, source.handle, &range, at)
        };
        if self.raw.is_true(ok) {
            Ok(())
        } else {
            Err(FlattenError::PdfiumFailed("copy the page"))
        }
    }

    pub(crate) fn load_page(&self, index: usize) -> Result<RawPage<'_>, FlattenError> {
        let handle = unsafe { self.raw.FPDF_LoadPage(self.handle, index as c_int) };
        if handle.is_null() {
            return Err(FlattenError::PdfiumFailed("load a page"));
        }
        Ok(RawPage {
            raw: self.raw,
            handle,
            _doc: std::marker::PhantomData,
        })
    }

    /// Saves a full copy (keeping any encryption) to `out`.
    pub(crate) fn save<W: Write>(&self, out: W) -> Result<W, FlattenError> {
        let mut writer = Box::new(FileWrite {
            base: FPDF_FILEWRITE {
                version: 1,
                WriteBlock: Some(write_block::<W>),
            },
            out,
            error: None,
        });
        let ok = unsafe { self.raw.FPDF_SaveAsCopy(self.handle, &mut writer.base, 0) };
        if let Some(err) = writer.error.take() {
            return Err(err.into());
        }
        if !self.raw.is_true(ok) {
            return Err(FlattenError::PdfiumFailed("save the document"));
        }
        let mut out = writer.out;
        out.flush()?;
        Ok(out)
    }

    pub(crate) fn save_to_file(&self, path: &Path) -> Result<(), FlattenError> {
        let out = self.save(BufWriter::with_capacity(1 << 20, File::create(path)?))?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(())
    }
}

impl Drop for RawDocument {
    fn drop(&mut self) {
        unsafe { self.raw.FPDF_CloseDocument(self.handle) };
    }
}

/// A loaded page. Closed on drop; call [`RawPage::generate_content`] first
/// to keep changes.
pub(crate) struct RawPage<'doc> {
    raw: &'static dyn PdfiumLibraryBindings,
    pub(crate) handle: FPDF_PAGE,
    _doc: std::marker::PhantomData<&'doc RawDocument>,
}

impl RawPage<'_> {
    /// Visible area in page space: the crop box, falling back to the media box.
    pub(crate) fn visible_box(&self) -> (f64, f64, f64, f64) {
        let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
        let ok = unsafe {
            self.raw
                .FPDFPage_GetCropBox(self.handle, &mut l, &mut b, &mut r, &mut t)
        };
        if !self.raw.is_true(ok) || r <= l || t <= b {
            unsafe {
                self.raw
                    .FPDFPage_GetMediaBox(self.handle, &mut l, &mut b, &mut r, &mut t)
            };
        }
        (l as f64, b as f64, r as f64, t as f64)
    }

    /// Clockwise display rotation in quarter turns (0-3).
    pub(crate) fn quarter_turns(&self) -> i32 {
        unsafe { self.raw.FPDFPage_GetRotation(self.handle) }.rem_euclid(4)
    }

    pub(crate) fn insert(&self, object: FPDF_PAGEOBJECT) -> Result<(), FlattenError> {
        if self
            .raw
            .is_true(unsafe { self.raw.FPDFPage_InsertObject(self.handle, object) })
        {
            Ok(())
        } else {
            unsafe { self.raw.FPDFPageObj_Destroy(object) };
            Err(FlattenError::PdfiumFailed("add the watermark to a page"))
        }
    }

    pub(crate) fn generate_content(&self) -> Result<(), FlattenError> {
        if self
            .raw
            .is_true(unsafe { self.raw.FPDFPage_GenerateContent(self.handle) })
        {
            Ok(())
        } else {
            Err(FlattenError::PdfiumFailed("write the page content"))
        }
    }
}

impl Drop for RawPage<'_> {
    fn drop(&mut self) {
        unsafe { self.raw.FPDF_ClosePage(self.handle) };
    }
}

#[repr(C)]
struct FileWrite<W> {
    base: FPDF_FILEWRITE,
    out: W,
    error: Option<io::Error>,
}

// Callbacks must not unwind into PDFium, so every failure becomes a 0 return.

unsafe extern "C" fn read_block(
    param: *mut c_void,
    position: c_ulong,
    buf: *mut c_uchar,
    size: c_ulong,
) -> c_int {
    let access = &mut *(param as *mut FileAccess);
    let buf = std::slice::from_raw_parts_mut(buf, size as usize);
    let ok = access.file.seek(SeekFrom::Start(position as u64)).is_ok()
        && access.file.read_exact(buf).is_ok();
    ok as c_int
}

unsafe extern "C" fn write_block<W: Write>(
    this: *mut FPDF_FILEWRITE,
    data: *const c_void,
    size: c_ulong,
) -> c_int {
    // `base` is the first field of the #[repr(C)] FileWrite.
    let writer = &mut *(this as *mut FileWrite<W>);
    let data = std::slice::from_raw_parts(data as *const u8, size as usize);
    match writer.out.write_all(data) {
        Ok(()) => 1,
        Err(err) => {
            writer.error = Some(err);
            0
        }
    }
}
