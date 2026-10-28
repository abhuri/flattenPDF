"use strict";

// Watermark tab: settings, text/image stamp, live preview and saved profiles.
// Loaded after main.js and uses its globals (state, t, $, el, invoke, toast...).
//
// Text is drawn on a canvas here and sent as a PNG, because the browser
// shapes Thai (and every other script) correctly; PDFium text objects do not.

Object.assign(STRINGS.en, {
  tabWatermark: "Watermark",
  hintWatermark: "Stamps text or a picture onto the pages. Tick \"Flatten afterwards\" so it cannot be removed.",
  startWatermark: "Add watermark",
  profile: "Profile",
  noProfile: "— Unsaved settings —",
  profileName: "Profile name",
  saveProfile: "💾 Save",
  deleteProfile: "🗑 Delete",
  confirmDelete: "Delete?",
  profileSaved: (n) => `Saved profile "${n}"`,
  profileDeleted: (n) => `Deleted profile "${n}"`,
  needProfileName: "Type a name for the profile",
  wmType: "Watermark",
  wmText: "Text",
  wmImage: "Picture",
  wmTextLabel: "Text",
  font: "Font",
  color: "Color",
  bold: "Bold",
  italic: "Italic",
  wmImageLabel: "Picture",
  chooseImage: "Choose…",
  noImage: "No picture chosen",
  size: "Size (% of page width)",
  angle: "Angle",
  opacity: "Opacity",
  position: "Position",
  tile: "Repeat across the page",
  pagesToStamp: "Pages",
  pagesAll: "All",
  pagesOnly: "Only",
  pagesExcept: "All except",
  pagesHint: "e.g. 1-3, 7, 10-",
  flattenAfter: "Flatten afterwards",
  flattenAfterNote: "Bakes the watermark into the page images so it cannot be removed (uses the Flatten tab's resolution and quality).",
  previewEmpty: "Add a PDF to see a preview",
  previewLocked: "Enter the file's password to see a preview",
  previewPage: (p, n) => `Page ${p} of ${n}`,
  needText: "Type the watermark text",
  needImage: "Choose a picture for the watermark",
  needPages: "Type which pages to use",
  badImage: "This picture could not be opened",
});

Object.assign(STRINGS.th, {
  tabWatermark: "ลายน้ำ",
  hintWatermark: "ใส่ข้อความหรือรูปภาพเป็นลายน้ำบนหน้า PDF ติ๊ก \"Flatten หลังใส่ลายน้ำ\" เพื่อไม่ให้ลบลายน้ำออกได้",
  startWatermark: "ใส่ลายน้ำ",
  profile: "Profile",
  noProfile: "— ค่าที่ยังไม่บันทึก —",
  profileName: "ชื่อ Profile",
  saveProfile: "💾 บันทึก",
  deleteProfile: "🗑 ลบ",
  confirmDelete: "ยืนยันลบ?",
  profileSaved: (n) => `บันทึก Profile "${n}" แล้ว`,
  profileDeleted: (n) => `ลบ Profile "${n}" แล้ว`,
  needProfileName: "ใส่ชื่อ Profile ก่อนบันทึก",
  wmType: "ลายน้ำ",
  wmText: "ข้อความ",
  wmImage: "รูปภาพ",
  wmTextLabel: "ข้อความ",
  font: "ฟอนต์",
  color: "สี",
  bold: "ตัวหนา",
  italic: "ตัวเอียง",
  wmImageLabel: "รูปภาพ",
  chooseImage: "เลือกรูป…",
  noImage: "ยังไม่ได้เลือกรูป",
  size: "ขนาด (% ของความกว้างหน้า)",
  angle: "มุมเอียง",
  opacity: "ความทึบ",
  position: "ตำแหน่ง",
  tile: "เรียงซ้ำเต็มหน้า",
  pagesToStamp: "หน้า",
  pagesAll: "ทุกหน้า",
  pagesOnly: "เฉพาะ",
  pagesExcept: "ยกเว้น",
  pagesHint: "เช่น 1-3, 7, 10-",
  flattenAfter: "Flatten หลังใส่ลายน้ำ",
  flattenAfterNote: "รวมลายน้ำเป็นภาพเดียวกับหน้า ทำให้ลบลายน้ำไม่ได้ (ใช้ความละเอียดและคุณภาพจากแท็บ Flatten)",
  previewEmpty: "เพิ่มไฟล์ PDF เพื่อดูตัวอย่าง",
  previewLocked: "ใส่รหัสผ่านของไฟล์เพื่อดูตัวอย่าง",
  previewPage: (p, n) => `หน้า ${p} จาก ${n}`,
  needText: "ใส่ข้อความลายน้ำก่อน",
  needImage: "เลือกรูปภาพสำหรับลายน้ำก่อน",
  needPages: "ระบุหน้าที่ต้องการก่อน",
  badImage: "เปิดรูปภาพนี้ไม่ได้",
});

