import "@fontsource/roboto/100.css";
import "@fontsource/roboto/300.css";
import "@fontsource/roboto/400.css";
import "@fontsource/roboto/700.css";
import "./styles.css";

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

/**
 * Mirror of Reshot.Core.Settings.AppSettings (SPEC §13), camelCase, with the
 * documented defaults. Only these keys are sent back; the Rust side deep-merges
 * them into settings.json so keys this UI does not know about survive a save.
 */
interface Settings {
  hotkey: string;
  audioHotkey: string;
  autostart: boolean;
  autostartElevated: boolean;
  dim: { opacity: number; color: string };
  paths: { screenshots: string; videos: string; records: string };
  format: { image: string; quality: number };
  filename: { template: string };
  video: {
    fps: number;
    audio: { mic: boolean; system: boolean; askOnSave: boolean; micDevice: string };
    corners: { enabled: boolean; color: string; opacity: number };
  };
  audio: { system: boolean; mic: boolean; micDevice: string };
  update: { auto: boolean };
  radial: { clickToChoose: boolean };
}

const defaults = (): Settings => ({
  hotkey: "PrtScn",
  audioHotkey: "",
  autostart: true,
  autostartElevated: false,
  dim: { opacity: 0.5, color: "#000000" },
  paths: { screenshots: "", videos: "", records: "" },
  format: { image: "png", quality: 90 },
  filename: { template: "Reshot_{date}_{time}" },
  video: {
    fps: 60,
    audio: { mic: true, system: true, askOnSave: true, micDevice: "default" },
    corners: { enabled: true, color: "#3C9898", opacity: 0.7 },
  },
  audio: { system: true, mic: false, micDevice: "default" },
  update: { auto: true },
  radial: { clickToChoose: true },
});

/** Overlays whatever is on disk onto the defaults, one level at a time. */
function hydrate(raw: Record<string, any>): Settings {
  const base = defaults();
  const merge = (target: any, source: any) => {
    if (!source || typeof source !== "object") return;
    for (const key of Object.keys(target)) {
      const value = source[key];
      if (value === undefined || value === null) continue;
      if (typeof target[key] === "object" && !Array.isArray(target[key])) merge(target[key], value);
      else target[key] = value;
    }
  };
  merge(base, raw);
  return base;
}

let draft = defaults();
let saved = defaults();

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const status = $<HTMLSpanElement>("status");

function markDirty() {
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  ($<HTMLButtonElement>("apply")).disabled = !dirty;
  if (dirty) status.textContent = "";
}

// ---------------------------------------------------------------- tabs

document.querySelectorAll<HTMLElement>(".tab").forEach((tab) => {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
    document.querySelectorAll(".panel").forEach((p) => p.classList.remove("active"));
    tab.classList.add("active");
    document.querySelector(`[data-panel="${tab.dataset.tab}"]`)?.classList.add("active");
  });
});

// ---------------------------------------------------------------- select

interface SelectOption {
  value: string;
  label: string;
}

/**
 * Dropdown in the HL2 idiom: a raised button that opens an inset list whose
 * rows highlight in the accent colour. A native <select> is unusable here -
 * its popup is drawn by the OS and cannot carry the bevel styling.
 */
function buildSelect(
  container: HTMLElement,
  options: SelectOption[],
  current: string,
  onChange: (value: string) => void
) {
  const selected = options.find((o) => o.value === current) ?? options[0];
  container.innerHTML = "";
  container.classList.add("select");

  const value = document.createElement("div");
  value.className = "selectValue";
  value.textContent = selected?.label ?? "";
  container.appendChild(value);

  const list = document.createElement("div");
  list.className = "selectList";
  for (const option of options) {
    const row = document.createElement("div");
    row.className = "selectOption" + (option.value === selected?.value ? " selected" : "");
    row.textContent = option.label;
    row.title = option.label;
    row.addEventListener("click", (event) => {
      event.stopPropagation();
      container.classList.remove("open");
      if (option.value === selected?.value) return;
      onChange(option.value);
      buildSelect(container, options, option.value, onChange);
    });
    list.appendChild(row);
  }
  container.appendChild(list);

  value.addEventListener("click", (event) => {
    event.stopPropagation();
    const wasOpen = container.classList.contains("open");
    closeAllPopovers();
    container.classList.toggle("open", !wasOpen);
  });
}

/** Dismisses every dropdown and colour palette; only one may be open at a time. */
function closeAllPopovers() {
  document.querySelectorAll(".select.open, .palette.open").forEach((el) => el.classList.remove("open"));
}

document.addEventListener("click", closeAllPopovers);

// ---------------------------------------------------------------- hotkeys

/**
 * Renders a KeyboardEvent as a token string that Reshot.Core's
 * HotkeyDefinition.TryParse accepts, in the same order its ToString() emits:
 * Ctrl, Alt, Shift, Win, then the main key.
 */
