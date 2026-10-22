const statusList = document.querySelector("#status");
const statusNote = document.querySelector("#status-note");
const devicesEl = document.querySelector("#devices");
const detail = document.querySelector("#detail");
const refresh = document.querySelector("#refresh");
const screenNote = document.querySelector("#screen-note");
const showScreen = document.querySelector("#show-screen");
const screenView = document.querySelector("#screen-view");
const phoneShell = document.querySelector("#phone-shell");
const placeholderHint = document.querySelector("#placeholder-hint");
const pillConnection = document.querySelector("#pill-connection");
const pillDevice = document.querySelector("#pill-device");
const toggleGuide = document.querySelector("#toggle-guide");
const runGuide = document.querySelector("#run-guide");
const workspace = document.querySelector("#workspace");
const remotePanel = document.querySelector("#remote-panel");
const restartScreen = document.querySelector("#restart-screen");
const phoneScreen = document.querySelector("#phone-screen");
const kbToggle = document.querySelector("#kb-toggle");
const wifiState = document.querySelector("#wifi-state");
const wifiPair = document.querySelector("#wifi-pair");
const MIRROR_PAGE = "/mirror.html?v=20261023_fix";
const MIRROR_API = "/mirror";

let selectedUdid = null;
let screenTimer = 0;
let transport = localStorage.getItem("handset-transport") === "wifi" ? "wifi" : "usb";
let wifiPaired = [];
let usbConnected = false;

document.querySelectorAll("[data-transport]").forEach((btn) => {
  btn.addEventListener("click", () => setTransport(btn.dataset.transport));
});
wifiPair.addEventListener("click", () => setUpWifi());
setTransport(transport, false);

function setTransport(value, reload = true) {
  transport = value === "wifi" ? "wifi" : "usb";
  localStorage.setItem("handset-transport", transport);
  document.querySelectorAll("[data-transport]").forEach((btn) => {
    const active = btn.dataset.transport === transport;
    btn.classList.toggle("is-active", active);
    btn.setAttribute("aria-checked", String(active));
  });
  if (reload) load();
}

function renderWifi() {
  const ready = selectedUdid ? wifiPaired.includes(selectedUdid) : wifiPaired.length > 0;
  wifiState.textContent = ready ? "✓ Wi‑Fi ready" : "Wi‑Fi not set up";
  wifiState.classList.toggle("ok", ready);
  wifiPair.hidden = ready;
  wifiPair.disabled = !usbConnected;
  wifiPair.title = usbConnected ? "One-time setup over the cable" : "Plug in the cable once to set up Wi‑Fi";
}

async function setUpWifi() {
  wifiPair.disabled = true;
  wifiPair.textContent = "Setting up…";
  try {
    const result = await postJson("/api/wifi/pair", { udid: selectedUdid });
    if (!result.ok) throw new Error(result.error || "Wi‑Fi setup failed.");
    screenNote.textContent = "Wi‑Fi is set up. You can unplug the cable and choose Wi‑Fi.";
    screenNote.className = "";
  } catch (error) {
    screenNote.textContent = error.message;
    screenNote.className = "warn";
  } finally {
    wifiPair.textContent = "Set up Wi‑Fi";
    load();
  }
}

const placeholder = document.querySelector("#placeholder");
placeholder?.addEventListener("click", () => {
  if (!showScreen.disabled) showPhone();
});

refresh.addEventListener("click", () => load());
showScreen.addEventListener("click", () => showPhone());
restartScreen.addEventListener("click", () => restartMirror());
wireRemoteControls();

const btnSettings = document.querySelector("#btn-settings");
const closeSettings = document.querySelector("#close-settings");
const controlPane = document.querySelector("#control-pane");

function toggleSettings(open) {
  if (!controlPane) return;
  const isHidden = controlPane.hidden;
  const shouldOpen = open !== undefined ? open : isHidden;
  controlPane.hidden = !shouldOpen;
  controlPane.classList.toggle("open", shouldOpen);
  controlPane.setAttribute("aria-hidden", String(!shouldOpen));
  btnSettings?.classList.toggle("active", shouldOpen);
  if (window.pywebview?.api?.set_drawer) {
    window.pywebview.api.set_drawer(shouldOpen);
  }
}