const WM_FONTS = [
  "Tahoma", "Leelawadee UI", "TH Sarabun New", "Angsana New", "Cordia New", "Browallia New",
  "Arial", "Arial Black", "Segoe UI", "Verdana", "Georgia", "Times New Roman", "Courier New", "Impact",
];

const WM_DEFAULTS = {
  type: "text",
  text: "CONFIDENTIAL",
  font: "Tahoma",
  color: "#c0392b",
  bold: true,
  italic: false,
  /** PNG data URL of the chosen picture (already scaled down). */
  image: null,
  imageName: "",
  size: 60,
  rotation: 45,
  opacity: 25,
  position: "center",
  pageMode: "all",
  pages: "",
  flatten: false,
  suffix: "_watermarked",
};

/** Text is drawn at this font size; plenty for print resolution. */
const TEXT_PX = 200;
/** Longest edge of a stamp in pixels (the backend accepts up to 4096). */
const STAMP_MAX_PX = 4000;
/** Pictures are scaled down to this before being stored in profiles. */
const IMAGE_MAX_PX = 2000;

const wm = {
  settings: structuredClone(WM_DEFAULTS),
  /** [{ name, settings }] */
  profiles: [],
  selected: "",
  textStamp: { key: "", url: null },
  preview: { page: 0, key: "", seq: 0, timer: 0 },
  saveTimer: 0,
  deleteArmed: 0,
};

// ---------- the stamp ----------

/** Resolves to the stamp as a PNG data URL, or null when there is none yet. */
async function stampUrl() {
  const s = wm.settings;
  if (s.type === "image") return s.image;
  if (!s.text.trim()) return null;
  const key = JSON.stringify([s.text, s.font, s.color, s.bold, s.italic]);
  if (wm.textStamp.key !== key) wm.textStamp = { key, url: await renderText(s) };
  return wm.textStamp.url;
}

async function renderText(s) {
  const lines = s.text.split("\n");
  const font = `${s.italic ? "italic " : ""}${s.bold ? "bold " : ""}${TEXT_PX}px "${s.font}", "Leelawadee UI", Tahoma, sans-serif`;
  try {
    await document.fonts.load(font, s.text);
  } catch {
    // Unknown fonts fall back to the next family in the list.
  }
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  ctx.font = font;
  // Measure all lines together so Thai marks above and below fit.
  const metrics = ctx.measureText(lines.join(" ") || "M");
  const ascent = metrics.actualBoundingBoxAscent;
  const descent = metrics.actualBoundingBoxDescent;
  const lineHeight = (ascent + descent) * 1.15;
  const pad = TEXT_PX * 0.12;
  const width = Math.max(...lines.map((line) => ctx.measureText(line).width), 1) + pad * 2;
  const height = lineHeight * (lines.length - 1) + ascent + descent + pad * 2;
  const scale = Math.min(1, STAMP_MAX_PX / Math.max(width, height));

  canvas.width = Math.ceil(width * scale);
  canvas.height = Math.ceil(height * scale);
  ctx.scale(scale, scale);
  ctx.font = font;
  ctx.fillStyle = s.color;
  ctx.textAlign = "center";
  lines.forEach((line, i) => ctx.fillText(line, width / 2, pad + ascent + i * lineHeight));
  return canvas.toDataURL("image/png");
}

