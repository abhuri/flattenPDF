use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use flatten_core::{
    flatten, load_engine, to_images, unlock, FlattenError, FlattenOptions, ImageOptions, Packaging,
    Progress, Rotation, Summary, UnlockOptions,
};

/// Offline PDF tools: flatten pages to images, remove passwords, export pages as PNG.
#[derive(Parser)]
#[command(name = "flattenpdf", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Directory containing the PDFium library, if it is not next to the executable
    #[arg(long, global = true)]
    pdfium_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Convert every page into an image so text cannot be selected, searched,
    /// edited or extracted, and forms, layers and metadata are removed
    Flatten(FlattenArgs),
    /// Remove the open password and permission restrictions, keeping content as is
    Unlock(UnlockArgs),
    /// Export pages as PNG images into a folder or a .zip file
    Images(ImagesArgs),
}

#[derive(Args)]
struct Common {
    /// PDF files to process
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Output path (only with a single input). Default: next to the input
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Password for opening encrypted input PDFs
    #[arg(long)]
    password: Option<String>,
}

#[derive(Args)]
struct FlattenArgs {
    #[command(flatten)]
    common: Common,

    /// Render resolution in dots per inch
    #[arg(long, default_value_t = 200.0)]
    dpi: f32,

    /// JPEG quality (1-100)
    #[arg(short, long, default_value_t = 85)]
    quality: u8,

    /// Produce a grayscale PDF (smaller files)
    #[arg(long)]
    gray: bool,

    /// JPEG encoder threads (0 = all cores)
    #[arg(long, default_value_t = 0)]
    threads: usize,
}

#[derive(Args)]
struct UnlockArgs {
    #[command(flatten)]
    common: Common,
}

#[derive(Args)]
struct ImagesArgs {
    #[command(flatten)]
    common: Common,

    /// Render resolution in dots per inch
    #[arg(long, default_value_t = 200.0)]
    dpi: f32,

    /// Rotate every image
    #[arg(long, value_enum, default_value_t = RotateArg::None)]
    rotate: RotateArg,

    /// Write a .zip archive instead of a folder
    #[arg(long)]
    zip: bool,

    /// Pages to export, e.g. "1-3,7,10-" (default: all)
    #[arg(long, default_value = "")]
    pages: String,

    /// PNG encoder threads (0 = all cores)
    #[arg(long, default_value_t = 0)]
    threads: usize,
}

#[derive(Clone, Copy, ValueEnum)]
enum RotateArg {
    None,
    /// 90 degrees clockwise
    Right,
    /// 90 degrees counter-clockwise
    Left,
    /// 180 degrees
    Half,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let engine = load_engine(cli.pdfium_dir.as_deref())?;
    match cli.command {
        Command::Flatten(args) => {
            let opts = FlattenOptions {
                dpi: args.dpi,
                jpeg_quality: args.quality,
                grayscale: args.gray,
                password: args.common.password.clone(),
                threads: args.threads,
            };
            for_each_input(
                &args.common,
                "_flattened.pdf",
                |input, output, cancel, progress| {
                    flatten(&engine, input, output, &opts, cancel, progress)
                },
            )
        }
        Command::Unlock(args) => {
            let opts = UnlockOptions {
                password: args.common.password.clone(),
            };
            for_each_input(&args.common, "_unlocked.pdf", |input, output, _, _| {
                unlock(&engine, input, output, &opts)
            })
        }
        Command::Images(args) => {
            let opts = ImageOptions {
                dpi: args.dpi,
                rotation: match args.rotate {
                    RotateArg::None => Rotation::None,
                    RotateArg::Right => Rotation::Right,
                    RotateArg::Left => Rotation::Left,
                    RotateArg::Half => Rotation::Half,
                },
                packaging: if args.zip {
                    Packaging::Zip
                } else {
                    Packaging::Folder
                },
                pages: args.pages.clone(),
                password: args.common.password.clone(),
                threads: args.threads,
            };
            let suffix = if args.zip { "_images.zip" } else { "_images" };
            for_each_input(&args.common, suffix, |input, output, cancel, progress| {
                to_images(&engine, input, output, &opts, cancel, progress)
            })
        }
    }
}

/// Runs `job(input, output, cancel, progress)` for every input, printing
/// progress, and fails if any input failed.
fn for_each_input(
    common: &Common,
    suffix: &str,
    job: impl Fn(&Path, &Path, &AtomicBool, &(dyn Fn(Progress) + Sync)) -> Result<Summary, FlattenError>,
) -> Result<()> {
    if common.output.is_some() && common.inputs.len() > 1 {
        bail!("--output can only be used with a single input file");
    }
    let cancel = AtomicBool::new(false);
    let mut failed = 0;
    for input in &common.inputs {
        let output = common
            .output
            .clone()
            .unwrap_or_else(|| default_output(input, suffix));
        eprintln!("{} -> {}", input.display(), output.display());
        let progress = |p: Progress| match p {
            Progress::Started { total_pages } => eprint!("  {total_pages} pages"),
            Progress::PageDone { done, total_pages } => {
                eprint!("\r  page {done}/{total_pages}");
                let _ = std::io::stderr().flush();
            }
        };
        match job(input, &output, &cancel, &progress)
            .with_context(|| format!("failed to process {}", input.display()))
        {
            Ok(summary) => eprintln!(
                "\r  done: {} pages, {:.1} MB, {:.1}s",
                summary.pages,
                summary.output_bytes as f64 / 1_048_576.0,
                summary.elapsed.as_secs_f64()
            ),
            Err(err) => {
                eprintln!("\n  error: {err:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} file(s) failed");
    }
    Ok(())
}

fn default_output(input: &Path, suffix: &str) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    input.with_file_name(format!("{stem}{suffix}"))
}
