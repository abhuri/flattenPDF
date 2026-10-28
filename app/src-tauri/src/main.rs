// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use flatten_core::{
    flatten, load_pdfium, to_images, watermark, watermark_preview, FlattenError, FlattenOptions,
    ImageOptions, Packaging, PageSelection, Pdfium, Position, Progress, Rotation, Stamp, Summary,
    WatermarkOptions,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// Largest image file accepted as a watermark picture.
const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;
const WATERMARK_STATE_FILE: &str = "watermark.json";

struct AppState {
    pdfium: Result<Pdfium, String>,
    cancel: Arc<AtomicBool>,
    busy: AtomicBool,
}

impl AppState {
    fn pdfium(&self) -> Result<&Pdfium, String> {
        self.pdfium.as_ref().map_err(Clone::clone)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileInfo {
    path: String,
    name: String,
    size_bytes: u64,
    /// Unknown until the file can be opened (wrong password or broken file).
    pages: Option<u32>,
    /// True when a password is needed to open the file.
    locked: bool,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: u32,
    input: String,
    password: Option<String>,
}

/// What to do with every job of a batch, with that task's settings.
#[derive(Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Task {
    Flatten {
        dpi: f32,
        quality: u8,
        grayscale: bool,
        suffix: String,
    },
    Images {
        dpi: f32,
        rotation: RotationArg,
        packaging: PackagingArg,
        pages: String,
    },
    Watermark(WatermarkSettings),
}

/// Watermark settings as the UI sends them (percentages, not fractions).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatermarkSettings {
    /// The stamp as a PNG data URL; the UI renders text watermarks to PNG.
    stamp: String,
    size: f32,
    rotation: f32,
    opacity: f32,
    position: PositionArg,
    page_mode: PageModeArg,
    pages: String,
    flatten: bool,
    dpi: f32,
    quality: u8,
    suffix: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum PositionArg {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    Tile,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum PageModeArg {
    All,
    Only,
    Except,
}

impl WatermarkSettings {
    fn decode_stamp(&self) -> Result<Stamp, FlattenError> {
        let data = self
            .stamp
            .split_once("base64,")
            .map_or(self.stamp.as_str(), |(_, data)| data);
        let png = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| FlattenError::InvalidOptions(format!("watermark image: {e}")))?;
        Stamp::from_png(&png)
    }

    fn options(&self, stamp: Stamp, password: Option<String>) -> WatermarkOptions {
        WatermarkOptions {
            size: self.size / 100.0,
            rotation: self.rotation,
            opacity: self.opacity / 100.0,
            position: match self.position {
                PositionArg::TopLeft => Position::TopLeft,
                PositionArg::Top => Position::Top,
                PositionArg::TopRight => Position::TopRight,
                PositionArg::Left => Position::Left,
                PositionArg::Center => Position::Center,
                PositionArg::Right => Position::Right,
                PositionArg::BottomLeft => Position::BottomLeft,
                PositionArg::Bottom => Position::Bottom,
                PositionArg::BottomRight => Position::BottomRight,
                PositionArg::Tile => Position::Tile,
            },
            pages: match self.page_mode {
                PageModeArg::All => PageSelection::All,
                PageModeArg::Only => PageSelection::Only(self.pages.clone()),
                PageModeArg::Except => PageSelection::Except(self.pages.clone()),
            },
            password,
            flatten: self.flatten.then(|| FlattenOptions {
                dpi: self.dpi,
                jpeg_quality: self.quality,
                ..FlattenOptions::default()
            }),
            ..WatermarkOptions::new(stamp)
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum RotationArg {
    None,
    Right,
    Left,
    Half,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum PackagingArg {
    Folder,
    Zip,
}

#[derive(Clone, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
enum JobEvent {
    Started {
        id: u32,
        total_pages: usize,
    },
    Progress {
        id: u32,
        done: usize,
        total_pages: usize,
    },
    Finished {
        id: u32,
        output: String,
        pages: usize,
        bytes: u64,
        seconds: f64,
    },
    Failed {
        id: u32,
        message: String,
        password_required: bool,
    },
    Cancelled {
        id: u32,
    },
    BatchFinished,
}

/// Reads page count and whether a password is needed for files added to a list.
#[tauri::command]
async fn inspect(app: AppHandle, paths: Vec<String>) -> Result<Vec<FileInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let pdfium = state.pdfium()?;
        Ok(paths
            .into_iter()
            .map(|path| inspect_one(pdfium, path))
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn inspect_one(pdfium: &Pdfium, path: String) -> FileInfo {
    let p = PathBuf::from(&path);
    let mut info = FileInfo {
        name: p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        size_bytes: std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
        path,
        pages: None,
        locked: false,
        error: None,
    };
    match pdfium.load_pdf_from_file(&p, None) {
        Ok(doc) => info.pages = Some(doc.pages().len() as u32),
        Err(err) => match FlattenError::from(err) {
            FlattenError::PasswordRequired => info.locked = true,
            other => info.error = Some(other.to_string()),
        },
    }
    info
}

/// Runs `task` on the given jobs one after another on a background thread.
/// Progress is reported through `job-event` events.
#[tauri::command]
fn start(
    app: AppHandle,
    state: State<AppState>,
    jobs: Vec<Job>,
    task: Task,
    output_dir: Option<String>,
) -> Result<(), String> {
    state.pdfium()?;
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("already running".into());
    }
    state.cancel.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let pdfium = state.pdfium().expect("checked above");
        let emit = |event: JobEvent| {
            let _ = app.emit("job-event", event);
        };
        // Decode the watermark once for the whole batch.
        let stamp = match &task {
            Task::Watermark(settings) => settings.decode_stamp().map(Some),
            _ => Ok(None),
        };
        for job in jobs {
            if state.cancel.load(Ordering::SeqCst) {
                emit(JobEvent::Cancelled { id: job.id });
                continue;
            }
            match &stamp {
                Ok(stamp) => run_job(
                    pdfium,
                    &job,
                    &task,
                    stamp.as_ref(),
                    output_dir.as_deref(),
                    &state.cancel,
                    &emit,
                ),
                Err(err) => emit(JobEvent::Failed {
                    id: job.id,
                    message: err.to_string(),
                    password_required: false,
                }),
            }
        }
        state.busy.store(false, Ordering::SeqCst);
        emit(JobEvent::BatchFinished);
    });
    Ok(())
}

fn run_job(
    pdfium: &Pdfium,
    job: &Job,
    task: &Task,
    stamp: Option<&Stamp>,
    output_dir: Option<&str>,
    cancel: &AtomicBool,
    emit: &(dyn Fn(JobEvent) + Sync),
) {
    let input = PathBuf::from(&job.input);
    let password = job.password.clone().filter(|p| !p.is_empty());
    let id = job.id;
    let on_progress = |p: Progress| match p {
        Progress::Started { total_pages } => emit(JobEvent::Started { id, total_pages }),
        Progress::PageDone { done, total_pages } => emit(JobEvent::Progress {
            id,
            done,
            total_pages,
        }),
    };

    let result: Result<Summary, FlattenError> = match task {
        Task::Flatten {
            dpi,
            quality,
            grayscale,
            suffix,
        } => {
            let output = output_path(&input, output_dir, suffix, ".pdf");
            let opts = FlattenOptions {
                dpi: *dpi,
                jpeg_quality: *quality,
                grayscale: *grayscale,
                password,
                threads: 0,
            };
            flatten(pdfium, &input, &output, &opts, cancel, &on_progress)
        }
        Task::Images {
            dpi,
            rotation,
            packaging,
            pages,
        } => {
            let (packaging, extension) = match packaging {
                PackagingArg::Folder => (Packaging::Folder, ""),
                PackagingArg::Zip => (Packaging::Zip, ".zip"),
            };
            let output = output_path(&input, output_dir, "_images", extension);
            let opts = ImageOptions {
                dpi: *dpi,
                rotation: match rotation {
                    RotationArg::None => Rotation::None,
                    RotationArg::Right => Rotation::Right,
                    RotationArg::Left => Rotation::Left,
                    RotationArg::Half => Rotation::Half,
                },
                packaging,
                pages: pages.clone(),
                password,
                threads: 0,
            };
            to_images(pdfium, &input, &output, &opts, cancel, &on_progress)
        }
        Task::Watermark(settings) => {
            let output = output_path(&input, output_dir, &settings.suffix, ".pdf");
            let stamp = stamp.expect("decoded for watermark batches").clone();
            let opts = settings.options(stamp, password);
            watermark(pdfium, &input, &output, &opts, cancel, &on_progress)
        }
    };

    match result {
        Ok(summary) => emit(JobEvent::Finished {
            id,
            output: summary.output.to_string_lossy().into_owned(),
            pages: summary.pages,
            bytes: summary.output_bytes,
            seconds: summary.elapsed.as_secs_f64(),
        }),
        Err(FlattenError::Cancelled) => emit(JobEvent::Cancelled { id }),
        Err(err) => emit(JobEvent::Failed {
            id,
            password_required: matches!(err, FlattenError::PasswordRequired),
            message: err.to_string(),
        }),
    }
}

/// `<dir>/<stem><suffix><extension>`, adding " (2)", " (3)"... rather than
/// overwriting. `dir` defaults to the input's folder.
fn output_path(input: &Path, output_dir: Option<&str>, suffix: &str, extension: &str) -> PathBuf {
    let dir = match output_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => input.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let mut candidate = dir.join(format!("{stem}{suffix}{extension}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}{suffix} ({n}){extension}"));
        n += 1;
    }
    candidate
}

#[tauri::command]
fn cancel(state: State<AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

/// Renders one page with the watermark applied, as a PNG data URL.
#[tauri::command]
async fn preview(
    app: AppHandle,
    path: String,
    password: Option<String>,
    page: usize,
    settings: WatermarkSettings,
    max_pixels: u32,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let pdfium = state.pdfium()?;
        let stamp = settings.decode_stamp().map_err(|e| e.to_string())?;
        let opts = settings.options(stamp, password.filter(|p| !p.is_empty()));
        let png = watermark_preview(
            pdfium,
            Path::new(&path),
            page,
            &opts,
            max_pixels.clamp(200, 2000),
        )
        .map_err(|e| e.to_string())?;
        Ok(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reads a picture chosen as a watermark, as a data URL the UI can draw.
#[tauri::command]
async fn read_image(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = PathBuf::from(path);
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "svg" => "image/svg+xml",
            _ => return Err("unsupported image type".to_string()),
        };
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        if size > MAX_IMAGE_BYTES {
            return Err("the image is larger than 25 MB".to_string());
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        Ok(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn watermark_state_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(WATERMARK_STATE_FILE))
}

/// The watermark state saved by the UI (current settings and profiles), or
/// null before anything was saved.
#[tauri::command]
fn load_watermark_state(app: AppHandle) -> Result<serde_json::Value, String> {
    match std::fs::read(watermark_state_path(&app)?) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Value::Null),
        Err(err) => Err(err.to_string()),
    }
}

#[tauri::command]
fn save_watermark_state(app: AppHandle, data: serde_json::Value) -> Result<(), String> {
    let path = watermark_state_path(&app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Write a temporary file first so a crash never leaves half a file.
    let temp = path.with_extension("json.part");
    let bytes = serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())?;
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Bundled builds ship PDFium under <resources>/pdfium.
            let resource_dir = app.path().resource_dir().ok().map(|dir| dir.join("pdfium"));
            let pdfium = load_pdfium(resource_dir.as_deref()).map_err(|e| e.to_string());
            app.manage(AppState {
                pdfium,
                cancel: Arc::new(AtomicBool::new(false)),
                busy: AtomicBool::new(false),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            inspect,
            start,
            cancel,
            preview,
            read_image,
            load_watermark_state,
            save_watermark_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running FlattenPDF");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The JSON shape sent by ui/watermark.js (`task()` and `preview`).
    #[test]
    fn watermark_settings_from_the_ui_parse() {
        let task: Task = serde_json::from_value(serde_json::json!({
            "mode": "watermark",
            "stamp": "data:image/png;base64,AAAA",
            "size": 60,
            "rotation": -45,
            "opacity": 25,
            "position": "topLeft",
            "pageMode": "except",
            "pages": "1, 10-",
            "flatten": true,
            "dpi": 200,
            "quality": 85,
            "suffix": "_watermarked"
        }))
        .unwrap();
        let Task::Watermark(settings) = task else {
            panic!("expected a watermark task");
        };
        let opts = settings.options(
            Stamp {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            },
            None,
        );
        assert_eq!(opts.position, Position::TopLeft);
        assert_eq!(opts.pages, PageSelection::Except("1, 10-".into()));
        assert!((opts.opacity - 0.25).abs() < 1e-6 && (opts.size - 0.6).abs() < 1e-6);
        assert_eq!(opts.flatten.map(|f| f.dpi), Some(200.0));
    }
}
