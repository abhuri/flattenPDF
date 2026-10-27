# FlattenPDF

Offline PDF tools for Windows, Linux and macOS, in one app with three tabs:

| Tab | What it does | Output |
|---|---|---|
| 🔒 **Flatten** | Converts every page into an image and writes a new PDF that contains only those images | `<name>_flattened.pdf` |
| 🔓 **Unlock** | Removes the open password and print/copy/edit restrictions; text, forms and bookmarks are kept | `<name>_unlocked.pdf` |
| 🖼️ **To Images** | Saves pages as PNG images, optionally rotated, into a folder or a single .zip | `<name>_images/page-001.png …` or `<name>_images.zip` |

A flattened PDF looks the same, but:

- text cannot be selected, copied, searched or edited
- form fields, annotations, comments and hidden layers (OCG) are gone
- JavaScript, attachments, bookmarks and metadata (Info / XMP) are gone

> **Limitation:** the pages are still readable by people and by OCR. Flattening
> prevents editing and direct data extraction; it does not hide what is visible.

> **Unlock** removes owner restrictions without asking for a password. A file
> that needs a password to *open* still needs that password. Use it on
> documents you are entitled to use.

## Desktop app

Download the installer for your OS from
[Releases](https://github.com/abhuri/flattenPDF/releases):
`.exe` / `.msi` (Windows), `.dmg` (macOS, Intel + Apple Silicon),
`.deb` / `.rpm` / `.AppImage` (Linux).

Pick a tab, drop PDFs onto the window (or click **Add PDFs…**), adjust the
settings, then press the action button. Files that need a password ask for it
inline. Output goes next to the original unless you choose another folder;
existing files are never overwritten (` (2)`, ` (3)`… is appended). The UI is
available in English and Thai.

> The installers are not code-signed yet. On Windows choose *More info → Run
> anyway*; on macOS right-click the app and choose *Open* the first time.

## Usage (CLI)

```bash
flattenpdf flatten input.pdf                         # -> input_flattened.pdf
flattenpdf flatten *.pdf --dpi 300 --quality 90 --gray
flattenpdf unlock locked.pdf --password secret       # -> locked_unlocked.pdf
flattenpdf unlock restricted.pdf                     # owner restrictions: no password needed
flattenpdf images input.pdf --rotate right           # -> input_images/page-001.png …
flattenpdf images input.pdf --zip --pages 1-3,7      # -> input_images.zip
```

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

The PDFium library (`pdfium.dll` / `libpdfium.so` / `libpdfium.dylib`) must sit
next to the executable; release archives already include it.

## Performance

98-page A4 document (27 MB input), Windows, 16 threads:

| Task | Time | Output |
|---|---|---|
| Flatten, 150 DPI | 3.2 s | 21 MB |
| Flatten, 200 DPI | 4.5 s | 32 MB |
| Flatten, 300 DPI | 7.4 s | 58 MB |
| Unlock | 0.7 s | 26 MB |
| To Images (PNG, 200 DPI, zip) | 4.4 s | 102 MB |

Memory stays bounded by thread count, not page count: pages are rendered,
encoded and streamed to disk one at a time (peak RAM 92 MB when flattening at
200 DPI).

## How it works

```
[PDFium render, 1 thread] --bitmaps--> [JPEG/PNG encoders, N threads] --> [in-order writer, streaming]
```

- `crates/flatten-core` – the shared render pipeline, flatten / unlock / images,
  and a minimal image-only PDF writer. Unlock saves through PDFium with
  `FPDF_REMOVE_SECURITY`.
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

The encrypted test fixtures in `crates/flatten-core/tests/fixtures` are
generated by `make_fixtures.py` there (needs `pypdf` and `cryptography`).

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
- [x] Unlock (remove passwords and restrictions)
- [x] Export pages as PNG (folder or zip, rotation, page ranges)
- [ ] Watermark, output password
- [ ] Code signing (Windows, macOS notarization)
