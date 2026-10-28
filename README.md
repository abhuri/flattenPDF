# FlattenPDF

Offline PDF tools for Windows, Linux and macOS, in one app with three tabs:

| Tab | What it does | Output |
|---|---|---|
| 🔒 **Flatten** | Converts every page into an image and writes a new PDF that contains only those images | `<name>_flattened.pdf` |
| 🖼️ **To Images** | Saves pages as PNG images, optionally rotated, into a folder or a single .zip | `<name>_images/page-001.png …` or `<name>_images.zip` |
| 💧 **Watermark** | Stamps text or a picture onto all, some, or all-but-some pages, optionally flattening the result | `<name>_watermarked.pdf` |

A flattened PDF looks the same, but:

- text cannot be selected, copied, searched or edited
- form fields, annotations, comments and hidden layers (OCG) are gone
- JavaScript, attachments, bookmarks and metadata (Info / XMP) are gone

> **Limitation:** the pages are still readable by people and by OCR. Flattening
> prevents editing and direct data extraction; it does not hide what is visible.

Password-protected PDFs ask for their password in the list before they can be
processed.

## Desktop app

Download the installer for your OS from
[Releases](https://github.com/abhuri/flattenPDF/releases):
`.exe` / `.msi` (Windows), `.dmg` (macOS, Intel + Apple Silicon),
`.deb` / `.rpm` / `.AppImage` (Linux).

Pick a tab, drop PDFs onto the window (or click **Add PDFs…**), adjust the
settings, then press the action button. Files that need a password ask for it
inline. Output goes next to the original unless you choose another folder;
existing files are never overwritten (` (2)`, ` (3)`… is appended). Switches in
the header choose light 🌞 or dark 🌛 mode (default: follow the system) and
the TH / EN language.

> The installers are not code-signed yet. On Windows choose *More info → Run
> anyway*; on macOS right-click the app and choose *Open* the first time.

### Watermarks

- **Text** (any font installed on the computer, color, bold, italic, several
  lines) or a **picture** (PNG with transparency, JPG, SVG, …)
- **Size** as a percentage of the page width, **angle** from -90° to 90°,
  **opacity**, and **position**: one of nine spots or repeated across the page
- **Pages**: all, only some (`1-3, 7`), or all except some (`1, 10-`)
- **Flatten afterwards** bakes the watermark into the page images so it cannot
  be removed (uses the Flatten tab's resolution and quality)
- A **live preview** shows the first file with the watermark; ◀ ▶ moves
  between its pages
- **Profiles** save a complete set of settings under a name, pictures
  included. They live in the app's config folder (`watermark.json`)

Text is drawn by the app's web view and stamped as an image, so Thai and other
complex scripts render correctly. The stamp is stored once in the PDF and
reused on every page, so even a tiled watermark on hundreds of pages adds very
little to the file size.

## Usage (CLI)

```bash
flattenpdf flatten input.pdf                         # -> input_flattened.pdf
flattenpdf flatten *.pdf --dpi 300 --quality 90 --gray
flattenpdf flatten locked.pdf --password secret      # encrypted input
flattenpdf images input.pdf --rotate right           # -> input_images/page-001.png …
flattenpdf images input.pdf --zip --pages 1-3,7      # -> input_images.zip
flattenpdf watermark input.pdf --image logo.png --position bottom-right --size 15 --opacity 60
flattenpdf watermark *.pdf --image stamp.png --position tile --angle 45 --except 1 --flatten
```

Text watermarks are made in the desktop app; on the command line, render the
text to a PNG first.

| Command | Option | Default | |
|---|---|---|---|
| all | `-o, --output` | next to input | output path (single input only) |
| all | `--password` | | password for encrypted input |
| all | `--pdfium-dir` | next to exe | where the PDFium library lives |
| flatten | `--dpi` | 200 | render resolution (36–600) |
| flatten | `-q, --quality` | 85 | JPEG quality (1–100) |
| flatten | `--gray` | off | grayscale output, smaller files |
| images | `--dpi` | 200 | render resolution (36–600) |
| images | `--rotate` | none | `none`, `right` (90° clockwise), `left` (90° counter-clockwise), `half` (180°) |
| images | `--zip` | off | write a .zip instead of a folder |
| images | `--pages` | all | e.g. `1-3,7,10-` |
| flatten, images | `--threads` | all cores | encoder threads |
| watermark | `--image` | (required) | the PNG to stamp |
| watermark | `--size` | 50 | width as % of the page width |
| watermark | `--angle` | 0 | counter-clockwise degrees |
| watermark | `--opacity` | 30 | percent (100 = solid) |
| watermark | `--position` | center | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right`, `tile` |
| watermark | `--pages` / `--except` | all | e.g. `1-3,7` / `1,10-` |
| watermark | `--flatten` | off | flatten afterwards (`--dpi` sets the resolution) |

The PDFium library (`pdfium.dll` / `libpdfium.so` / `libpdfium.dylib`) must sit
next to the executable; release archives already include it.

## Performance

98-page A4 document (27 MB input), Windows, 16 threads:

| Task | Time | Output |
|---|---|---|
| Flatten, 150 DPI | 3.2 s | 21 MB |
| Flatten, 200 DPI | 4.5 s | 32 MB |
| Flatten, 300 DPI | 7.4 s | 58 MB |
| To Images (PNG, 200 DPI, zip) | 4.4 s | 102 MB |
| Watermark, tiled on every page | 2.0 s | 26 MB (+0.3 MB) |
| Watermark + flatten, 200 DPI | 7.7 s | 33 MB |

Memory stays bounded by thread count, not page count: pages are rendered,
encoded and streamed to disk one at a time (peak RAM 92 MB when flattening at
200 DPI).

## How it works

```
[PDFium render, 1 thread] --bitmaps--> [JPEG/PNG encoders, N threads] --> [in-order writer, streaming]
```

- `crates/flatten-core` – the shared render pipeline, flatten, images and
  watermark, and a minimal image-only PDF writer. Watermarks use a few PDFium
  calls directly (`raw.rs`) to share one form XObject across all pages.
- `crates/flatten-cli` – the `flattenpdf` command
- `app/` – the Tauri 2 desktop app (`src-tauri/` backend, `ui/` plain HTML/JS front end)

Results are written to `<output>.part` and renamed only on success, so a
failure or cancellation never leaves a half-written file or folder behind.

## Building

Requires Rust (stable) and the prebuilt PDFium from
[bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)
(version must match the `pdfium_*` feature of `pdfium-render`).

```bash
scripts/fetch-pdfium.sh          # downloads into ./pdfium
cargo test --release             # end-to-end tests use ./pdfium
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
`CARGO_TARGET_DIR` to a local path so build output is not synced, and run
`npm install` outside the synced folder.

## Releases

Pushing a tag `v*` builds the desktop installers and the CLI archives for
Windows, Linux and macOS and attaches them to a GitHub Release.

## Roadmap

- [x] Core pipeline + CLI
- [x] Desktop app (Tauri): drag & drop, progress, cancel, passwords, EN/TH
- [x] Installers (.exe / .msi / .dmg / .deb / .rpm / .AppImage)
- [x] Export pages as PNG (folder or zip, rotation, page ranges)
- [x] Watermarks (text or picture, profiles, live preview)
- [ ] Output password
- [ ] Code signing (Windows, macOS notarization)
