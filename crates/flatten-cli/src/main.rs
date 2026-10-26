use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};
use clap::Parser;
use flatten_core::{flatten, load_pdfium, Options, Progress};

/// Convert every page of a PDF into an image so text cannot be selected,
/// searched, edited or extracted, and forms, layers and metadata are removed.
#[derive(Parser)]
#[command(name = "flattenpdf", version)]
struct Args {
    /// PDF files to flatten
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Output file (only with a single input). Default: <name>_flattened.pdf
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Render resolution in dots per inch
    #[arg(long, default_value_t = 200.0)]
    dpi: f32,

    /// JPEG quality (1-100)
    #[arg(short, long, default_value_t = 85)]
    quality: u8,

    /// Produce a grayscale PDF (smaller files)
    #[arg(long)]
    gray: bool,

    /// Password for opening encrypted input PDFs
    #[arg(long)]
    password: Option<String>,

    /// JPEG encoder threads (0 = all cores)
    #[arg(long, default_value_t = 0)]
    threads: usize,

    /// Directory containing the PDFium library, if it is not next to the executable
    #[arg(long)]
    pdfium_dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.output.is_some() && args.inputs.len() > 1 {
        bail!("--output can only be used with a single input file");
    }

    let pdfium = load_pdfium(args.pdfium_dir.as_deref())?;
    let opts = Options {
        dpi: args.dpi,
        jpeg_quality: args.quality,
        grayscale: args.gray,
        password: args.password.clone(),
        threads: args.threads,
    };
    let cancel = AtomicBool::new(false);

    let mut failed = 0;
    for input in &args.inputs {
        let output = args.output.clone().unwrap_or_else(|| default_output(input));
        eprintln!("{} -> {}", input.display(), output.display());
        let progress = |p: Progress| match p {
            Progress::Started { total_pages } => eprint!("  {total_pages} pages"),
            Progress::PageDone { done, total_pages } => {
                eprint!("\r  page {done}/{total_pages}");
                let _ = std::io::stderr().flush();
            }
        };
        match flatten(&pdfium, input, &output, &opts, &cancel, &progress)
            .with_context(|| format!("failed to flatten {}", input.display()))
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

fn default_output(input: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    input.with_file_name(format!("{stem}_flattened.pdf"))
}
