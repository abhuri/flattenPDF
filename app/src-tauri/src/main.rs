// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use flatten_core::{flatten, load_pdfium, FlattenError, Options, Pdfium, Progress};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

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
    pages: Option<u32>,
    encrypted: bool,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: u32,
    input: String,
    password: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    dpi: f32,
    quality: u8,
    grayscale: bool,
    /// None = save next to the input file.
    output_dir: Option<String>,
    suffix: String,
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

/// Reads page count and encryption state for files added to the list.
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
        encrypted: false,
        error: None,
    };
    match pdfium.load_pdf_from_file(&p, None) {
        Ok(doc) => info.pages = Some(doc.pages().len() as u32),
        Err(err) => match FlattenError::from(err) {
            FlattenError::PasswordRequired => info.encrypted = true,
            other => info.error = Some(other.to_string()),
        },
    }
    info
}

/// Starts flattening the given jobs one after another on a background thread.
/// Progress is reported through `job-event` events.
#[tauri::command]
fn start(
    app: AppHandle,
    state: State<AppState>,
    jobs: Vec<Job>,
    settings: Settings,
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
        for job in jobs {
            if state.cancel.load(Ordering::SeqCst) {
                emit(JobEvent::Cancelled { id: job.id });
                continue;
            }
            run_job(pdfium, &job, &settings, &state.cancel, &emit);
        }
        state.busy.store(false, Ordering::SeqCst);
        emit(JobEvent::BatchFinished);
    });
    Ok(())
}

fn run_job(
    pdfium: &Pdfium,
    job: &Job,
    settings: &Settings,
    cancel: &AtomicBool,
    emit: &(dyn Fn(JobEvent) + Sync),
) {
    let input = PathBuf::from(&job.input);
    let output = output_path(&input, settings);
    let opts = Options {
        dpi: settings.dpi,
        jpeg_quality: settings.quality,
        grayscale: settings.grayscale,
        password: job.password.clone().filter(|p| !p.is_empty()),
        threads: 0,
    };
    let id = job.id;
    let on_progress = |p: Progress| match p {
        Progress::Started { total_pages } => emit(JobEvent::Started { id, total_pages }),
        Progress::PageDone { done, total_pages } => emit(JobEvent::Progress {
            id,
            done,
            total_pages,
        }),
    };
    match flatten(pdfium, &input, &output, &opts, cancel, &on_progress) {
        Ok(summary) => emit(JobEvent::Finished {
            id,
            output: output.to_string_lossy().into_owned(),
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

/// `<dir>/<stem><suffix>.pdf`, adding " (2)", " (3)"... rather than overwriting.
fn output_path(input: &Path, settings: &Settings) -> PathBuf {
    let dir = match &settings.output_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => input.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let mut candidate = dir.join(format!("{stem}{}.pdf", settings.suffix));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}{} ({n}).pdf", settings.suffix));
        n += 1;
    }
    candidate
}

#[tauri::command]
fn cancel(state: State<AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
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
        .invoke_handler(tauri::generate_handler![inspect, start, cancel])
        .run(tauri::generate_context!())
        .expect("error while running FlattenPDF");
}
