"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;
const { revealItemInDir } = window.__TAURI__.opener;
const { getCurrentWebview } = window.__TAURI__.webview;

const STRINGS = {
  en: {
    dropTitle: "Drop PDF files here",
    dropHint: "Every page becomes an image: text can't be selected or edited, forms, layers and metadata are removed.",
    dropMore: "Drop more PDFs here, or",
    addFiles: "Add PDFs…",
    resolution: "Resolution (DPI)",
    dpiSmall: "small",
    dpiBalanced: "balanced",
    dpiSharp: "sharp",
    quality: "JPEG quality",
    grayscale: "Grayscale (smaller files)",
    saveTo: "Save to",
    sameFolderPath: "Same folder as the original",
    change: "Change…",
    sameFolder: "Reset",
    suffix: "File name suffix",
    clear: "Clear list",
    start: "Flatten",
    cancel: "Cancel",
    pages: (n) => `${n} pages`,
    summary: (files, pages) => `${files} file${files === 1 ? "" : "s"} · ${pages} pages`,
    ready: "Ready",
    queued: "Waiting",
    running: (d, t) => `${d} / ${t}`,
    done: (s) => `Done · ${s}`,
    failed: "Failed",
    locked: "Password needed",
    cancelled: "Cancelled",
    invalid: "Not a valid PDF",
    showInFolder: "Show in folder",
    passwordPlaceholder: "Password",
    unlock: "Use",
    wrongPassword: "Wrong password",
    remove: "Remove",
    notPdf: (n) => `Skipped ${n} non-PDF file${n === 1 ? "" : "s"}`,
    allDone: (n) => `Finished ${n} file${n === 1 ? "" : "s"}`,
    nothingToDo: "Nothing to flatten",
  },
  th: {
    dropTitle: "ลากไฟล์ PDF มาวางที่นี่",
    dropHint: "ทุกหน้าจะถูกแปลงเป็นภาพ: เลือกหรือแก้ไขข้อความไม่ได้ และฟอร์ม เลเยอร์ และ metadata จะถูกลบออก",
    dropMore: "ลากไฟล์ PDF มาเพิ่มได้ หรือ",
    addFiles: "เลือกไฟล์ PDF…",
    resolution: "ความละเอียด (DPI)",
    dpiSmall: "ไฟล์เล็ก",
    dpiBalanced: "สมดุล",
    dpiSharp: "คมชัด",
    quality: "คุณภาพ JPEG",
    grayscale: "ขาวดำ (ไฟล์เล็กลง)",
    saveTo: "บันทึกที่",
    sameFolderPath: "โฟลเดอร์เดียวกับไฟล์ต้นฉบับ",
    change: "เปลี่ยน…",
    sameFolder: "ค่าเดิม",
    suffix: "ต่อท้ายชื่อไฟล์",
    clear: "ล้างรายการ",
    start: "แปลงไฟล์",
    cancel: "ยกเลิก",
    pages: (n) => `${n} หน้า`,
    summary: (files, pages) => `${files} ไฟล์ · ${pages} หน้า`,
    ready: "พร้อม",
    queued: "รอคิว",
    running: (d, t) => `${d} / ${t}`,
    done: (s) => `เสร็จ · ${s}`,
    failed: "ไม่สำเร็จ",
    locked: "ต้องใส่รหัสผ่าน",
    cancelled: "ยกเลิกแล้ว",
    invalid: "ไม่ใช่ไฟล์ PDF ที่ถูกต้อง",
    showInFolder: "เปิดโฟลเดอร์",
    passwordPlaceholder: "รหัสผ่าน",
    unlock: "ใช้",
    wrongPassword: "รหัสผ่านไม่ถูกต้อง",
    remove: "ลบออก",
    notPdf: (n) => `ข้ามไฟล์ที่ไม่ใช่ PDF ${n} ไฟล์`,
    allDone: (n) => `แปลงเสร็จ ${n} ไฟล์`,
    nothingToDo: "ไม่มีไฟล์ให้แปลง",
  },
};

const DEFAULT_SETTINGS = { dpi: 200, quality: 85, grayscale: false, outputDir: null, suffix: "_flattened" };

const store = {
  get(key, fallback) {
    try {
      const raw = localStorage.getItem(key);
      return raw === null ? fallback : JSON.parse(raw);
    } catch {
      return fallback;
    }
  },
  set(key, value) {
    try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* ignore */ }
  },
};

