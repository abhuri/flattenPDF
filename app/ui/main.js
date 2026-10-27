"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;
const { revealItemInDir } = window.__TAURI__.opener;
const { getCurrentWebview } = window.__TAURI__.webview;

const STRINGS = {
  en: {
    tabFlatten: "Flatten",
    tabImages: "To Images",
    dropTitle: "Drop PDF files here",
    hintFlatten: "Every page becomes an image: text can't be selected or edited, forms, layers and metadata are removed.",
    hintImages: "Saves every page as a PNG image, into a folder next to the PDF or a single .zip file.",
    dropMore: "Drop more PDFs here, or",
    addFiles: "Add PDFs…",
    resolution: "Resolution (DPI)",
    dpiSmall: "small",
    dpiBalanced: "balanced",
    dpiSharp: "sharp",
    quality: "JPEG quality",
    grayscale: "Grayscale (smaller files)",
    suffix: "File name suffix",
    rotate: "Rotate images",
    rotNone: "None",
    rotLeft: "Left",
    rotRight: "Right",
    packaging: "Save as",
    packFolder: "Folder",
    pageRange: "Pages",
    pageRangeHint: "All pages, or e.g. 1-3, 7, 10-",
    saveTo: "Save to",
    sameFolderPath: "Same folder as the original",
    change: "Change…",
    sameFolder: "Reset",
    clear: "Clear list",
    startFlatten: "Flatten",
    startImages: "Export images",
    cancel: "Cancel",
    pages: (n) => `${n} pages`,
    summary: (files, pages) => `${files} file${files === 1 ? "" : "s"} · ${pages} pages`,
    ready: "Ready",
    queued: "Waiting",
    running: (d, t) => (t ? `${d} / ${t}` : "Working…"),
    done: (s) => `Done · ${s}`,
    failed: "Failed",
    locked: "Password needed",
    cancelled: "Cancelled",
    invalid: "Not a valid PDF",
    tagLocked: "Password",
    darkMode: "Dark mode",
    language: "Language: TH / EN",
    showInFolder: "Show in folder",
    passwordPlaceholder: "Password",
    unlock: "Use",
    wrongPassword: "Wrong password",
    remove: "Remove",
    notPdf: (n) => `Skipped ${n} non-PDF file${n === 1 ? "" : "s"}`,
    allDone: (n) => `Finished ${n} file${n === 1 ? "" : "s"}`,
    nothingToDo: "Nothing to do",
  },
  th: {
    tabFlatten: "Flatten",
    tabImages: "แปลงเป็นภาพ",
    dropTitle: "ลากไฟล์ PDF มาวางที่นี่",
    hintFlatten: "ทุกหน้าจะถูกแปลงเป็นภาพ: เลือกหรือแก้ไขข้อความไม่ได้ และฟอร์ม เลเยอร์ และ metadata จะถูกลบออก",
    hintImages: "บันทึกทุกหน้าเป็นภาพ PNG ลงในโฟลเดอร์ข้างไฟล์ PDF หรือรวมเป็นไฟล์ .zip ไฟล์เดียว",
    dropMore: "ลากไฟล์ PDF มาเพิ่มได้ หรือ",
    addFiles: "เลือกไฟล์ PDF…",
    resolution: "ความละเอียด (DPI)",
    dpiSmall: "ไฟล์เล็ก",
    dpiBalanced: "สมดุล",
    dpiSharp: "คมชัด",
    quality: "คุณภาพ JPEG",
    grayscale: "ขาวดำ (ไฟล์เล็กลง)",
    suffix: "ต่อท้ายชื่อไฟล์",
    rotate: "หมุนภาพ",
    rotNone: "ไม่หมุน",
    rotLeft: "ซ้าย",
    rotRight: "ขวา",
    packaging: "บันทึกเป็น",
    packFolder: "โฟลเดอร์",
    pageRange: "หน้า",
    pageRangeHint: "ทุกหน้า หรือระบุ เช่น 1-3, 7, 10-",
    saveTo: "บันทึกที่",
    sameFolderPath: "โฟลเดอร์เดียวกับไฟล์ต้นฉบับ",
    change: "เปลี่ยน…",
    sameFolder: "ค่าเดิม",
    clear: "ล้างรายการ",
    startFlatten: "แปลงไฟล์",
    startImages: "แปลงเป็นภาพ",
    cancel: "ยกเลิก",
    pages: (n) => `${n} หน้า`,
    summary: (files, pages) => `${files} ไฟล์ · ${pages} หน้า`,
    ready: "พร้อม",
    queued: "รอคิว",
    running: (d, t) => (t ? `${d} / ${t}` : "กำลังทำงาน…"),
    done: (s) => `เสร็จ · ${s}`,
    failed: "ไม่สำเร็จ",
    locked: "ต้องใส่รหัสผ่าน",
    cancelled: "ยกเลิกแล้ว",
    invalid: "ไม่ใช่ไฟล์ PDF ที่ถูกต้อง",
    tagLocked: "มีรหัสผ่าน",
    darkMode: "โหมดมืด",
    language: "ภาษา: TH / EN",
    showInFolder: "เปิดโฟลเดอร์",
    passwordPlaceholder: "รหัสผ่าน",
    unlock: "ใช้",
    wrongPassword: "รหัสผ่านไม่ถูกต้อง",
    remove: "ลบออก",
    notPdf: (n) => `ข้ามไฟล์ที่ไม่ใช่ PDF ${n} ไฟล์`,
    allDone: (n) => `ทำเสร็จ ${n} ไฟล์`,
    nothingToDo: "ไม่มีไฟล์ให้ทำงาน",
  },
};