btnSettings?.addEventListener("click", () => toggleSettings());
closeSettings?.addEventListener("click", () => toggleSettings(false));
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !controlPane?.hidden) {
    toggleSettings(false);
  }
});

// Native Window Controls (Minimize, Close, Full Size)
document.querySelector("#win-min")?.addEventListener("click", () => {
  window.pywebview?.api?.minimize();
});
document.querySelector("#win-close")?.addEventListener("click", () => {
  if (window.pywebview?.api?.close) {
    window.pywebview.api.close();
  } else {
    window.close();
  }
});

const appRoot = document.querySelector("#app-root");
function toggleFullSize() {
  appRoot?.classList.toggle("is-fullsize");
  if (window.pywebview?.api?.toggle_fullscreen) {
    window.pywebview.api.toggle_fullscreen();
  } else if (!document.fullscreenElement) {
    document.documentElement.requestFullscreen?.().catch(() => {});
  } else {
    document.exitFullscreen?.().catch(() => {});
  }
  const notifyFrame = () => screenView?.contentWindow?.postMessage({ type: "resize" }, "*");
  notifyFrame();
  setTimeout(notifyFrame, 100);
  setTimeout(notifyFrame, 300);
}
document.querySelector("#btn-fullsize")?.addEventListener("click", toggleFullSize);

toggleGuide?.addEventListener("click", () => {
  const open = runGuide.hidden;
  runGuide.hidden = !open;
  toggleGuide.setAttribute("aria-expanded", String(open));
  if (open) runGuide.scrollIntoView({ behavior: "smooth", block: "start" });
});

async function load() {
  refresh.disabled = true;
  detail.hidden = true;
  detail.replaceChildren();
  try {
    const state = await getJson("/api/status");
    wifiPaired = state.wifi?.paired || [];
    usbConnected = (state.devices || []).length > 0;
    renderStatus(state);
    renderDevices(state.devices || []);
    renderWifi();
    renderScreen(state.screen);
    renderPills(state);
  } catch (error) {
    setPill(pillConnection, "Offline", "bad");
    statusNote.textContent = error.message;
    statusNote.className = "warn";
  } finally {
    refresh.disabled = false;
  }
}

function renderPills(state) {
  const driver = state.driver || {};
  const connected = (state.devices || []).length > 0;
  const overWifi = transport === "wifi" && wifiPaired.length > 0;
  if (state.screen?.phase === "running") {
    setPill(pillConnection, state.screen.transport === "wifi" ? "Mirroring · Wi‑Fi" : "Mirroring", "ok");
  } else if (overWifi) {
    setPill(pillConnection, "Wi‑Fi ready", "ok");
  } else if (state.error) {
    setPill(pillConnection, "Error", "bad");
  } else if (!driver.ready) {
    setPill(pillConnection, "Install Apple Devices", "warn");
  } else if (!connected) {
    setPill(pillConnection, "", "");
  } else if (state.developerMode === false) {
    setPill(pillConnection, "Developer Mode off", "warn");
  } else if (state.screen?.phase === "running") {
    setPill(pillConnection, "Mirroring", "ok");
  } else {
    setPill(pillConnection, "Ready", "ok");
  }

  if (!connected) {
    pillDevice.hidden = true;
    pillDevice.textContent = "";
    return;
  }
  pillDevice.hidden = false;
  const count = state.devices.length;
  pillDevice.textContent = count === 1 ? "1 device · USB" : `${count} devices`;
}

function setPill(el, text, tone) {
  if (!text) {
    el.hidden = true;
    el.textContent = "";
    return;
  }
  el.hidden = false;
  el.textContent = text;
  el.className = "pill" + (tone ? ` pill-${tone}` : "");
}