const state = {
  lang: store.get("lang", navigator.language.startsWith("th") ? "th" : "en"),
  settings: { ...DEFAULT_SETTINGS, ...store.get("settings", {}) },
  files: [],
  nextId: 1,
  running: false,
};

const $ = (id) => document.getElementById(id);
const t = (key, ...args) => {
  const value = STRINGS[state.lang][key];
  return typeof value === "function" ? value(...args) : value;
};

function formatBytes(bytes) {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  Object.assign(node, props);
  for (const child of children) if (child != null) node.append(child);
  return node;
}

// ---------- files ----------

async function addPaths(paths) {
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  const known = new Set(state.files.map((f) => f.path));
  const fresh = pdfs.filter((p) => !known.has(p));
  if (pdfs.length < paths.length) toast(t("notPdf", paths.length - pdfs.length));
  if (!fresh.length) return;

  let infos;
  try {
    infos = await invoke("inspect", { paths: fresh });
  } catch (err) {
    toast(String(err));
    return;
  }
  for (const info of infos) {
    state.files.push({
      id: state.nextId++,
      ...info,
      status: info.error ? "invalid" : info.encrypted ? "locked" : "ready",
      password: "",
      done: 0,
      total: info.pages ?? 0,
    });
  }
  render();
}

function runnable(file) {
  return ["ready", "failed", "cancelled"].includes(file.status) && !(file.encrypted && !file.password);
}

async function start() {
  const files = state.files.filter(runnable);
  if (!files.length) {
    toast(t("nothingToDo"));
    return;
  }
  for (const f of files) {
    f.status = "queued";
    f.done = 0;
  }
  state.running = true;
  render();
  try {
    await invoke("start", {
      jobs: files.map((f) => ({ id: f.id, input: f.path, password: f.password || null })),
      settings: state.settings,
    });
  } catch (err) {
    state.running = false;
    for (const f of files) f.status = "ready";
    toast(String(err));
    render();
  }
}

function onJobEvent(event) {
  if (event.kind === "batchFinished") {
    state.running = false;
    const finished = state.files.filter((f) => f.status === "done" && f.justFinished);
    if (finished.length) toast(t("allDone", finished.length));
    for (const f of state.files) f.justFinished = false;
    render();
    return;
  }
  const file = state.files.find((f) => f.id === event.id);
  if (!file) return;
  switch (event.kind) {
    case "started":
      file.status = "running";
      file.total = event.totalPages;
      break;
    case "progress":
      file.done = event.done;
      file.total = event.totalPages;
      break;
    case "finished":
      Object.assign(file, { status: "done", output: event.output, outBytes: event.bytes, seconds: event.seconds, justFinished: true });
      break;
    case "failed":
      if (event.passwordRequired) {
        Object.assign(file, { status: "locked", encrypted: true, password: "", passwordError: true });
      } else {
        Object.assign(file, { status: "failed", message: event.message });
      }
      break;
    case "cancelled":
      file.status = "cancelled";
      break;
  }
  render();
}

// ---------- rendering ----------

function statusText(file) {
  switch (file.status) {
    case "running": return t("running", file.done, file.total);
    case "done": return t("done", formatBytes(file.outBytes));
    default: return t(file.status);
  }
}

function fileRow(file) {
  const meta = el("div", { className: "file-meta" });
  if (file.pages != null) meta.append(el("span", { textContent: t("pages", file.pages) }));
  meta.append(el("span", { textContent: formatBytes(file.sizeBytes) }));
  if (file.status === "done") {
    meta.append(el("button", {
      className: "link",
      type: "button",
      textContent: t("showInFolder"),
      onclick: () => revealItemInDir(file.output),
    }));
  }
  if (file.status === "failed" && file.message) meta.append(el("span", { textContent: file.message }));

  const side = el("div", { className: "file-side" },
    el("span", { className: `status ${file.status}`, textContent: statusText(file), title: file.message || "" }));
  if (!state.running) {
    side.append(el("button", {
      className: "icon",
      type: "button",
      textContent: "×",
      title: t("remove"),
      ariaLabel: t("remove"),
      onclick: () => {
        state.files = state.files.filter((f) => f !== file);
        render();
      },
    }));
  }

  const li = el("li", {}, el("div", { className: "file-name", textContent: file.name, title: file.path }), side, meta);

  if (file.status === "locked" && !state.running) {
    const input = el("input", { type: "password", placeholder: file.passwordError ? t("wrongPassword") : t("passwordPlaceholder") });
    const apply = () => {
      if (!input.value) return;
      file.password = input.value;
      file.passwordError = false;
      file.status = "ready";
      render();
    };
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") apply(); });
    li.append(el("div", { className: "password" }, input,
      el("button", { className: "secondary small", type: "button", textContent: t("unlock"), onclick: apply })));
  }

  if (file.status === "running" || file.status === "queued") {
    const fill = el("div");
    fill.style.width = file.total ? `${(100 * file.done) / file.total}%` : "0";
    li.append(el("div", { className: "bar" }, fill));
  }
  return li;
}