const TABS = ["flatten", "images"];
const START_LABEL = { flatten: "startFlatten", images: "startImages" };
const HINT = { flatten: "hintFlatten", images: "hintImages" };

const DEFAULT_SETTINGS = {
  outputDir: null,
  flatten: { dpi: 200, quality: 85, grayscale: false, suffix: "_flattened" },
  images: { dpi: 200, rotation: "none", packaging: "folder", pages: "" },
};

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

function loadSettings() {
  const saved = store.get("settings.v2", {});
  const settings = structuredClone(DEFAULT_SETTINGS);
  settings.outputDir = saved.outputDir ?? null;
  for (const tab of TABS) Object.assign(settings[tab], saved[tab] || {});
  return settings;
}

const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

const state = {
  lang: store.get("lang", navigator.language.startsWith("th") ? "th" : "en"),
  /** "light" or "dark" once chosen with the switch; null follows the system. */
  theme: ["light", "dark"].includes(store.get("theme", null)) ? store.get("theme", null) : null,
  tab: TABS.includes(store.get("tab", "flatten")) ? store.get("tab", "flatten") : "flatten",
  settings: loadSettings(),
  files: { flatten: [], images: [] },
  nextId: 1,
  running: false,
};

const $ = (id) => document.getElementById(id);
const t = (key, ...args) => {
  const value = STRINGS[state.lang][key];
  return typeof value === "function" ? value(...args) : value;
};
const currentFiles = () => state.files[state.tab];
const currentTheme = () => state.theme ?? (systemDark.matches ? "dark" : "light");

/** Shows `value` as the active option of a two-option `.toggle` switch. */
function setToggle(toggle, value, label) {
  const options = [...toggle.querySelectorAll(".opt")];
  for (const opt of options) opt.classList.toggle("active", opt.dataset.opt === value);
  toggle.dataset.on = options[1].dataset.opt === value ? "right" : "left";
  toggle.setAttribute("aria-checked", String(toggle.dataset.on === "right"));
  toggle.setAttribute("aria-label", label);
  toggle.title = label;
}

function applyTheme() {
  const theme = currentTheme();
  document.documentElement.dataset.theme = theme;
  setToggle($("theme-toggle"), theme, t("darkMode"));
}

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

function initialStatus(info) {
  if (info.error) return "invalid";
  if (info.locked) return "locked";
  return "ready";
}

async function addPaths(paths) {
  const tab = state.tab;
  const list = state.files[tab];
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  const known = new Set(list.map((f) => f.path));
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
    list.push({
      id: state.nextId++,
      ...info,
      status: initialStatus(info),
      password: "",
      done: 0,
      total: info.pages ?? 0,
    });
  }
  render();
}

function runnable(file) {
  return ["ready", "failed", "cancelled"].includes(file.status);
}

function taskFor(tab) {
  const s = state.settings[tab];
  return { mode: tab, ...s };
}

async function start() {
  const files = currentFiles().filter(runnable);
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
      task: taskFor(state.tab),
      outputDir: state.settings.outputDir,
    });
  } catch (err) {
    state.running = false;
    for (const f of files) f.status = "ready";
    toast(String(err));
    render();
  }
}

function findFile(id) {
  for (const tab of TABS) {
    const file = state.files[tab].find((f) => f.id === id);
    if (file) return file;
  }
  return null;
}

function onJobEvent(event) {
  if (event.kind === "batchFinished") {
    state.running = false;
    const finished = TABS.flatMap((tab) => state.files[tab]).filter((f) => f.justFinished);
    if (finished.length) toast(t("allDone", finished.length));
    for (const f of finished) f.justFinished = false;
    render();
    return;
  }
  const file = findFile(event.id);
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
      Object.assign(file, { status: "done", output: event.output, outBytes: event.bytes, justFinished: true });
      break;
    case "failed":
      if (event.passwordRequired) {
        Object.assign(file, { status: "locked", password: "", passwordError: true });
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
  if (file.locked) meta.append(el("span", { className: "tag", textContent: t("tagLocked") }));
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
  if (file.status === "failed" && file.message) meta.append(el("span", { className: "error", textContent: file.message }));

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
        state.files[state.tab] = currentFiles().filter((f) => f !== file);
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
    const indeterminate = file.status === "running" && !file.total;
    fill.style.width = indeterminate ? "40%" : file.total ? `${(100 * file.done) / file.total}%` : "0";
    li.append(el("div", { className: `bar${indeterminate ? " indeterminate" : ""}` }, fill));
  }
  return li;
}