const namedKeys: Record<string, string> = {
  PrintScreen: "PrtScn",
  Insert: "Insert",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Space: "Space",
  Tab: "Tab",
  Enter: "Enter",
  NumpadEnter: "Enter",
  Escape: "Esc",
  Backspace: "Backspace",
  Pause: "Pause",
  ScrollLock: "ScrollLock",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
};

function mainKeyOf(event: KeyboardEvent): string | null {
  // PrintScreen often arrives with an empty `code` in WebView2; `key` may still be set.
  if (event.key === "PrintScreen" || event.code === "PrintScreen") return "PrtScn";

  const code = event.code;
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return namedKeys[code] ?? null;
}

function bindHotkeyField(input: HTMLInputElement, apply: (token: string) => void) {
  const capture = (event: KeyboardEvent) => {
    event.preventDefault();
    const key = mainKeyOf(event);
    if (!key) return; // a bare modifier press, keep waiting for the real key

    const parts: string[] = [];
    if (event.ctrlKey) parts.push("Ctrl");
    if (event.altKey) parts.push("Alt");
    if (event.shiftKey) parts.push("Shift");
    if (event.metaKey) parts.push("Win");
    parts.push(key);

    const token = parts.join("+");
    input.value = token;
    apply(token);
    markDirty();
  };

  // keyup: PrintScreen sometimes only fires on release (or not at all — see poll below).
  input.addEventListener("keydown", capture);
  input.addEventListener("keyup", capture);

  // WebView2 usually swallows PrintScreen. Poll the native key state while focused.
  let pollId: number | null = null;
  let lastNative: string | null = null;
  const stopPoll = () => {
    if (pollId !== null) {
      window.clearInterval(pollId);
      pollId = null;
    }
    lastNative = null;
  };
  input.addEventListener("focus", () => {
    stopPoll();
    pollId = window.setInterval(async () => {
      try {
        const token = await invoke<string | null>("poll_print_screen");
        if (!token) {
          lastNative = null;
          return;
        }
        if (token === lastNative) return;
        lastNative = token;
        input.value = token;
        apply(token);
        markDirty();
      } catch {
        // Native poll unavailable (non-Windows / old binary): ignore.
      }
    }, 50);
  });
  input.addEventListener("blur", stopPoll);
}

// ---------------------------------------------------------------- bindings

function bindCheckbox(id: string, get: () => boolean, set: (value: boolean) => void) {
  const input = $<HTMLInputElement>(id);
  input.checked = get();
  input.addEventListener("change", () => {
    set(input.checked);
    markDirty();
  });
}

function bindText(id: string, get: () => string, set: (value: string) => void) {
  const input = $<HTMLInputElement>(id);
  input.value = get();
  input.addEventListener("input", () => {
    set(input.value);
    markDirty();
  });
}

/** Slider over a 0..1 fraction rendered as a percentage. */
function bindFractionSlider(
  id: string,
  labelId: string,
  get: () => number,
  set: (value: number) => void
) {
  const input = $<HTMLInputElement>(id);
  const label = $<HTMLSpanElement>(labelId);
  const render = (percent: number) => (label.textContent = `${percent}%`);

  input.value = String(Math.round(get() * 100));
  render(Number(input.value));

  input.addEventListener("input", () => {
    const percent = Number(input.value);
    render(percent);
    set(percent / 100);
    markDirty();
  });
}

function bindIntSlider(
  id: string,
  labelId: string,
  get: () => number,
  set: (value: number) => void
) {
  const input = $<HTMLInputElement>(id);
  const label = $<HTMLSpanElement>(labelId);

  input.value = String(get());
  label.textContent = input.value;

  input.addEventListener("input", () => {
    label.textContent = input.value;
    set(Number(input.value));
    markDirty();
  });
}

/**
 * The overlay's brush palette (OverlayWindow.Palette), reused verbatim so a
 * colour picked in settings matches the ones offered while drawing.
 */
const palette = [
  "#FF3B30", "#FF9500", "#FFCC00", "#34C759", "#00C7BE", "#30B0C7", "#007AFF", "#5856D6",
  "#AF52DE", "#FF2D55", "#A2845E", "#8E8E93", "#FFFFFF", "#C7C7CC", "#48484A", "#000000",
];

