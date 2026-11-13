"use strict";

// Combine tab: join PDFs and pictures, in the order of the list, into one PDF.
// Loaded after main.js and uses its globals (state, t, $, el, invoke, toast...).
//
// Unlike the other tabs this is one job for the whole list, so it has its own
// result card instead of a status on every file.

Object.assign(STRINGS.en, {
  tabCombine: "Combine",
  hintCombine: "Joins PDFs and pictures (JPG, PNG, GIF, WebP, BMP, TIFF) into one PDF, in the order of the list.",
  dropTitleCombine: "Drop PDFs or pictures here",
  dropMoreCombine: "Drop more files here, or",
  addFilesCombine: "Add files…",
  supportedFiles: "PDFs and pictures",
  notSupported: (n) => `Skipped ${n} unsupported file${n === 1 ? "" : "s"}`,
  startCombine: "Combine into PDF",
  outputName: "Output file name",
  imagePage: "Page size for pictures",
  pageFit: "Picture size",
  order: "Order",
  sortByName: "A→Z by name",
  reverseOrder: "⇅ Reverse",
  flattenCombined: "Flatten the result",
  flattenCombinedNote: "Every page becomes an image, like the Flatten tab (uses its resolution, quality and grayscale settings).",
  dragToReorder: "Drag to reorder",
  moveUp: "Move up",
  moveDown: "Move down",
  picture: "Picture",
  combining: (d, t) => (t ? `Combining… ${d} / ${t}` : "Combining…"),
  combined: (pages, size) => `Done · ${pages} pages · ${size}`,
  combinedToast: (name) => `Saved ${name}`,
  fixInvalid: "Remove the files that can't be opened first",
  fixLocked: "Enter the passwords of the locked files first",
});

Object.assign(STRINGS.th, {
  tabCombine: "รวมไฟล์",
  hintCombine: "รวมไฟล์ PDF และรูปภาพ (JPG, PNG, GIF, WebP, BMP, TIFF) เป็น PDF ไฟล์เดียว ตามลำดับในรายการ",
  dropTitleCombine: "ลากไฟล์ PDF หรือรูปภาพมาวางที่นี่",
  dropMoreCombine: "ลากไฟล์มาเพิ่มได้ หรือ",
  addFilesCombine: "เลือกไฟล์…",
  supportedFiles: "PDF และรูปภาพ",
  notSupported: (n) => `ข้ามไฟล์ที่ไม่รองรับ ${n} ไฟล์`,
  startCombine: "รวมเป็น PDF",
  outputName: "ชื่อไฟล์ผลลัพธ์",
  imagePage: "ขนาดหน้าสำหรับรูปภาพ",
  pageFit: "เท่าขนาดรูป",
  order: "ลำดับ",
  sortByName: "เรียงตามชื่อ A→Z",
  reverseOrder: "⇅ กลับลำดับ",
  flattenCombined: "Flatten ไฟล์ที่รวมแล้ว",
  flattenCombinedNote: "แปลงทุกหน้าเป็นภาพเหมือนแท็บ Flatten (ใช้ความละเอียด คุณภาพ และโหมดขาวดำจากแท็บ Flatten)",
  dragToReorder: "ลากเพื่อจัดลำดับ",
  moveUp: "เลื่อนขึ้น",
  moveDown: "เลื่อนลง",
  picture: "รูปภาพ",
  combining: (d, t) => (t ? `กำลังรวม… ${d} / ${t}` : "กำลังรวม…"),
  combined: (pages, size) => `เสร็จ · ${pages} หน้า · ${size}`,
  combinedToast: (name) => `บันทึก ${name} แล้ว`,
  fixInvalid: "ลบไฟล์ที่เปิดไม่ได้ออกจากรายการก่อน",
  fixLocked: "ใส่รหัสผ่านของไฟล์ที่ล็อกไว้ก่อน",
});

const CB_IMAGE_EXTENSIONS = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff"];

const cb = {
  /** The current or last run: { id, status, done, total, output, bytes, pages, message }. */
  job: null,
};

const cbFiles = () => state.files.combine;
const baseName = (path) => path.split(/[\\/]/).pop();
const stemOf = (path) => baseName(path).replace(/\.[^.]*$/, "");

/** The name used when the box is left empty (the backend does the same). */
function defaultName() {
  const first = cbFiles()[0];
  return first ? `${stemOf(first.path)}_combined` : "combined";
}

