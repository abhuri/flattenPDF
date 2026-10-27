// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use flatten_core::{
    flatten, load_pdfium, to_images, FlattenError, FlattenOptions, ImageOptions, Packaging, Pdfium,
    Progress, Rotation, Summary,
};
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
        for job in jobs {
            if state.cancel.load(Ordering::SeqCst) {
                emit(JobEvent::Cancelled { id: job.id });
                continue;
            }
            run_job(
                pdfium,
                &job,
                &task,
                output_dir.as_deref(),
                &state.cancel,
                &emit,
            );
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
