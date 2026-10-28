use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use flatten_core::{
    flatten, load_pdfium, to_images, watermark, FlattenError, FlattenOptions, ImageOptions,
    Packaging, PageSelection, Position, Progress, Rotation, Stamp, Summary, WatermarkOptions,
};

/// Offline PDF tools: flatten pages to images, export pages as PNG, add watermarks.
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
    /// Export pages as PNG images into a folder or a .zip file
    Images(ImagesArgs),
    /// Stamp a PNG image (e.g. a logo) onto pages. Text watermarks are made
    /// in the desktop app, which renders the text to an image first.
    Watermark(WatermarkArgs),
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

#[derive(Args)]
struct WatermarkArgs {
    #[command(flatten)]
    common: Common,

    /// The watermark picture (PNG; transparency is kept)
    #[arg(long, required = true)]
    image: PathBuf,

    /// Width as a percentage of the page width
    #[arg(long, default_value_t = 50.0)]
    size: f32,

    /// Counter-clockwise rotation in degrees
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    angle: f32,

    /// Opacity in percent (0 = invisible, 100 = solid)
    #[arg(long, default_value_t = 30.0)]
    opacity: f32,

    /// Where to place it
    #[arg(long, value_enum, default_value_t = PositionArg::Center)]
    position: PositionArg,

    /// Only these pages, e.g. "1-3,7"
    #[arg(long, conflicts_with = "except")]
    pages: Option<String>,

    /// Every page except these, e.g. "1,10-"
    #[arg(long)]
    except: Option<String>,

    /// Flatten afterwards so the watermark cannot be removed
    #[arg(long)]
    flatten: bool,

    /// Render resolution when flattening
    #[arg(long, default_value_t = 200.0, requires = "flatten")]
    dpi: f32,
}

#[derive(Clone, Copy, ValueEnum)]
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
    /// Repeat across the whole page
    Tile,
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
    let pdfium = load_pdfium(cli.pdfium_dir.as_deref())?;
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
                    flatten(&pdfium, input, output, &opts, cancel, progress)
                },
            )
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
                to_images(&pdfium, input, output, &opts, cancel, progress)
            })
        }
        Command::Watermark(args) => {
            let png = std::fs::read(&args.image)
                .with_context(|| format!("cannot read {}", args.image.display()))?;
            let opts = WatermarkOptions {
                size: args.size / 100.0,
                rotation: args.angle,
                opacity: args.opacity / 100.0,
                position: match args.position {
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
                pages: match (args.pages.clone(), args.except.clone()) {
                    (Some(only), _) => PageSelection::Only(only),
                    (_, Some(except)) => PageSelection::Except(except),
                    _ => PageSelection::All,
                },
                password: args.common.password.clone(),
                flatten: args.flatten.then(|| FlattenOptions {
                    dpi: args.dpi,
                    ..FlattenOptions::default()
                }),
                ..WatermarkOptions::new(Stamp::from_png(&png)?)
            };
            for_each_input(
                &args.common,
                "_watermarked.pdf",
                |input, output, cancel, progress| {
                    watermark(&pdfium, input, output, &opts, cancel, progress)
                },
            )
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