// ---------- ordering ----------

function setOrder(files) {
  state.files.combine = files;
  cb.job = null; // an old result no longer matches the list
  render();
}

function moveFile(index, delta) {
  const files = [...cbFiles()];
  const to = index + delta;
  if (to < 0 || to >= files.length) return;
  [files[index], files[to]] = [files[to], files[index]];
  setOrder(files);
}

/**
 * Drag a row by its handle. Uses pointer events on the document, because
 * the webview's file drop handling disables HTML drag and drop on Windows.
 */
function startDrag(event, li) {
  if (state.running || event.button !== 0) return;
  event.preventDefault();
  const list = $("files");
  const scroller = document.querySelector("main");
  li.classList.add("dragging");
  document.body.classList.add("reordering");

  const move = (e) => {
    const rows = [...list.children].filter((row) => row !== li);
    const before = rows.find((row) => {
      const box = row.getBoundingClientRect();
      return e.clientY < box.top + box.height / 2;
    });
    if (before) {
      if (li.nextElementSibling !== before) list.insertBefore(li, before);
    } else if (list.lastElementChild !== li) {
      list.append(li);
    }
    // Scroll when dragging near the top or bottom edge.
    const box = scroller.getBoundingClientRect();
    if (e.clientY < box.top + 40) scroller.scrollBy(0, -12);
    else if (e.clientY > box.bottom - 40) scroller.scrollBy(0, 12);
  };
  const end = () => {
    document.removeEventListener("pointermove", move);
    document.removeEventListener("pointerup", end);
    document.removeEventListener("pointercancel", end);
    li.classList.remove("dragging");
    document.body.classList.remove("reordering");
    const byId = new Map(cbFiles().map((f) => [f.id, f]));
    const order = [...list.children].map((row) => byId.get(Number(row.dataset.id))).filter(Boolean);
    const changed = order.some((f, i) => f !== cbFiles()[i]);
    if (changed) setOrder(order);
    else render();
  };
  document.addEventListener("pointermove", move);
  document.addEventListener("pointerup", end);
  document.addEventListener("pointercancel", end);
}

function decorateRow(li, file, index) {
  const files = cbFiles();
  const busy = state.running;
  li.dataset.id = file.id;
  li.classList.add("orderable");

  const handle = el("div", {
    className: "handle",
    title: t("dragToReorder"),
    ariaLabel: t("dragToReorder"),
  }, el("span", { className: "grip", textContent: "⠿", ariaHidden: "true" }),
  el("span", { className: "order-no", textContent: String(index + 1) }));
  if (!busy) handle.addEventListener("pointerdown", (e) => startDrag(e, li));
  li.prepend(handle);

  // The file type, and no per-file status: the result card shows progress.
  const meta = li.querySelector(".file-meta");
  const ext = file.path.split(".").pop().toUpperCase();
  meta.prepend(el("span", { className: `tag kind ${file.kind}`, textContent: file.kind === "image" ? `${t("picture")} ${ext}` : "PDF" }));
  if (file.kind === "image") {
    // "1 pages" says nothing for a picture.
    for (const span of meta.querySelectorAll("span")) if (span.textContent === t("pages", 1)) span.remove();
  }
  const status = li.querySelector(".file-side .status");
  if (file.status === "ready") status.remove();

  const side = li.querySelector(".file-side");
  side.prepend(
    el("button", {
      className: "icon move", type: "button", textContent: "▲", title: t("moveUp"), ariaLabel: t("moveUp"),
      disabled: busy || index === 0, onclick: () => moveFile(index, -1),
    }),
    el("button", {
      className: "icon move", type: "button", textContent: "▼", title: t("moveDown"), ariaLabel: t("moveDown"),
      disabled: busy || index === files.length - 1, onclick: () => moveFile(index, 1),
    }),
  );
}

// ---------- running ----------

function problem() {
  const files = cbFiles();
  if (!files.length) return t("nothingToDo");
  if (files.some((f) => f.status === "invalid")) return t("fixInvalid");
  if (files.some((f) => f.status === "locked")) return t("fixLocked");
  return null;
}