/** Hex text field kept in sync with its inset colour well and swatch grid. */
function bindColor(id: string, swatchId: string, get: () => string, set: (value: string) => void) {
  const input = $<HTMLInputElement>(id);
  const swatch = $<HTMLSpanElement>(swatchId);
  const grid = document.querySelector<HTMLElement>(`.palette[data-target="${id}"]`);

  const buttons: HTMLElement[] = [];
  if (grid) {
    grid.innerHTML = "";
    for (const hex of palette) {
      const button = document.createElement("div");
      button.className = "swatchBtn";
      button.style.backgroundColor = hex;
      button.title = hex;
      button.dataset.hex = hex;
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        input.value = hex;
        apply(hex);
        markDirty();
        grid.classList.remove("open");
      });
      grid.appendChild(button);
      buttons.push(button);
    }
  }

  const apply = (hex: string) => {
    swatch.style.backgroundColor = /^#[0-9a-fA-F]{6}$/.test(hex) ? hex : "transparent";
    const current = hex.toUpperCase();
    for (const button of buttons)
      button.classList.toggle("selected", button.dataset.hex === current);
    set(hex);
  };

  // The palette is a popover on the colour well, not a permanent grid.
  swatch.addEventListener("click", (event) => {
    if (!grid) return;
    event.stopPropagation();
    const wasOpen = grid.classList.contains("open");
    closeAllPopovers();
    grid.classList.toggle("open", !wasOpen);
  });

  input.value = get();
  apply(input.value);

  input.addEventListener("input", () => {
    apply(input.value);
    markDirty();
  });
}

function bindBrowse(buttonSelector: string, inputId: string, set: (value: string) => void) {
  const button = document.querySelector<HTMLButtonElement>(buttonSelector);
  const input = $<HTMLInputElement>(inputId);
  button?.addEventListener("click", async () => {
    const picked = await open({ directory: true, multiple: false, defaultPath: input.value });
    if (typeof picked !== "string") return;
    input.value = picked;
    set(picked);
    markDirty();
  });
}

// ---------------------------------------------------------------- wiring

