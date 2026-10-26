# FlattenPDF

Converts every page of a PDF into an image and writes a new PDF that contains
**only those images**. The result looks the same, but:

- text cannot be selected, copied, searched or edited
- form fields, annotations, comments and hidden layers (OCG) are gone
- JavaScript, attachments, bookmarks and metadata (Info / XMP) are gone

> **Limitation:** the pages are still readable by people and by OCR. FlattenPDF
> prevents editing and direct data extraction; it does not hide what is visible.

Runs entirely offline on Windows, Linux and macOS.

## Desktop app

Download the installer for your OS from
[Releases](https://github.com/abhuri/flattenPDF/releases):
`.exe` / `.msi` (Windows), `.dmg` (macOS, Intel + Apple Silicon),
`.deb` / `.rpm` / `.AppImage` (Linux).

Drop PDFs onto the window (or click **Add PDFs…**), pick the resolution and
quality, then **Flatten**. Encrypted PDFs ask for their password inline.
Output goes next to the original as `<name>_flattened.pdf` unless you choose
another folder. The UI is available in English and Thai.

> The installers are not code-signed yet. On Windows choose *More info → Run
> anyway*; on macOS right-click the app and choose *Open* the first time.

## Usage (CLI)

```bash
flattenpdf input.pdf                       # -> input_flattened.pdf
flattenpdf input.pdf -o secured.pdf --dpi 300 --quality 90
flattenpdf *.pdf --gray                    # batch, grayscale
flattenpdf locked.pdf --password secret    # encrypted input
```

| Option | Default | |
|---|---|---|
| `--dpi` | 200 | render resolution (36–600) |
| `-q, --quality` | 85 | JPEG quality (1–100) |
| `--gray` | off | grayscale output, smaller files |
| `--threads` | all cores | JPEG encoder threads |
| `--pdfium-dir` | next to exe | where the PDFium library lives |

The PDFium library (`pdfium.dll` / `libpdfium.so` / `libpdfium.dylib`) must sit
next to the executable; release archives already include it.

## Performance

98-page A4 document (27 MB input), Windows, 16 threads:

| DPI | Time | Peak RAM | Output |
|---|---|---|---|
| 150 | 3.2 s | 74 MB | 21 MB |
| 200 | 4.5 s | 92 MB | 32 MB |
| 300 | 7.4 s | 169 MB | 58 MB |

Memory stays bounded by thread count, not page count: pages are rendered,
encoded and streamed to disk one at a time.

## How it works

```
[PDFium render, 1 thread] --bitmaps--> [JPEG encoders, N threads] --> [in-order PDF writer, streaming]
```

- `crates/flatten-core` – the pipeline and a minimal image-only PDF writer
- `crates/flatten-cli` – the `flattenpdf` command
- `app/` – the Tauri 2 desktop app (`src-tauri/` backend, `ui/` plain HTML/JS front end)

Output is written to `<output>.part` and renamed only on success.

## Building

Requires Rust (stable) and the prebuilt PDFium from
[bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)
(version must match the `pdfium_*` feature of `pdfium-render`).

```bash
scripts/fetch-pdfium.sh          # downloads into ./pdfium
cargo build --release
cp pdfium/* target/release/
```

Desktop app (needs Node.js; on Linux also the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/)):

```bash
cd app
npm install
npx tauri dev       # run
npx tauri build     # installers in target/release/bundle/
```

Tip: if the repo lives in a synced folder (Google Drive, OneDrive), set
`CARGO_TARGET_DIR` to a local path so build output is not synced.

## Releases

Pushing a tag `v*` builds the desktop installers and the CLI archives for
Windows, Linux and macOS and attaches them to a GitHub Release.

## Roadmap

- [x] Core pipeline + CLI
- [x] Desktop app (Tauri): drag & drop, progress, cancel, passwords, EN/TH
- [x] Installers (.exe / .msi / .dmg / .deb / .rpm / .AppImage)
- [ ] Watermark, page ranges, output password
- [ ] Code signing (Windows, macOS notarization)