/** Draws any picture the webview can open into a PNG data URL, scaled down to `maxPx`. */
function pictureToPng(src, maxPx) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      // SVGs without a size report 0; give them a sensible one.
      const w = img.naturalWidth || 1000;
      const h = img.naturalHeight || 1000;
      const k = Math.min(1, maxPx / Math.max(w, h));
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.round(w * k));
      canvas.height = Math.max(1, Math.round(h * k));
      canvas.getContext("2d").drawImage(img, 0, 0, canvas.width, canvas.height);
      resolve(canvas.toDataURL("image/png"));
    };
    img.onerror = () => reject(new Error(t("badImage")));
    img.src = src;
  });
}

async function pickImage() {
  const path = await open({
    multiple: false,
    filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"] }],
  });
  if (!path) return;
  try {
    const src = await invoke("read_image", { path });
    update({ image: await pictureToPng(src, IMAGE_MAX_PX), imageName: path.split(/[\\/]/).pop() });
  } catch (err) {
    toast(err.message || String(err));
  }
}

/** Settings in the shape the backend expects (see WatermarkSettings in main.rs). */
function backendSettings(stamp) {
  const s = wm.settings;
  return {
    stamp,
    size: s.size,
    rotation: s.rotation,
    opacity: s.opacity,
    position: s.position,
    pageMode: s.pageMode,
    pages: s.pages,
    flatten: s.flatten,
    dpi: state.settings.flatten.dpi,
    quality: state.settings.flatten.quality,
    suffix: s.suffix,
  };
}

// ---------- preview ----------

/** The file the preview shows: the first one in the list that can be opened. */
function previewFile() {
  return state.files.watermark.find((f) => f.status !== "invalid");
}

function schedulePreview() {
  const file = previewFile();
  const s = wm.settings;
  const key = JSON.stringify([
    file?.path, file?.password, file?.status === "locked", wm.preview.page, s.type,
    s.type === "image" ? s.image?.length : wm.textStamp.key || [s.text, s.font, s.color, s.bold, s.italic],
    s.image?.slice(-64), s.size, s.rotation, s.opacity, s.position, s.pageMode, s.pages,
  ]);
  if (key === wm.preview.key) return;
  wm.preview.key = key;
  clearTimeout(wm.preview.timer);
  wm.preview.timer = setTimeout(updatePreview, 250);
}

function showPreviewMessage(message) {
  $("wm-preview-img").hidden = true;
  $("wm-preview-msg").hidden = false;
  $("wm-preview-msg").textContent = message;
}

async function updatePreview() {
  const file = previewFile();
  const seq = ++wm.preview.seq;
  if (!file) return showPreviewMessage(t("previewEmpty"));
  if (file.status === "locked" && !file.password) return showPreviewMessage(t("previewLocked"));

  const frame = document.querySelector(".preview-frame");
  frame.classList.add("loading");
  try {
    const stamp = await stampUrl();
    if (seq !== wm.preview.seq) return;
    if (!stamp) {
      showPreviewMessage(wm.settings.type === "image" ? t("needImage") : t("needText"));
      return;
    }
    const url = await invoke("preview", {
      path: file.path,
      password: file.password || null,
      page: wm.preview.page,
      settings: backendSettings(stamp),
      maxPixels: 900,
    });
    if (seq !== wm.preview.seq) return; // a newer preview is on its way
    $("wm-preview-img").src = url;
    $("wm-preview-img").hidden = false;
    $("wm-preview-msg").hidden = true;
  } catch (err) {
    if (seq === wm.preview.seq) showPreviewMessage(err.message || String(err));
  } finally {
    if (seq === wm.preview.seq) frame.classList.remove("loading");
  }
}

function turnPreviewPage(delta) {
  const pages = previewFile()?.pages;
  if (!pages) return;
  wm.preview.page = Math.min(pages - 1, Math.max(0, wm.preview.page + delta));
  render();
}

// ---------- profiles and persistence ----------

function persist() {
  clearTimeout(wm.saveTimer);
  wm.saveTimer = setTimeout(() => {
    const data = { version: 1, current: wm.settings, selected: wm.selected, profiles: wm.profiles };
    invoke("save_watermark_state", { data }).catch((err) => toast(String(err)));
  }, 400);
}

