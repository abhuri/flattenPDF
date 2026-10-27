//! Unlock: save a copy of the PDF without encryption, passwords or permission
//! restrictions. Content (text, forms, bookmarks, metadata) is kept as is.
//!
//! `pdfium-render` does not expose `FPDF_REMOVE_SECURITY`, so this module
//! talks to PDFium through the engine's raw bindings with small file
//! read/write adapters.

use std::ffi::{c_int, c_uchar, c_ulong, c_void};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

use pdfium_render::prelude::*;

use crate::{ensure_not_input, part_path, remove_part, Engine, FlattenError, Summary};

/// fpdf_save.h: write a copy without the security handler.
const FPDF_REMOVE_SECURITY: FPDF_DWORD = 3;
/// fpdfview.h: FPDF_GetLastError() value for a wrong or missing password.
const FPDF_ERR_PASSWORD: c_ulong = 4;

#[derive(Debug, Clone, Default)]
pub struct UnlockOptions {
    /// The document open (user) password, if the file needs one. Files that
    /// only carry an owner password / permission restrictions need none.
    pub password: Option<String>,
}

/// How a PDF is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// No encryption.
    None,
    /// Opens without a password but has an owner password, usually with
    /// printing, copying or editing restricted.
    OwnerRestricted,
    /// A password is needed to open it.
    PasswordRequired,
}

/// Reports how `path` is protected without needing any password.
pub fn inspect_security(engine: &Engine, path: &Path) -> Result<Security, FlattenError> {
    match RawDocument::open(engine, path, None) {
        Ok(doc) => Ok(if doc.is_encrypted() {
            Security::OwnerRestricted
        } else {
            Security::None
        }),
        Err(FlattenError::PasswordRequired) => Ok(Security::PasswordRequired),
        Err(err) => Err(err),
    }
}

/// Writes `input` to `output` with all security removed.
pub fn unlock(
    engine: &Engine,
    input: &Path,
    output: &Path,
    opts: &UnlockOptions,
) -> Result<Summary, FlattenError> {
    ensure_not_input(input, output)?;
    let started = Instant::now();
    let password = opts.password.as_deref().filter(|p| !p.is_empty());
    let doc = RawDocument::open(engine, input, password)?;
    let pages = doc.page_count();

    let temp_path = part_path(output);
    match doc.save_without_security(&temp_path) {
        Ok(()) => {
            drop(doc);
            fs::rename(&temp_path, output)?;
            Ok(Summary {
                pages,
                output_bytes: fs::metadata(output)?.len(),
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

/// An FPDF_DOCUMENT opened through the raw bindings. Closed on drop.
struct RawDocument<'a> {
    engine: &'a Engine,
    handle: FPDF_DOCUMENT,
    // PDFium reads from this for as long as the document is open.
    _access: Box<FileAccess>,
}

#[repr(C)]
struct FileAccess {
    base: FPDF_FILEACCESS,
    file: File,
}

impl<'a> RawDocument<'a> {
    fn open(engine: &'a Engine, path: &Path, password: Option<&str>) -> Result<Self, FlattenError> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let len = c_ulong::try_from(len).map_err(|_| {
            FlattenError::InvalidOptions("file is too large to unlock (over 4 GB)".into())
        })?;
        let mut access = Box::new(FileAccess {
            base: FPDF_FILEACCESS {
                m_FileLen: len,
                m_GetBlock: Some(read_block),
                m_Param: std::ptr::null_mut(),
            },
            file,
        });
        access.base.m_Param = &mut *access as *mut FileAccess as *mut c_void;

        let raw = engine.raw();
        let handle = unsafe { raw.FPDF_LoadCustomDocument(&mut access.base, password) };
        if handle.is_null() {
            return Err(match unsafe { raw.FPDF_GetLastError() } {
                FPDF_ERR_PASSWORD => FlattenError::PasswordRequired,
                _ => FlattenError::PdfiumFailed("open this file as a PDF"),
            });
        }
        Ok(Self {
            engine,
            handle,
            _access: access,
        })
    }

    fn is_encrypted(&self) -> bool {
        unsafe {
            self.engine
                .raw()
                .FPDF_GetSecurityHandlerRevision(self.handle)
                != -1
        }
    }

    fn page_count(&self) -> usize {
        unsafe { self.engine.raw().FPDF_GetPageCount(self.handle).max(0) as usize }
    }

    fn save_without_security(&self, path: &Path) -> Result<(), FlattenError> {
        let mut writer = Box::new(FileWrite {
            base: FPDF_FILEWRITE {
                version: 1,
                WriteBlock: Some(write_block),
            },
            out: BufWriter::with_capacity(1 << 20, File::create(path)?),
            error: None,
        });
        let ok = unsafe {
            self.engine
                .raw()
                .FPDF_SaveAsCopy(self.handle, &mut writer.base, FPDF_REMOVE_SECURITY)
        };
        if let Some(err) = writer.error.take() {
            return Err(err.into());
        }
        if !self.engine.raw().is_true(ok) {
            return Err(FlattenError::PdfiumFailed("save the unlocked copy"));
        }
        writer.out.flush()?;
        writer.out.get_ref().sync_all()?;
        Ok(())
    }
}

impl Drop for RawDocument<'_> {
    fn drop(&mut self) {
        unsafe { self.engine.raw().FPDF_CloseDocument(self.handle) };
    }
}

#[repr(C)]
struct FileWrite {
    base: FPDF_FILEWRITE,
    out: BufWriter<File>,
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

unsafe extern "C" fn write_block(
    this: *mut FPDF_FILEWRITE,
    data: *const c_void,
    size: c_ulong,
) -> c_int {
    // `base` is the first field of the #[repr(C)] FileWrite.
    let writer = &mut *(this as *mut FileWrite);
    let data = std::slice::from_raw_parts(data as *const u8, size as usize);
    match writer.out.write_all(data) {
        Ok(()) => 1,
        Err(err) => {
            writer.error = Some(err);
            0
        }
    }
}