function renderStatus(state) {
  const library = state.library || {};
  const driver = state.driver || {};
  statusList.replaceChildren(
    ...row("Client", library.ready ? "ready" : "missing", library.ready),
    ...row("USB service", driver.ready ? "running" : "missing", driver.ready),
    ...row("Developer Mode", state.developerMode === false ? "off" : state.developerMode ? "on" : "—", state.developerMode !== false),
  );
  const connected = (state.devices || []).length > 0;
  if (state.developerMode === false) {
    statusNote.textContent =
      "Turn on Developer Mode on the iPhone (Settings → Privacy & Security), restart, then mirror again.";
    placeholderHint.textContent = "Developer Mode must be on before mirroring works.";
    return;
  }
  statusNote.textContent =
    state.error ||
    (connected ? "Device connected. Press Start mirroring." : driver.detail) ||
    "";
  statusNote.className = state.error || !driver.ready ? "warn" : "";
  if (!connected && (state.wifi?.paired || []).length) {
    statusNote.textContent = "No cable. Choose Wi‑Fi, keep the iPhone unlocked on the same network, and press Start mirroring.";
    statusNote.className = "";
    placeholderHint.textContent = "No cable needed — choose Wi‑Fi and press Start mirroring.";
  } else if (!connected) {
    placeholderHint.textContent = "Connect your iPhone with a data cable, unlock it, and tap Trust.";
  }
}

function renderDevices(list) {
  devicesEl.replaceChildren();
  if (!list.length && transport === "wifi" && wifiPaired.length) {
    list = wifiPaired.map((udid) => ({ udid, connection: "Wi‑Fi", wifiOnly: true }));
  }
  if (!list.length) {
    const item = document.createElement("li");
    item.className = "empty";
    item.textContent = "No iPhone detected.";
    devicesEl.append(item);
    selectedUdid = null;
    return;
  }
  if (list.length === 1) selectedUdid = list[0].udid;

  for (const device of list) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.className = "device" + (device.udid === selectedUdid ? " selected" : "");
    button.type = "button";
    const name = document.createElement("span");
    name.className = "device-name";
    name.textContent = device.udid.slice(0, 8) + "…";
    const meta = document.createElement("span");
    meta.className = "device-meta";
    meta.textContent = device.connection || "USB";
    button.append(name, meta);
    button.addEventListener("click", () => {
      selectedUdid = device.udid;
      devicesEl.querySelectorAll(".device").forEach((el) => el.classList.remove("selected"));
      button.classList.add("selected");
      if (!device.wifiOnly) openDevice(device.udid);
      renderWifi();
    });
    item.append(button);
    devicesEl.append(item);
  }
  if (list.length === 1 && !list[0].wifiOnly) openDevice(list[0].udid);
}

async function openDevice(udid) {
  detail.hidden = false;
  detail.textContent = "Reading device…";
  try {
    const device = await getJson(`/api/devices/${encodeURIComponent(udid)}`);
    renderDetail(device);
    if (device.info?.DeviceName) {
      pillDevice.hidden = false;
      pillDevice.textContent = device.info.DeviceName;
    }
  } catch (error) {
    detail.textContent = error.message;
  }
}

function renderDetail(device) {
  detail.replaceChildren();
  const title = document.createElement("h3");
  title.textContent = device.info?.DeviceName || "iPhone";
  detail.append(title);
  if (device.error) {
    const note = document.createElement("p");
    note.className = "warn";
    note.textContent = device.error;
    detail.append(note);
  }
  const list = document.createElement("dl");
  list.className = "kv";
  const labels = {
    ProductType: "Model",
    ProductVersion: "iOS",
    BuildVersion: "Build",
  };
  for (const [key, label] of Object.entries(labels)) {
    if (device.info && device.info[key] != null) list.append(...row(label, String(device.info[key]), true));
  }
  if (list.childElementCount) detail.append(list);
}

function renderScreen(screen) {
  if (!screen) return;
  const mirroring = screen.phase === "running";
  if (mirroring) {
    workspace.classList.add("is-live");
    phoneShell.classList.add("live");
    remotePanel.hidden = false;
    restartScreen.hidden = false;
    restartScreen.disabled = false;
    kbToggle.hidden = false;
    screenNote.textContent = "Mirroring — typing on your keyboard goes to the iPhone.";
    screenNote.className = "";
    showScreen.textContent = "Mirroring…";
    showScreen.disabled = true;
    if (screenView.getAttribute("src") !== MIRROR_PAGE) screenView.src = MIRROR_PAGE;
    screenView.hidden = false;
    return;
  }

  workspace.classList.remove("is-live");
  phoneShell.classList.remove("live");
  remotePanel.hidden = true;
  restartScreen.hidden = false;
  restartScreen.disabled = false;
  kbToggle.hidden = true;
  screenView.hidden = true;
  screenView.src = "about:blank";
  showScreen.textContent = "Start mirroring";
  showScreen.disabled = false;

  if (screen.phase === "starting") {
    screenNote.textContent = screen.note || "Starting stream… keep the iPhone unlocked.";
    screenNote.className = "";
    showScreen.disabled = true;
    showScreen.textContent = "Connecting…";
    restartScreen.disabled = true;
    return;
  }
  if (screen.error) {
    screenNote.textContent = screen.error;
    screenNote.className = "warn";
    return;
  }
  screenNote.textContent = "";
  screenNote.className = "";
}