async function load() {
  const raw = await invoke<Record<string, any>>("load_settings").catch((error) => {
    status.textContent = String(error);
    return {};
  });

  draft = hydrate(raw);

  // Empty output folders mean "use the per-user default"; show the resolved path
  // rather than an empty box, matching what the C# side writes back on load.
  const dirs = await invoke<Record<string, string>>("default_dirs").catch(() => ({}) as any);
  if (!draft.paths.screenshots) draft.paths.screenshots = dirs.screenshots ?? "";
  if (!draft.paths.videos) draft.paths.videos = dirs.videos ?? "";
  if (!draft.paths.records) draft.paths.records = dirs.records ?? "";

  saved = JSON.parse(JSON.stringify(draft));

  $<HTMLParagraphElement>("settingsPath").textContent = await invoke<string>("settings_path").catch(
    () => ""
  );

  // -- General
  const hotkeyInput = $<HTMLInputElement>("hotkey");
  hotkeyInput.value = draft.hotkey;
  // The Info tab quotes both hotkeys several times, so every chip has to follow a
  // rebind, a shortcut reference that lies is worse than none.
  const infoHotkey = $<HTMLElement>("infoHotkey");
  const infoHotkeyAliases = document.querySelectorAll<HTMLElement>(".infoHotkeyAlias");
  const showCaptureHotkey = (token: string) => {
    infoHotkey.textContent = token;
    infoHotkeyAliases.forEach((el) => (el.textContent = token));
  };
  showCaptureHotkey(draft.hotkey);
  bindHotkeyField(hotkeyInput, (token) => {
    draft.hotkey = token;
    showCaptureHotkey(token);
  });

  const audioHotkeyInput = $<HTMLInputElement>("audioHotkey");
  audioHotkeyInput.value = draft.audioHotkey;
  const infoAudioHotkeys = document.querySelectorAll<HTMLElement>(".infoAudioHotkey");
  const showAudioHotkey = (token: string) =>
    infoAudioHotkeys.forEach((el) => (el.textContent = token || "not set"));
  showAudioHotkey(draft.audioHotkey);
  bindHotkeyField(audioHotkeyInput, (token) => {
    draft.audioHotkey = token;
    showAudioHotkey(token);
  });

  $<HTMLButtonElement>("clearAudioHotkey").addEventListener("click", () => {
    audioHotkeyInput.value = "";
    draft.audioHotkey = "";
    showAudioHotkey("");
    markDirty();
  });

  // Clicking is the default, so the checkbox offers the gesture and therefore writes the
  // stored key negated. The key keeps its own name pointing the right way: clickToChoose
  // true really is click-to-choose, whatever the box next to it happens to say.
  //
  // The Info tab lists the radial bindings, and the two modes have different ones, so the
  // reference follows the checkbox immediately rather than after a save-and-reopen.
  const showRadialMode = (gesture: boolean) =>
    document.body.classList.toggle("radialGestureMode", gesture);
  showRadialMode(!draft.radial.clickToChoose);
  bindCheckbox("radialGesture", () => !draft.radial.clickToChoose, (v) => {
    draft.radial.clickToChoose = !v;
    showRadialMode(v);
  });

  // "…as administrator" only means anything while autostart is on, and leaving it checked
  // but inert would be a setting that lies. Turning autostart off clears it outright.
  const elevatedInput = $<HTMLInputElement>("autostartElevated");
  const syncElevated = () => {
    elevatedInput.disabled = !draft.autostart;
    elevatedInput.closest(".checkboxWrapper")?.classList.toggle("disabled", !draft.autostart);
  };
  bindCheckbox("autostart", () => draft.autostart, (v) => {
    draft.autostart = v;
    if (!v) {
      draft.autostartElevated = false;
      elevatedInput.checked = false;
    }
    syncElevated();
  });
  bindCheckbox("autostartElevated", () => draft.autostartElevated, (v) => (draft.autostartElevated = v));
  syncElevated();

  bindCheckbox("updateAuto", () => draft.update.auto, (v) => (draft.update.auto = v));

  // -- Overlay
  bindFractionSlider("dimOpacity", "dimOpacityValue", () => draft.dim.opacity, (v) => (draft.dim.opacity = v));
  bindColor("dimColor", "dimColorSwatch", () => draft.dim.color, (v) => (draft.dim.color = v));

  // -- Output
  bindText("pathScreenshots", () => draft.paths.screenshots, (v) => (draft.paths.screenshots = v));
  bindText("pathVideos", () => draft.paths.videos, (v) => (draft.paths.videos = v));
  bindText("pathRecords", () => draft.paths.records, (v) => (draft.paths.records = v));
  bindBrowse('[data-browse="pathScreenshots"]', "pathScreenshots", (v) => (draft.paths.screenshots = v));
  bindBrowse('[data-browse="pathVideos"]', "pathVideos", (v) => (draft.paths.videos = v));
  bindBrowse('[data-browse="pathRecords"]', "pathRecords", (v) => (draft.paths.records = v));

  buildSelect(
    $("formatImage"),
    [
      { value: "png", label: "PNG" },
      { value: "jpg", label: "JPG" },
      { value: "webp", label: "WebP" },
    ],
    draft.format.image,
    (value) => {
      draft.format.image = value;
      markDirty();
    }
  );

  bindIntSlider("quality", "qualityValue", () => draft.format.quality, (v) => (draft.format.quality = v));
  bindText("filenameTemplate", () => draft.filename.template, (v) => (draft.filename.template = v));

  // -- Video
  buildSelect(
    $("videoFps"),
    [
      { value: "60", label: "60 FPS" },
      { value: "30", label: "30 FPS" },
      { value: "25", label: "25 FPS" },
    ],
    String(draft.video.fps),
    (value) => {
      draft.video.fps = Number(value);
      markDirty();
    }
  );

  bindCheckbox("cornersEnabled", () => draft.video.corners.enabled, (v) => (draft.video.corners.enabled = v));
  bindColor("cornersColor", "cornersColorSwatch", () => draft.video.corners.color, (v) => (draft.video.corners.color = v));
  bindFractionSlider("cornersOpacity", "cornersOpacityValue", () => draft.video.corners.opacity, (v) => (draft.video.corners.opacity = v));
  bindCheckbox("videoAudioSystem", () => draft.video.audio.system, (v) => (draft.video.audio.system = v));
  bindCheckbox("videoAudioMic", () => draft.video.audio.mic, (v) => (draft.video.audio.mic = v));
  bindCheckbox("videoAudioAskOnSave", () => draft.video.audio.askOnSave, (v) => (draft.video.audio.askOnSave = v));

  // -- Audio
  bindCheckbox("audioSystem", () => draft.audio.system, (v) => (draft.audio.system = v));
  bindCheckbox("audioMic", () => draft.audio.mic, (v) => (draft.audio.mic = v));

  const mics = await invoke<{ id: string; name: string }[]>("list_microphones").catch(() => []);
  buildSelect(
    $("micDevice"),
    [{ value: "default", label: "System default" }, ...mics.map((m) => ({ value: m.id, label: m.name }))],
    draft.audio.micDevice || "default",
    (value) => {
      // The video recorder shares this device, exactly as the C# window did.
      draft.audio.micDevice = value;
      draft.video.audio.micDevice = value;
      markDirty();
    }
  );

  markDirty();
}

async function save(): Promise<boolean> {
  try {
    await invoke("save_settings", { patch: draft });
    saved = JSON.parse(JSON.stringify(draft));
    markDirty();
    status.textContent = "Saved. Restart Reshot to apply.";
    return true;
  } catch (error) {
    status.textContent = String(error);
    return false;
  }
}

$<HTMLButtonElement>("apply").addEventListener("click", () => void save());

$<HTMLButtonElement>("ok").addEventListener("click", async () => {
  if (await save()) void getCurrentWindow().close();
});

const close = () => void getCurrentWindow().close();
$<HTMLButtonElement>("cancel").addEventListener("click", close);
$<HTMLButtonElement>("close").addEventListener("click", close);

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") close();
});

void load();