function update(changes) {
  Object.assign(wm.settings, changes);
  persist();
  render();
}

function saveProfile() {
  const name = $("wm-profile-name").value.trim() || wm.selected;
  if (!name) {
    toast(t("needProfileName"));
    $("wm-profile-name").focus();
    return;
  }
  const profile = { name, settings: structuredClone(wm.settings) };
  const index = wm.profiles.findIndex((p) => p.name === name);
  if (index >= 0) wm.profiles[index] = profile;
  else wm.profiles.push(profile);
  wm.profiles.sort((a, b) => a.name.localeCompare(b.name));
  wm.selected = name;
  persist();
  toast(t("profileSaved", name));
  render();
}

function deleteProfile() {
  if (!wm.selected) return;
  // First click arms the button; a second click within 3 s deletes.
  if (!wm.deleteArmed) {
    wm.deleteArmed = setTimeout(() => {
      wm.deleteArmed = 0;
      render();
    }, 3000);
    render();
    return;
  }
  clearTimeout(wm.deleteArmed);
  wm.deleteArmed = 0;
  const name = wm.selected;
  wm.profiles = wm.profiles.filter((p) => p.name !== name);
  wm.selected = "";
  $("wm-profile-name").value = "";
  persist();
  toast(t("profileDeleted", name));
  render();
}

function selectProfile(name) {
  wm.selected = name;
  const profile = wm.profiles.find((p) => p.name === name);
  if (profile) {
    wm.settings = { ...structuredClone(WM_DEFAULTS), ...structuredClone(profile.settings) };
    $("wm-profile-name").value = name;
  }
  persist();
  render();
}

async function loadState() {
  try {
    const data = await invoke("load_watermark_state");
    if (data && typeof data === "object") {
      wm.settings = { ...structuredClone(WM_DEFAULTS), ...(data.current || {}) };
      wm.profiles = Array.isArray(data.profiles) ? data.profiles.filter((p) => p?.name && p.settings) : [];
      wm.selected = wm.profiles.some((p) => p.name === data.selected) ? data.selected : "";
      if (wm.selected) $("wm-profile-name").value = wm.selected;
    }
  } catch (err) {
    toast(String(err));
  }
}

// ---------- rendering ----------

/** Sets a value only when it differs, so typing is not interrupted. */
function setValue(input, value) {
  if (input.value !== String(value)) input.value = value;
}

function renderWatermark() {
  const s = wm.settings;
  const busy = state.running;

  const select = $("wm-profile");
  const options = [el("option", { value: "", textContent: t("noProfile") })]
    .concat(wm.profiles.map((p) => el("option", { value: p.name, textContent: p.name })));
  select.replaceChildren(...options);
  select.value = wm.selected;
  $("wm-profile-delete").disabled = busy || !wm.selected;
  $("wm-profile-delete").textContent = wm.deleteArmed ? t("confirmDelete") : t("deleteProfile");
  $("wm-profile-delete").classList.toggle("danger", Boolean(wm.deleteArmed));

  for (const group of document.querySelectorAll("[data-wm]")) {
    for (const btn of group.children) {
      btn.classList.toggle("active", s[group.dataset.wm] === btn.dataset.value);
      btn.disabled = busy;
    }
  }
  document.querySelector(".wm-text-only").hidden = s.type !== "text";
  document.querySelector(".wm-image-only").hidden = s.type !== "image";

  setValue($("wm-text"), s.text);
  setValue($("wm-font"), s.font);
  setValue($("wm-color"), s.color);
  $("wm-bold").classList.toggle("active", s.bold);
  $("wm-italic").classList.toggle("active", s.italic);
  $("wm-bold").setAttribute("aria-pressed", String(s.bold));
  $("wm-italic").setAttribute("aria-pressed", String(s.italic));

  $("wm-image-thumb").hidden = !s.image;
  if (s.image && $("wm-image-thumb").src !== s.image) $("wm-image-thumb").src = s.image;
  $("wm-image-name").textContent = s.image ? s.imageName : t("noImage");

  setValue($("wm-size"), s.size);
  $("wm-size-out").textContent = `${s.size}%`;
  setValue($("wm-rotation"), s.rotation);
  $("wm-rotation-out").textContent = `${s.rotation}°`;
  setValue($("wm-opacity"), s.opacity);
  $("wm-opacity-out").textContent = `${s.opacity}%`;
  for (const btn of $("wm-position").children) {
    btn.classList.toggle("active", btn.dataset.value === s.position);
    btn.disabled = busy;
  }
  $("wm-pages").hidden = s.pageMode === "all";
  setValue($("wm-pages"), s.pages);
  $("wm-flatten").checked = s.flatten;
  setValue($("wm-suffix"), s.suffix);

  for (const id of ["wm-profile", "wm-profile-name", "wm-profile-save", "wm-text", "wm-font", "wm-color",
    "wm-bold", "wm-italic", "wm-image-pick", "wm-size", "wm-rotation", "wm-opacity", "wm-pages",
    "wm-flatten", "wm-suffix"]) {
    $(id).disabled = busy;
  }
  for (const btn of $("wm-rotation-quick").children) btn.disabled = busy;

  const file = previewFile();
  wm.preview.page = Math.min(wm.preview.page, Math.max(0, (file?.pages ?? 1) - 1));
  $("wm-page-label").textContent = file?.pages ? t("previewPage", wm.preview.page + 1, file.pages) : "";
  $("wm-prev-page").disabled = !file?.pages || wm.preview.page === 0;
  $("wm-next-page").disabled = !file?.pages || wm.preview.page >= file.pages - 1;
  schedulePreview();
}