async function showPhone() {
  showScreen.disabled = true;
  showScreen.textContent = "Connecting…";
  try {
    renderScreen(await postJson("/api/screen", { udid: selectedUdid, transport }));
    watchScreen();
    load();
  } catch (error) {
    screenNote.textContent = error.message;
    screenNote.className = "warn";
    showScreen.textContent = "Start mirroring";
    showScreen.disabled = false;
  }
}

function watchScreen() {
  window.clearInterval(screenTimer);
  screenTimer = window.setInterval(async () => {
    try {
      const screen = await getJson("/api/screen");
      renderScreen(screen);
      if (screen.phase !== "starting") {
        window.clearInterval(screenTimer);
        load();
      }
    } catch (error) {
      screenNote.textContent = error.message;
      window.clearInterval(screenTimer);
    }
  }, 1000);
}

function row(label, value, ok) {
  const term = document.createElement("dt");
  term.textContent = label;
  const data = document.createElement("dd");
  data.textContent = value;
  if (ok === true) data.className = "ok";
  if (ok === false) data.className = "warn";
  return [term, data];
}

async function getJson(url) {
  const response = await fetch(url);
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || `Request failed (${response.status})`);
  return body;
}

async function postJson(url, payload) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || `Request failed (${response.status})`);
  return body;
}

function mirrorMessage(cmd, extra = {}) {
  if (!screenView.contentWindow || screenView.hidden) return;
  screenView.contentWindow.postMessage({ source: "handset", cmd, ...extra }, window.location.origin);
}

function reloadMirrorIframe() {
  if (screenView.hidden) return;
  const stamp = `t=${Date.now()}`;
  screenView.src = "about:blank";
  requestAnimationFrame(() => {
    screenView.src = `${MIRROR_PAGE}?${stamp}`;
  });
}

async function restartMirror() {
  restartScreen.disabled = true;
  screenNote.textContent = "Restarting screen… keep the iPhone unlocked.";
  screenNote.className = "";
  try {
    await postJson("/api/screen/restart", { udid: selectedUdid });
    await new Promise((r) => setTimeout(r, 1500));
    reloadMirrorIframe();
    screenNote.textContent = "Restart sent — waiting for picture…";
    window.setTimeout(() => {
      if (workspace.classList.contains("is-live")) {
        screenNote.textContent = "Mirroring — use Restart screen if the picture stays black.";
      }
    }, 4000);
  } catch (error) {
    screenNote.textContent = error.message;
    screenNote.className = "warn";
  } finally {
    restartScreen.disabled = false;
  }
}