function render() {
  document.documentElement.lang = state.lang;
  for (const node of document.querySelectorAll("[data-i18n]")) node.textContent = t(node.dataset.i18n);
  $("lang").textContent = state.lang === "th" ? "English" : "ไทย";

  const list = $("files");
  list.replaceChildren(...state.files.map(fileRow));
  list.hidden = state.files.length === 0;
  $("drop").classList.toggle("compact", state.files.length > 0);

  const s = state.settings;
  for (const btn of $("dpi").children) btn.classList.toggle("active", Number(btn.dataset.value) === s.dpi);
  $("quality").value = s.quality;
  $("quality-out").textContent = s.quality;
  $("gray").checked = s.grayscale;
  $("suffix").value = s.suffix;
  $("outdir").textContent = s.outputDir || t("sameFolderPath");
  $("outdir").title = s.outputDir || "";
  $("reset-dir").hidden = !s.outputDir;

  const pages = state.files.reduce((sum, f) => sum + (f.pages || 0), 0);
  $("summary").textContent = state.files.length ? t("summary", state.files.length, pages) : "";

  const busy = state.running;
  for (const id of ["add", "clear", "choose-dir", "reset-dir", "quality", "gray", "suffix"]) $(id).disabled = busy;
  for (const btn of $("dpi").children) btn.disabled = busy;
  $("start").hidden = busy;
  $("start").disabled = !state.files.some(runnable);
  $("cancel").hidden = !busy;
}

let toastTimer;
function toast(message) {
  const node = $("toast");
  node.textContent = message;
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { node.hidden = true; }, 3500);
}

function saveSettings() {
  store.set("settings", state.settings);
  render();
}

// ---------- wiring ----------

$("lang").onclick = () => {
  state.lang = state.lang === "th" ? "en" : "th";
  store.set("lang", state.lang);
  render();
};

$("add").onclick = async () => {
  const picked = await open({ multiple: true, filters: [{ name: "PDF", extensions: ["pdf", "PDF"] }] });
  if (picked) await addPaths(Array.isArray(picked) ? picked : [picked]);
};

for (const btn of $("dpi").children) {
  btn.onclick = () => { state.settings.dpi = Number(btn.dataset.value); saveSettings(); };
}
$("quality").oninput = (e) => { state.settings.quality = Number(e.target.value); saveSettings(); };
$("gray").onchange = (e) => { state.settings.grayscale = e.target.checked; saveSettings(); };
$("suffix").onchange = (e) => {
  // Keep the suffix safe for file names on every OS.
  state.settings.suffix = e.target.value.replace(/[\\/:*?"<>|]/g, "").trim() || DEFAULT_SETTINGS.suffix;
  saveSettings();
};
$("choose-dir").onclick = async () => {
  const dir = await open({ directory: true });
  if (dir) { state.settings.outputDir = dir; saveSettings(); }
};
$("reset-dir").onclick = () => { state.settings.outputDir = null; saveSettings(); };

$("clear").onclick = () => { state.files = []; render(); };
$("start").onclick = start;
$("cancel").onclick = () => invoke("cancel");

listen("job-event", (e) => onJobEvent(e.payload));

getCurrentWebview().onDragDropEvent((e) => {
  const drop = $("drop");
  if (e.payload.type === "enter" || e.payload.type === "over") {
    if (!state.running) drop.classList.add("over");
  } else {
    drop.classList.remove("over");
    if (e.payload.type === "drop" && !state.running) addPaths(e.payload.paths);
  }
});

render();