// ---------- wiring ----------

$("wm-fonts").replaceChildren(...WM_FONTS.map((font) => el("option", { value: font })));

for (const group of document.querySelectorAll("[data-wm]")) {
  for (const btn of group.children) {
    btn.onclick = () => update({ [group.dataset.wm]: btn.dataset.value });
  }
}
for (const btn of $("wm-position").children) btn.onclick = () => update({ position: btn.dataset.value });
for (const btn of $("wm-rotation-quick").children) btn.onclick = () => update({ rotation: Number(btn.dataset.value) });

$("wm-text").oninput = (e) => update({ text: e.target.value });
$("wm-font").onchange = (e) => update({ font: e.target.value.trim() || WM_DEFAULTS.font });
$("wm-color").oninput = (e) => update({ color: e.target.value });
$("wm-bold").onclick = () => update({ bold: !wm.settings.bold });
$("wm-italic").onclick = () => update({ italic: !wm.settings.italic });
$("wm-image-pick").onclick = pickImage;
$("wm-size").oninput = (e) => update({ size: Number(e.target.value) });
$("wm-rotation").oninput = (e) => update({ rotation: Number(e.target.value) });
$("wm-opacity").oninput = (e) => update({ opacity: Number(e.target.value) });
$("wm-pages").onchange = (e) => update({ pages: e.target.value.replace(/[^\d,\-\s]/g, "").trim() });
$("wm-flatten").onchange = (e) => update({ flatten: e.target.checked });
$("wm-suffix").onchange = (e) => {
  // Keep the suffix safe for file names on every OS.
  update({ suffix: e.target.value.replace(/[\\/:*?"<>|]/g, "").trim() || WM_DEFAULTS.suffix });
};

$("wm-profile").onchange = (e) => selectProfile(e.target.value);
$("wm-profile-save").onclick = saveProfile;
$("wm-profile-name").addEventListener("keydown", (e) => { if (e.key === "Enter") saveProfile(); });
$("wm-profile-delete").onclick = deleteProfile;
$("wm-prev-page").onclick = () => turnPreviewPage(-1);
$("wm-next-page").onclick = () => turnPreviewPage(1);

tabHooks.watermark = {
  render: renderWatermark,
  validate() {
    const s = wm.settings;
    if (s.type === "text" && !s.text.trim()) return t("needText");
    if (s.type === "image" && !s.image) return t("needImage");
    if (s.pageMode !== "all" && !s.pages.trim()) return t("needPages");
    return null;
  },
  async task() {
    return { mode: "watermark", ...backendSettings(await stampUrl()) };
  },
};

render();
loadState().then(render);