function render() {
  document.documentElement.lang = state.lang;
  for (const node of document.querySelectorAll("[data-i18n]")) node.textContent = t(node.dataset.i18n);
  for (const node of document.querySelectorAll("[data-i18n-placeholder]")) node.placeholder = t(node.dataset.i18nPlaceholder);
  setToggle($("lang-toggle"), state.lang, t("language"));
  applyTheme();
  $("drop-hint").textContent = t(HINT[state.tab]);
  $("start").textContent = t(START_LABEL[state.tab]);

  for (const btn of $("tabs").children) {
    const active = btn.dataset.tab === state.tab;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-selected", String(active));
    btn.disabled = state.running && !active;
  }
  for (const panel of document.querySelectorAll("[data-panel]")) panel.hidden = panel.dataset.panel !== state.tab;

  const files = currentFiles();
  const list = $("files");
  list.replaceChildren(...files.map(fileRow));
  list.hidden = files.length === 0;
  $("drop").classList.toggle("compact", files.length > 0);

  const s = state.settings;
  for (const group of document.querySelectorAll("[data-setting]")) {
    const [tab, key] = group.dataset.setting.split(".");
    for (const btn of group.children) {
      btn.classList.toggle("active", String(s[tab][key]) === btn.dataset.value);
      btn.disabled = state.running;
    }
  }
  $("quality").value = s.flatten.quality;
  $("quality-out").textContent = s.flatten.quality;
  $("gray").checked = s.flatten.grayscale;
  for (const input of document.querySelectorAll("[data-suffix]")) input.value = s[input.dataset.suffix].suffix;
  $("pages").value = s.images.pages;
  $("outdir").textContent = s.outputDir || t("sameFolderPath");
  $("outdir").title = s.outputDir || "";
  $("reset-dir").hidden = !s.outputDir;

  const pages = files.reduce((sum, f) => sum + (f.pages || 0), 0);
  $("summary").textContent = files.length ? t("summary", files.length, pages) : "";

  const busy = state.running;
  for (const id of ["add", "clear", "choose-dir", "reset-dir", "quality", "gray", "pages"]) $(id).disabled = busy;
  for (const input of document.querySelectorAll("[data-suffix]")) input.disabled = busy;
  $("start").hidden = busy;
  $("start").disabled = !files.some(runnable);
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
  store.set("settings.v2", state.settings);
  render();
}

// ---------- wiring ----------

$("lang-toggle").onclick = () => {
  state.lang = state.lang === "th" ? "en" : "th";
  store.set("lang", state.lang);
  render();
};

$("theme-toggle").onclick = () => {
  state.theme = currentTheme() === "dark" ? "light" : "dark";
  store.set("theme", state.theme);
  applyTheme();
};
// Until the switch is used, follow the system setting as it changes.
systemDark.addEventListener("change", () => { if (state.theme === null) applyTheme(); });

for (const btn of $("tabs").children) {
  btn.onclick = () => {
    if (state.running) return;
    state.tab = btn.dataset.tab;
    store.set("tab", state.tab);
    render();
  };
}

$("add").onclick = async () => {
  const picked = await open({ multiple: true, filters: [{ name: "PDF", extensions: ["pdf", "PDF"] }] });
  if (picked) await addPaths(Array.isArray(picked) ? picked : [picked]);
};

for (const group of document.querySelectorAll("[data-setting]")) {
  const [tab, key] = group.dataset.setting.split(".");
  for (const btn of group.children) {
    btn.onclick = () => {
      const raw = btn.dataset.value;
      state.settings[tab][key] = /^\d+$/.test(raw) ? Number(raw) : raw;
      saveSettings();
    };
  }
}
$("quality").oninput = (e) => { state.settings.flatten.quality = Number(e.target.value); saveSettings(); };
$("gray").onchange = (e) => { state.settings.flatten.grayscale = e.target.checked; saveSettings(); };
for (const input of document.querySelectorAll("[data-suffix]")) {
  input.onchange = (e) => {
    const tab = input.dataset.suffix;
    // Keep the suffix safe for file names on every OS.
    state.settings[tab].suffix = e.target.value.replace(/[\\/:*?"<>|]/g, "").trim() || DEFAULT_SETTINGS[tab].suffix;
    saveSettings();
  };
}
$("pages").onchange = (e) => {
  state.settings.images.pages = e.target.value.replace(/[^\d,\-\s]/g, "").trim();
  saveSettings();
};
$("choose-dir").onclick = async () => {
  const dir = await open({ directory: true });
  if (dir) { state.settings.outputDir = dir; saveSettings(); }
};
$("reset-dir").onclick = () => { state.settings.outputDir = null; saveSettings(); };

$("clear").onclick = () => { state.files[state.tab] = []; render(); };
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