async function startCombine() {
  const issue = problem();
  if (issue) {
    toast(issue);
    return;
  }
  const s = state.settings;
  cb.job = { id: state.nextId++, status: "queued", done: 0, total: 0 };
  state.running = true;
  render();
  try {
    await invoke("start_combine", {
      id: cb.job.id,
      files: cbFiles().map((f) => ({ id: f.id, input: f.path, password: f.password || null })),
      settings: {
        name: s.combine.name,
        pageSize: s.combine.pageSize,
        flatten: s.combine.flatten,
        dpi: s.flatten.dpi,
        quality: s.flatten.quality,
        grayscale: s.flatten.grayscale,
      },
      outputDir: s.outputDir,
    });
  } catch (err) {
    state.running = false;
    cb.job = null;
    toast(String(err));
    render();
  }
}

function onCombineEvent(event) {
  const job = cb.job;
  if (!job || event.id !== job.id) return false;
  switch (event.kind) {
    case "started":
      Object.assign(job, { status: "running", total: event.totalPages });
      break;
    case "progress":
      Object.assign(job, { status: "running", done: event.done, total: event.totalPages });
      break;
    case "finished":
      Object.assign(job, { status: "done", output: event.output, bytes: event.bytes, pages: event.pages });
      toast(t("combinedToast", baseName(event.output)));
      break;
    case "failed": {
      Object.assign(job, { status: "failed", message: event.message });
      const file = event.file && cbFiles().find((f) => f.path === event.file);
      if (file && event.passwordRequired) {
        Object.assign(file, { status: "locked", password: "", passwordError: true });
      }
      break;
    }
    case "cancelled":
      job.status = "cancelled";
      break;
  }
  return true;
}

// ---------- rendering ----------

function renderCombine() {
  const s = state.settings.combine;
  const busy = state.running;

  // Texts that differ from the PDF-only tabs.
  document.querySelector(".drop-title").textContent = t("dropTitleCombine");
  document.querySelector(".drop-more").textContent = t("dropMoreCombine");
  $("add").textContent = t("addFilesCombine");

  const name = $("cb-name");
  if (document.activeElement !== name) name.value = s.name;
  name.placeholder = defaultName();
  $("cb-flatten").checked = s.flatten;
  for (const id of ["cb-name", "cb-flatten"]) $(id).disabled = busy;
  for (const id of ["cb-sort", "cb-reverse"]) $(id).disabled = busy || cbFiles().length < 2;

  if (!cbFiles().length && !busy) cb.job = null; // the list was cleared
  const job = cb.job;
  $("cb-result").hidden = !job;
  if (!job) return;
  const status = $("cb-result-status");
  status.className = `status ${job.status}`;
  status.textContent = {
    queued: t("combining", 0, 0),
    running: t("combining", job.done, job.total),
    done: job.status === "done" ? t("combined", job.pages, formatBytes(job.bytes ?? 0)) : "",
    failed: t("failed"),
    cancelled: t("cancelled"),
  }[job.status];
  $("cb-result-name").textContent = job.output ? baseName(job.output) : "";
  $("cb-result-name").title = job.output || "";
  $("cb-result-show").hidden = job.status !== "done";
  const active = job.status === "queued" || job.status === "running";
  const bar = $("cb-result-bar");
  bar.hidden = !active;
  bar.classList.toggle("indeterminate", active && !job.total);
  bar.firstElementChild.style.width = !job.total ? "40%" : `${(100 * job.done) / job.total}%`;
  $("cb-result-error").hidden = job.status !== "failed";
  $("cb-result-error").textContent = job.message || "";
}

// ---------- wiring ----------

$("cb-name").onchange = (e) => {
  // Same rules as the backend: no characters that are invalid in file names.
  state.settings.combine.name = e.target.value.replace(/[\\/:*?"<>|]/g, "").replace(/\.pdf$/i, "").trim();
  saveSettings();
};
$("cb-flatten").onchange = (e) => {
  state.settings.combine.flatten = e.target.checked;
  saveSettings();
};
$("cb-sort").onclick = () => {
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  setOrder([...cbFiles()].sort((a, b) => collator.compare(a.name, b.name)));
};
$("cb-reverse").onclick = () => setOrder([...cbFiles()].reverse());
$("cb-result-show").onclick = () => cb.job?.output && revealItemInDir(cb.job.output);

tabHooks.combine = {
  extensions: ["pdf", ...CB_IMAGE_EXTENSIONS],
  render: renderCombine,
  validate: problem,
  start: startCombine,
  canStart: () => !state.running && cbFiles().length > 0,
  onJobEvent: onCombineEvent,
  decorateRow,
};

render();