async function mirrorPost(path, payload) {
  try {
    await fetch(`${MIRROR_API}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
  } catch (_) {}
}

async function swipe(direction) {
  const yMid = 32768;
  const xStart = direction === "left" ? 65535 : 0;
  const xEnd = direction === "left" ? 10000 : 52000;
  const steps = 14;
  const dt = 16;
  await mirrorPost("/touch", { type: "contact", x: xStart, y: yMid });
  for (let i = 1; i <= steps; i++) {
    const x = Math.round(xStart + (xEnd - xStart) * (i / steps));
    await new Promise((r) => setTimeout(r, dt));
    await mirrorPost("/touch", { type: "contact", x, y: yMid });
  }
  await mirrorPost("/touch", { type: "release", x: xEnd, y: yMid });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Coordinates are HID units (0..65535). iOS only treats a swipe as a system
// gesture when the contact starts on the very edge of the screen.
const GESTURES = {
  back: { from: [0, 32768], to: [42000, 32768], steps: 14, dt: 16 },
  notifications: { from: [16000, 0], to: [16000, 42000], steps: 12, dt: 16 },
  control: { from: [58000, 0], to: [58000, 42000], steps: 12, dt: 16 },
};

// A swipe-up injected over USB is too slow for iOS to treat as the Home
// gesture, so Home and the app switcher use the Home button instead.
async function pressHome(times) {
  for (let i = 0; i < times; i++) {
    if (i) await sleep(120);
    await mirrorPost("/button", { name: "home", state: "press" });
  }
}

async function gesture(name) {
  if (name === "home") return pressHome(1);
  if (name === "switcher" || name === "recent") return pressHome(2);
  const g = GESTURES[name];
  if (!g) return;
  const [x0, y0] = g.from;
  const [x1, y1] = g.to;
  await mirrorPost("/touch", { type: "contact", x: x0, y: y0 });
  for (let i = 1; i <= g.steps; i++) {
    await sleep(g.dt);
    const t = i / g.steps;
    await mirrorPost("/touch", {
      type: "contact",
      x: Math.round(x0 + (x1 - x0) * t),
      y: Math.round(y0 + (y1 - y0) * t),
    });
  }
  if (g.hold) {
    await sleep(g.hold);
    await mirrorPost("/touch", { type: "contact", x: x1, y: y1 });
  }
  await mirrorPost("/touch", { type: "release", x: x1, y: y1 });
}

function toggleFullscreen() {
  if (document.fullscreenElement) {
    document.exitFullscreen().catch(() => {});
    return;
  }
  phoneScreen.requestFullscreen?.().catch(() => {});
}

window.handsetKeyboardOn = true;

function setKeyboard(on) {
  window.handsetKeyboardOn = on;
  kbToggle.setAttribute("aria-pressed", String(on));
  kbToggle.textContent = `⌨ Keyboard → iPhone: ${on ? "On" : "Off"}`;
  kbToggle.classList.toggle("is-on", on);
  if (!on) keyboard.releaseAll();
}

const keyboard = window.HandsetKeys.attach({
  post: (payload) => mirrorPost("/key", payload),
  enabled: () => window.handsetKeyboardOn && workspace.classList.contains("is-live"),
});

function wireRemoteControls() {
  kbToggle.addEventListener("click", () => setKeyboard(!window.handsetKeyboardOn));
  setKeyboard(true);
  // A focused button would swallow Space and Enter instead of typing them.
  document.addEventListener("pointerup", (e) => {
    const btn = e.target.closest?.("button");
    if (btn) btn.blur();
  });
  document.querySelectorAll("[data-gesture]").forEach((btn) => {
    btn.addEventListener("click", () => gesture(btn.dataset.gesture));
  });
  // Mouse Back button (button 3) and Alt+ArrowLeft shortcut
  window.addEventListener("pointerdown", (e) => {
    if (e.button === 3) {
      e.preventDefault();
      gesture("back");
    }
  });
  window.addEventListener("keydown", (e) => {
    if (e.altKey && e.key === "ArrowLeft") {
      e.preventDefault();
      gesture("back");
    }
  });
  document.querySelector("#fullscreen")?.addEventListener("click", toggleFullscreen);
  document.querySelector("#fs-exit")?.addEventListener("click", toggleFullscreen);
  document.addEventListener("fullscreenchange", () => {
    const btn = document.querySelector("#fullscreen");
    if (btn) btn.textContent = document.fullscreenElement ? "Exit full screen" : "⛶ Full screen";
  });
  document.querySelectorAll("[data-btn]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const name = btn.dataset.btn;
      if (name) mirrorPost("/button", { name, state: "press" });
    });
  });
  document.querySelectorAll("[data-swipe]").forEach((btn) => {
    btn.addEventListener("click", () => swipe(btn.dataset.swipe));
  });
  document.querySelector("#zoom-in")?.addEventListener("click", () => mirrorMessage("zoom-in"));
  document.querySelector("#zoom-out")?.addEventListener("click", () => mirrorMessage("zoom-out"));
  document.querySelector("#zoom-reset")?.addEventListener("click", () => mirrorMessage("zoom-reset"));
  document.querySelector("#rotate-cw")?.addEventListener("click", () => mirrorMessage("rotate", { deg: 90 }));
  document.querySelector("#rotate-ccw")?.addEventListener("click", () => mirrorMessage("rotate", { deg: -90 }));
}

load();
