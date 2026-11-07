// ======================================================================
// Tahap 70: Global JS Error Tracker & Diagnostics
// ======================================================================
window.__jsErrors = window.__jsErrors || [];
window.addEventListener("error", (e) => {
  const msg = (e && e.message) || String(e);
  window.__jsErrors.push(msg);
  console.error("[iPhoneLink-UncaughtError]", msg);
});
window.addEventListener("unhandledrejection", (e) => {
  const msg = (e && e.reason && (e.reason.message || String(e.reason))) || "Unhandled Rejection";
  window.__jsErrors.push(msg);
  console.error("[iPhoneLink-UnhandledRejection]", msg);
});

// Alias & dual-binding hook for #status-pill and #pill-connection
(() => {
  const origQS = Document.prototype.querySelector;
  Document.prototype.querySelector = function (sel) {
    if (sel === "#pill-connection" || sel === "#status-pill") {
      return origQS.call(this, "#status-pill") || origQS.call(this, "#pill-connection");
    }
    return origQS.call(this, sel);
  };
})();

// ======================================================================
// Tahap 70: Global DOM Element References (Declared Top-Level to Eliminate TDZ)
// ======================================================================
const appRoot = document.querySelector("#app-root");
const appBar = document.querySelector(".app-bar");
const controlPane = document.querySelector("#control-pane");
const sideDrawer = document.querySelector(".side-drawer");
const btnSettings = document.querySelector("#btn-settings");
const closeSettings = document.querySelector("#close-settings");
const btnRotateLandscape = document.querySelector("#btn-rotate-landscape");
const rotateModeBadge = document.querySelector("#rotate-mode-badge");

const statusList = document.querySelector("#status");
const statusNote = document.querySelector("#status-note");
const devicesEl = document.querySelector("#devices");
const detail = document.querySelector("#detail");
const refresh = document.querySelector("#refresh");
const screenNote = document.querySelector("#screen-note");
const showScreen = document.querySelector("#show-screen");
const screenView = document.querySelector("#screen-view");
const phoneShell = document.querySelector("#phone-shell");
const phoneScreen = document.querySelector("#phone-screen");
const placeholderHint = document.querySelector("#placeholder-hint");
const pillConnection = document.querySelector("#pill-connection");
const pillDevice = document.querySelector("#pill-device");
const toggleGuide = document.querySelector("#toggle-guide");
const runGuide = document.querySelector("#run-guide");
const workspace = document.querySelector("#workspace");
const remotePanel = document.querySelector("#remote-panel");
const restartScreen = document.querySelector("#restart-screen");
const stopScreen = document.querySelector("#stop-screen");
const screenErrorBanner = document.querySelector("#screen-error-banner");
const screenEmpty = document.querySelector("#screen-empty");
const placeholder = screenEmpty;
const kbToggle = document.querySelector("#kb-toggle");
const wifiState = document.querySelector("#wifi-state");
const wifiPair = document.querySelector("#wifi-pair");
const wifiScan = document.querySelector("#wifi-scan");

const btnWakeUnlock = document.querySelector("#btn-wake-unlock");
const btnLockIphone = document.querySelector("#btn-lock-iphone");
const btnStealthMode = document.querySelector("#btn-stealth-mode");
const stealthModeHint = document.querySelector("#stealth-mode-hint");
const inputUnlockPin = document.querySelector("#input-unlock-pin");
const btnTogglePinVisibility = document.querySelector("#btn-toggle-pin-visibility");
const btnWipePrivacy = document.querySelector("#btn-wipe-privacy");

// Strict hide of stopScreen on boot (Tahap 70: 0% visual flash / zero event blocking)
if (stopScreen) {
  stopScreen.hidden = true;
  stopScreen.setAttribute("hidden", "");
  stopScreen.classList.add("is-hidden");
  stopScreen.style.setProperty("display", "none", "important");
}
if (screenEmpty) {
  screenEmpty.hidden = false;
  screenEmpty.removeAttribute("hidden");
  screenEmpty.style.display = "flex";
}

// ======================================================================
// Tahap 70: Global State Variables (Declared Top-Level to Eliminate TDZ)
// ======================================================================
const MIRROR_PAGE = "/mirror.html?v=20261101_tahap70_hotfix_anti_tdz";
const MIRROR_API = "/mirror";

let isLandscapeMode = false;
window.isLandscapeMode = false;
let isDrawerOpen = false;
let isFullSize = false;
window.isFullSize = false;

let lastDeviceAspect = null;
let lastDeviceDimensions = null;

let autoRotateEnabled = false;
try {
  autoRotateEnabled = localStorage.getItem("pane-auto-rotate") === "true";
} catch (_) { }

let currentTheme = "dark";
try {
  currentTheme = localStorage.getItem("pane-theme") || "dark";
} catch (_) { }

let currentFitMode = "autofit";
try {
  currentFitMode = localStorage.getItem("pane-fit-mode") || "autofit";
} catch (_) { }

let currentTrackpadSpeed = 100;
try {
  const savedSpeed = parseInt(localStorage.getItem("pane-trackpad-speed"), 10);
  if (!isNaN(savedSpeed) && savedSpeed >= 30 && savedSpeed <= 200) {
    currentTrackpadSpeed = savedSpeed;
  }
} catch (_) { }

let currentKeyboardSettings = {
  crossPlatformShortcuts: true,
  pastePcClipboard: true,
};
try {
  const sShortcuts = localStorage.getItem("pane-shortcuts");
  if (sShortcuts !== null) currentKeyboardSettings.crossPlatformShortcuts = sShortcuts === "true";
  const sPaste = localStorage.getItem("pane-paste-pc");
  if (sPaste !== null) currentKeyboardSettings.pastePcClipboard = sPaste === "true";
} catch (_) { }
window.HandsetKeyboardSettings = currentKeyboardSettings;

const DEFAULT_CROP = { top: 0, bottom: 0, left: 0, right: 0 };
let currentCrop = { ...DEFAULT_CROP };
try {
  const savedCrop = JSON.parse(localStorage.getItem("pane-crop-edges") || "{}");
  currentCrop = {
    top: Math.max(0, Math.min(120, parseInt(savedCrop.top, 10) || 0)),
    bottom: Math.max(0, Math.min(120, parseInt(savedCrop.bottom, 10) || 0)),
    left: Math.max(0, Math.min(120, parseInt(savedCrop.left, 10) || 0)),
    right: Math.max(0, Math.min(120, parseInt(savedCrop.right, 10) || 0)),
  };
} catch (_) { }

let selectedUdid = null;
let transport = "usb";
let screenTimer = 0;
let wifiPaired = [];
let usbConnected = false;
let currentStealthActive = false;
let currentWallpaperSig = null;
let currentWifiState = {};
let currentAudioState = { enabled: false, volume: 100 };
const _cachedDeviceDetails = new Map();


// Immediate Early Window Controls & Drawer Toggle (Anti-Freeze Guard)
function toggleSettings(open) {
  const controlPane = document.querySelector("#control-pane");
  const btnSettings = document.querySelector("#btn-settings");
  const appRoot = document.querySelector("#app-root");
  if (!controlPane) return;
  const isHidden = controlPane.hidden;
  const shouldOpen = open !== undefined ? open : isHidden;
  isDrawerOpen = shouldOpen;
  window.isDrawerOpen = shouldOpen;
  controlPane.hidden = !shouldOpen;
  controlPane.classList.toggle("open", shouldOpen);
  controlPane.classList.toggle("is-open", shouldOpen);
  controlPane.setAttribute("aria-hidden", String(!shouldOpen));
  btnSettings?.classList.toggle("active", shouldOpen);
  appRoot?.classList.toggle("drawer-open", shouldOpen);

  // Tahap 91: Dispatch resize to mirror iframe & window so layout updates smoothly
  window.dispatchEvent(new Event("resize"));
  const iframe = document.getElementById("screen-view");
  if (iframe && iframe.contentWindow) {
    [0, 60, 150].forEach((ms) => {
      setTimeout(() => {
        try {
          iframe.contentWindow.dispatchEvent(new Event("resize"));
          iframe.contentWindow.postMessage({ type: "resize" }, "*");
        } catch (_) { }
      }, ms);
    });
  }

  try {
    const isLand = Boolean(window.isLandscapeMode || (typeof isLandscapeMode !== "undefined" && isLandscapeMode) || appRoot?.classList.contains("is-landscape"));
    if (window.pywebview?.api?.set_orientation) {
      window.pywebview.api.set_orientation(isLand, shouldOpen);
    } else if (window.pywebview?.api?.set_drawer) {
      window.pywebview.api.set_drawer(shouldOpen);
    }
  } catch (_) { }
}
window.toggleSettings = toggleSettings;

function setupEarlyControls() {
  try {
    const btnClose = document.querySelector("#win-close");
    if (btnClose && !btnClose._earlyBound) {
      btnClose._earlyBound = true;
      btnClose.addEventListener("click", () => {
        if (window.pywebview?.api?.close) {
          window.pywebview.api.close();
        } else {
          window.close();
        }
      });
    }

    const btnMin = document.querySelector("#win-min");
    if (btnMin && !btnMin._earlyBound) {
      btnMin._earlyBound = true;
      btnMin.addEventListener("click", () => {
        window.pywebview?.api?.minimize?.();
      });
      btnMin.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (window.pywebview?.api?.minimize_to_tray) {
          window.pywebview.api.minimize_to_tray();
        } else {
          window.pywebview?.api?.minimize?.();
        }
      });
    }

    const btnHide = document.querySelector("#btn-hide-to-tray");
    if (btnHide && !btnHide._earlyBound) {
      btnHide._earlyBound = true;
      btnHide.addEventListener("click", () => {
        if (window.pywebview?.api?.minimize_to_tray) {
          window.pywebview.api.minimize_to_tray();
        } else {
          window.pywebview?.api?.minimize?.();
        }
      });
    }

    const btnSet = document.querySelector("#btn-settings");
    if (btnSet && !btnSet._earlyBound) {
      btnSet._earlyBound = true;
      btnSet.addEventListener("click", (e) => {
        e.preventDefault();
        toggleSettings();
      });
    }

    const btnCloseSet = document.querySelector("#close-settings");
    if (btnCloseSet && !btnCloseSet._earlyBound) {
      btnCloseSet._earlyBound = true;
      btnCloseSet.addEventListener("click", (e) => {
        e.preventDefault();
        toggleSettings(false);
      });
    }
  } catch (e) {
    console.error("[Tahap65-EarlyControlsError]", e);
  }
}


function applyCleanSlatePinGuard(settingsPayload) {
  if (!settingsPayload) return;
  const currentFolderInstanceId = String(settingsPayload.folderInstanceId || "");
  const isFreshFolder = Boolean(settingsPayload.isFreshFolder);
  const pinEl = document.querySelector("#input-unlock-pin");
  try {
    const savedInstanceId = localStorage.getItem("iphone_link_folder_instance_id") || "";
    if (isFreshFolder || !currentFolderInstanceId || savedInstanceId !== currentFolderInstanceId) {
      localStorage.removeItem("iphone_link_unlock_pin");
      localStorage.removeItem("pane-unlock-pin");
      if (pinEl) pinEl.value = "";
      if (currentFolderInstanceId) {
        localStorage.setItem("iphone_link_folder_instance_id", currentFolderInstanceId);
      }
    } else {
      const savedPin =
        localStorage.getItem("iphone_link_unlock_pin") ||
        localStorage.getItem("pane-unlock-pin") ||
        "";
      if (pinEl) pinEl.value = savedPin;
    }
  } catch (_) {
    if (pinEl) pinEl.value = "";
  }
}

// Persistent Settings via /api/settings (Tahap 69: Isolated per-property safe parsing)
async function fetchUserSettings() {
  try {
    const res = await getJson("/api/settings");
    if (!res) return;
    try { applyCleanSlatePinGuard(res); } catch (_) { }
    if (res.crop) {
      try {
        currentCrop = {
          top: Math.max(0, Math.min(120, parseInt(res.crop.top, 10) || 0)),
          bottom: Math.max(0, Math.min(120, parseInt(res.crop.bottom, 10) || 0)),
          left: Math.max(0, Math.min(120, parseInt(res.crop.left, 10) || 0)),
          right: Math.max(0, Math.min(120, parseInt(res.crop.right, 10) || 0)),
        };
        localStorage.setItem("pane-crop-edges", JSON.stringify(currentCrop));
        applyCrop(currentCrop, true);
      } catch (_) { }
    }
    if (res.fitMode) {
      try { applyFitMode(res.fitMode); } catch (_) { }
    }
    if (res.autoRotate !== undefined) {
      try { applyAutoRotateSetting(res.autoRotate, false); } catch (_) { }
    }
    if (res.theme) {
      try { applyTheme(res.theme); } catch (_) { }
    }
    if (res.trackpadSpeed !== undefined) {
      try { applyTrackpadSpeed(res.trackpadSpeed, true); } catch (_) { }
    }
    if (
      res.crossPlatformShortcuts !== undefined ||
      res.pastePcClipboard !== undefined
    ) {
      try {
        applyKeyboardSettings(
          {
            crossPlatformShortcuts: res.crossPlatformShortcuts,
            pastePcClipboard: res.pastePcClipboard,
          },
          true
        );
      } catch (_) { }
    }
    if (res.audioVolume !== undefined) {
      try {
        const volSlider = document.querySelector("#audio-vol-slider");
        const volVal = document.querySelector("#audio-vol-val");
        if (volSlider) volSlider.value = String(res.audioVolume);
        if (volVal) volVal.textContent = `${res.audioVolume}%`;
      } catch (_) { }
    }
  } catch (err) {
    console.warn("fetchUserSettings error:", err);
  }
}

async function saveUserSettings() {
  const btnSave = document.querySelector("#btn-save-crop");
  try {
    if (btnSave) {
      btnSave.disabled = true;
      btnSave.textContent = "Saving…";
    }
    localStorage.setItem("pane-crop-edges", JSON.stringify(currentCrop));
    localStorage.setItem("pane-fit-mode", currentFitMode);
    localStorage.setItem("pane-theme", currentTheme);

    await postJson("/api/settings", {
      crop: currentCrop,
      fitMode: currentFitMode,
      theme: currentTheme,
      trackpadSpeed: currentTrackpadSpeed,
      crossPlatformShortcuts: currentKeyboardSettings.crossPlatformShortcuts,
      pastePcClipboard: currentKeyboardSettings.pastePcClipboard,
    });

    applyCrop(currentCrop, true);

    if (btnSave) {
      btnSave.textContent = "✓ Crop Saved!";
      btnSave.classList.add("is-saved");
      setTimeout(() => {
        btnSave.textContent = "💾 Save Crop";
        btnSave.classList.remove("is-saved");
        btnSave.disabled = false;
      }, 1500);
    }
  } catch (err) {
    console.warn("saveUserSettings error:", err);
    if (btnSave) {
      btnSave.textContent = "Error saving";
      btnSave.disabled = false;
    }
  }
}

// Audio Sink UI & Management

function renderAudio(audio) {
  if (!audio) return;
  currentAudioState = { ...currentAudioState, ...audio };

  const isEnabled = Boolean(currentAudioState.enabled || currentAudioState.active);
  const btnTop = document.querySelector("#btn-audio-top");
  const iconOn = btnTop?.querySelector(".audio-icon-on");
  const iconOff = btnTop?.querySelector(".audio-icon-off");
  if (btnTop) {
    btnTop.classList.toggle("is-active", isEnabled);
    if (iconOn && iconOff) {
      iconOn.hidden = !isEnabled;
      iconOff.hidden = isEnabled;
      iconOn.style.display = isEnabled ? "block" : "none";
      iconOff.style.display = isEnabled ? "none" : "block";
    }
  }

  const badge = document.querySelector("#audio-status-badge");
  if (badge) {
    badge.classList.toggle("active", isEnabled);
    if (isEnabled) {
      badge.textContent = `● Active (${currentAudioState.device || "PC"})`;
    } else {
      badge.textContent = "○ Off / Muted";
    }
  }

  const btnDrawer = document.querySelector("#btn-audio-drawer");
  const textDrawer = document.querySelector("#audio-toggle-text");
  const iconDrawer = document.querySelector("#audio-toggle-icon");
  if (btnDrawer) {
    btnDrawer.classList.toggle("is-active", isEnabled);
    if (textDrawer) {
      textDrawer.textContent = isEnabled ? "Disable Audio Sink (Active)" : "Enable Audio Sink";
    }
    if (iconDrawer) {
      iconDrawer.textContent = isEnabled ? "🔇" : "🔊";
    }
  }

  const volSlider = document.querySelector("#audio-vol-slider");
  const volVal = document.querySelector("#audio-vol-val");
  if (volSlider && currentAudioState.volume !== undefined) {
    volSlider.value = String(currentAudioState.volume);
  }
  if (volVal && currentAudioState.volume !== undefined) {
    volVal.textContent = `${currentAudioState.volume}%`;
  }
}

async function toggleAudio(forcedState) {
  try {
    const shouldEnable = forcedState !== undefined ? forcedState : !currentAudioState.enabled;
    const res = await postJson("/api/audio/toggle", { enabled: shouldEnable });
    renderAudio(res);
  } catch (err) {
    console.warn("toggleAudio error:", err);
  }
}
window.trayToggleAudio = () => toggleAudio();

async function setAudioVolume(vol) {
  try {
    const res = await postJson("/api/audio/volume", { volume: parseInt(vol, 10) || 100 });
    if (res && res.volume !== undefined) {
      currentAudioState.volume = res.volume;
      const volVal = document.querySelector("#audio-vol-val");
      if (volVal) volVal.textContent = `${res.volume}%`;
    }
  } catch (err) {
    console.warn("setAudioVolume error:", err);
  }
}

function initAudioControls() {
  document.querySelector("#btn-audio-top")?.addEventListener("click", () => toggleAudio());
  document.querySelector("#btn-audio-drawer")?.addEventListener("click", () => toggleAudio());

  const volSlider = document.querySelector("#audio-vol-slider");
  const volVal = document.querySelector("#audio-vol-val");
  volSlider?.addEventListener("input", (e) => {
    if (volVal) volVal.textContent = `${e.target.value}%`;
  });
  volSlider?.addEventListener("change", (e) => {
    setAudioVolume(e.target.value);
  });

  document.querySelector("#btn-audio-pair")?.addEventListener("click", async () => {
    try {
      await postJson("/api/audio/pair", {});
    } catch (_) { }
  });
}

function syncThemeToIframe() {
  try {
    if (screenView && screenView.contentDocument) {
      screenView.contentDocument.documentElement.setAttribute("data-theme", currentTheme);
      screenView.contentDocument.body?.setAttribute("data-theme", currentTheme);
    }
  } catch (_) { }
  try {
    screenView?.contentWindow?.postMessage({ type: "set-theme", theme: currentTheme }, "*");
  } catch (_) { }
}

function applyTheme(theme) {
  currentTheme = theme === "light" ? "light" : "dark";
  document.documentElement.setAttribute("data-theme", currentTheme);
  const appEl = document.querySelector("#app-root");
  appEl?.setAttribute("data-theme", currentTheme);
  localStorage.setItem("pane-theme", currentTheme);

  document.querySelectorAll("[data-theme-choice]").forEach((btn) => {
    const active = btn.dataset.themeChoice === currentTheme;
    btn.classList.toggle("is-active", active);
    btn.setAttribute("aria-checked", String(active));
  });

  syncThemeToIframe();

  try {
    if (window.pywebview?.api?.set_theme) {
      window.pywebview.api.set_theme(currentTheme);
    }
  } catch (_) { }
}


// Tahap 80: Toast Notification is completely disabled (no-op) for 100% clean display
function showToastNotification(text, duration = 2200) {
  return;
}
window.showToastNotification = showToastNotification;

function updateRotateModeBadge(isOn) {
  const badge = document.querySelector("#rotate-mode-badge");
  const btnRot = document.querySelector("#btn-rotate-landscape");
  if (badge) {
    badge.textContent = isOn ? "Auto" : "";
    if (!isOn) {
      badge.style.setProperty("display", "none", "important");
    } else {
      badge.style.display = "";
    }
  }
  if (btnRot) {
    btnRot.classList.toggle("is-auto-rotate-on", Boolean(isOn));
  }
}
window.updateRotateModeBadge = updateRotateModeBadge;



function applyAutoRotateSetting(enabled, userTriggered = false) {
  autoRotateEnabled = Boolean(enabled);
  try {
    localStorage.setItem("pane-auto-rotate", String(autoRotateEnabled));
  } catch (_) { }
  document.querySelectorAll("[data-auto-rotate-choice]").forEach((btn) => {
    const choice = (btn.dataset.autoRotateChoice || "").toLowerCase();
    const isThisActive = (choice === "on" || choice === "true" || choice === "1") ? autoRotateEnabled : !autoRotateEnabled;
    btn.classList.toggle("is-active", isThisActive);
    btn.classList.toggle("active", isThisActive);
    btn.setAttribute("aria-checked", String(isThisActive));
  });
  updateRotateModeBadge(autoRotateEnabled);
  if (userTriggered) {
    showToastNotification(autoRotateEnabled ? "🔄 Auto-Rotate: ON" : "🔄 Auto-Rotate: OFF");
  }
  try {
    screenView?.contentWindow?.postMessage({
      type: "set-auto-rotate",
      enabled: autoRotateEnabled,
    }, "*");
  } catch (_) { }
}

function initAutoRotateControls() {
  document.querySelectorAll("[data-auto-rotate-choice]").forEach((btn) => {
    btn.onclick = (e) => {
      e.preventDefault();
      const choice = (btn.dataset.autoRotateChoice || "").toLowerCase();
      const targetEnabled = (choice === "on" || choice === "true" || choice === "1");
      applyAutoRotateSetting(targetEnabled, true);
    };
  });
  applyAutoRotateSetting(autoRotateEnabled, false);
}

function applyFitMode(mode) {
  const choice = (mode === "contain" || mode === "fill") ? mode : "autofit";
  currentFitMode = choice;
  try {
    localStorage.setItem("pane-fit-mode", currentFitMode);
  } catch (_) { }
  document.querySelectorAll("[data-fit-choice]").forEach((btn) => {
    const active = btn.dataset.fitChoice === currentFitMode;
    btn.classList.toggle("is-active", active);
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-checked", String(active));
  });
  if (currentFitMode === "autofit" && lastDeviceDimensions) {
    handleDeviceDimensions(lastDeviceDimensions.width, lastDeviceDimensions.height);
  }
  try {
    screenView?.contentWindow?.postMessage({ type: "set-fit-mode", mode: currentFitMode }, "*");
  } catch (_) { }
}

function applyCrop(crop, syncInputs = false) {
  currentCrop = {
    top: Math.max(0, Math.min(120, parseInt(crop?.top, 10) || 0)),
    bottom: Math.max(0, Math.min(120, parseInt(crop?.bottom, 10) || 0)),
    left: Math.max(0, Math.min(120, parseInt(crop?.left, 10) || 0)),
    right: Math.max(0, Math.min(120, parseInt(crop?.right, 10) || 0)),
  };
  localStorage.setItem("pane-crop-edges", JSON.stringify(currentCrop));

  if (syncInputs) {
    ["top", "bottom", "left", "right"].forEach((side) => {
      const slider = document.querySelector(`#crop-${side}-slider`);
      const numInput = document.querySelector(`#crop-${side}-num`);
      if (slider) slider.value = String(currentCrop[side]);
      if (numInput) numInput.value = String(currentCrop[side]);
    });
  }

  try {
    screenView?.contentWindow?.postMessage({ type: "set-crop", crop: currentCrop }, "*");
  } catch (_) { }
}

function initCropControls() {
  ["top", "bottom", "left", "right"].forEach((side) => {
    const slider = document.querySelector(`#crop-${side}-slider`);
    const numInput = document.querySelector(`#crop-${side}-num`);

    if (slider) {
      slider.value = String(currentCrop[side]);
      slider.addEventListener("input", (e) => {
        const val = parseInt(e.target.value, 10) || 0;
        if (numInput) numInput.value = String(val);
        currentCrop[side] = val;
        applyCrop(currentCrop, false);
      });
    }

    if (numInput) {
      numInput.value = String(currentCrop[side]);
      const handleNumChange = (e) => {
        let val = parseInt(e.target.value, 10);
        if (isNaN(val)) val = 0;
        val = Math.max(0, Math.min(120, val));
        e.target.value = String(val);
        if (slider) slider.value = String(val);
        currentCrop[side] = val;
        applyCrop(currentCrop, false);
      };
      numInput.addEventListener("input", handleNumChange);
      numInput.addEventListener("change", handleNumChange);
    }
  });

  const btnSave = document.querySelector("#btn-save-crop");
  btnSave?.addEventListener("click", saveUserSettings);

  const btnReset = document.querySelector("#btn-reset-crop");
  btnReset?.addEventListener("click", async () => {
    applyCrop(DEFAULT_CROP, true);
    try {
      await postJson("/api/settings", { crop: DEFAULT_CROP });
    } catch (_) { }
  });
}

function applyTrackpadSpeed(speed, syncInputs = false) {
  let val = parseInt(speed, 10);
  if (isNaN(val)) val = 100;
  val = Math.max(30, Math.min(200, val));
  currentTrackpadSpeed = val;
  localStorage.setItem("pane-trackpad-speed", String(currentTrackpadSpeed));

  if (syncInputs) {
    const slider = document.querySelector("#trackpad-speed-slider");
    const numInput = document.querySelector("#trackpad-speed-num");
    if (slider) slider.value = String(currentTrackpadSpeed);
    if (numInput) numInput.value = String(currentTrackpadSpeed);
  }

  document.querySelectorAll("[data-speed-choice]").forEach((btn) => {
    const choice = parseInt(btn.dataset.speedChoice, 10);
    const active = choice === currentTrackpadSpeed;
    btn.classList.toggle("is-active", active);
    btn.setAttribute("aria-checked", String(active));
  });

  try {
    screenView?.contentWindow?.postMessage(
      { type: "set-trackpad-speed", speed: currentTrackpadSpeed },
      "*"
    );
  } catch (_) { }
}

async function saveTrackpadSpeed() {
  const btnSave = document.querySelector("#btn-save-speed");
  try {
    if (btnSave) {
      btnSave.disabled = true;
      btnSave.textContent = "Saving…";
    }
    localStorage.setItem("pane-trackpad-speed", String(currentTrackpadSpeed));

    await postJson("/api/settings", {
      trackpadSpeed: currentTrackpadSpeed,
    });

    applyTrackpadSpeed(currentTrackpadSpeed, true);

    if (btnSave) {
      btnSave.textContent = "✓ Speed Saved!";
      btnSave.classList.add("is-saved");
      setTimeout(() => {
        btnSave.textContent = "💾 Save Speed";
        btnSave.classList.remove("is-saved");
        btnSave.disabled = false;
      }, 1500);
    }
  } catch (err) {
    console.warn("saveTrackpadSpeed error:", err);
    if (btnSave) {
      btnSave.textContent = "Error saving";
      btnSave.disabled = false;
    }
  }
}

function initTrackpadSpeedControls() {
  const slider = document.querySelector("#trackpad-speed-slider");
  const numInput = document.querySelector("#trackpad-speed-num");

  if (slider) {
    slider.value = String(currentTrackpadSpeed);
    slider.addEventListener("input", (e) => {
      const val = parseInt(e.target.value, 10) || 100;
      if (numInput) numInput.value = String(val);
      applyTrackpadSpeed(val, false);
    });
  }

  if (numInput) {
    numInput.value = String(currentTrackpadSpeed);
    const handleNumChange = (e) => {
      let val = parseInt(e.target.value, 10);
      if (isNaN(val)) val = 100;
      val = Math.max(30, Math.min(200, val));
      e.target.value = String(val);
      if (slider) slider.value = String(val);
      applyTrackpadSpeed(val, false);
    };
    numInput.addEventListener("input", handleNumChange);
    numInput.addEventListener("change", handleNumChange);
  }

  document.querySelectorAll("[data-speed-choice]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const choice = parseInt(btn.dataset.speedChoice, 10);
      if (!isNaN(choice)) {
        applyTrackpadSpeed(choice, true);
      }
    });
  });

  const btnSave = document.querySelector("#btn-save-speed");
  btnSave?.addEventListener("click", saveTrackpadSpeed);
}

function applyKeyboardSettings(settings, syncInputs = false) {
  if (settings) {
    if (settings.crossPlatformShortcuts !== undefined) {
      currentKeyboardSettings.crossPlatformShortcuts = Boolean(settings.crossPlatformShortcuts);
    }
    if (settings.pastePcClipboard !== undefined) {
      currentKeyboardSettings.pastePcClipboard = Boolean(settings.pastePcClipboard);
    }
  }

  window.HandsetKeyboardSettings = currentKeyboardSettings;
  localStorage.setItem("pane-shortcuts", String(currentKeyboardSettings.crossPlatformShortcuts));
  localStorage.setItem("pane-paste-pc", String(currentKeyboardSettings.pastePcClipboard));

  if (syncInputs) {
    document.querySelectorAll("[data-shortcut-choice]").forEach((btn) => {
      const active = (btn.dataset.shortcutChoice === "true") === currentKeyboardSettings.crossPlatformShortcuts;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-checked", String(active));
    });

    document.querySelectorAll("[data-paste-pc-choice]").forEach((btn) => {
      const active = (btn.dataset.pastePcChoice === "true") === currentKeyboardSettings.pastePcClipboard;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-checked", String(active));
    });
  }

  try {
    screenView?.contentWindow?.postMessage(
      { type: "set-keyboard-settings", settings: currentKeyboardSettings },
      "*"
    );
  } catch (_) { }
}

async function saveKeyboardSettings() {
  try {
    await postJson("/api/settings", {
      crossPlatformShortcuts: currentKeyboardSettings.crossPlatformShortcuts,
      pastePcClipboard: currentKeyboardSettings.pastePcClipboard,
    });
  } catch (err) {
    console.warn("saveKeyboardSettings error:", err);
  }
}

function initKeyboardSettingsControls() {
  document.querySelectorAll("[data-shortcut-choice]").forEach((btn) => {
    btn.addEventListener("click", () => {
      applyKeyboardSettings({ crossPlatformShortcuts: btn.dataset.shortcutChoice === "true" }, true);
      saveKeyboardSettings();
    });
  });

  document.querySelectorAll("[data-paste-pc-choice]").forEach((btn) => {
    btn.addEventListener("click", () => {
      applyKeyboardSettings({ pastePcClipboard: btn.dataset.pastePcChoice === "true" }, true);
      saveKeyboardSettings();
    });
  });

  const COPY_IPHONE_LABEL = "📥 Copy iPhone Text (Alt+C)";
  const PASTE_PC_LABEL = "📋 Paste PC Text (Alt+V)";
  const CLEAR_COPY_CACHE_LABEL = "🧹 Clear Copy Cache (Alt+R)";
  const SCAN_QR_LABEL = "🔗 Grab Link (Alt+B)";

  window.triggerCopyIphoneTextWithFeedback = async function () {
    const btnCopyIphone = document.querySelector("#btn-copy-iphone-now");
    if (btnCopyIphone) {
      btnCopyIphone.disabled = true;
      btnCopyIphone.textContent = "⏳ Copying…";
    }
    try {
      if (typeof window.HandsetKeys?.flushTypedQueue === "function") {
        await window.HandsetKeys.flushTypedQueue();
      }
      const frameDataUrl = window.HandsetKeys?.captureMirrorCanvasDataUrl ? window.HandsetKeys.captureMirrorCanvasDataUrl() : null;
      const frameHistory = window.HandsetKeys?.getMirrorFrameHistory ? window.HandsetKeys.getMirrorFrameHistory() : [];
      let res;
      if (window.HandsetKeys?.triggerShortcut) {
        res = await window.HandsetKeys.triggerShortcut("copy", frameDataUrl, true, frameHistory);
      } else {
        res = await postJson("/api/keyboard/shortcut", { action: "copy", frameDataUrl, frameHistory, waitResult: true });
      }
      if (res?.autoClearedCache === true) {
        if (typeof window.clearOcrSessionMemory === "function") window.clearOcrSessionMemory();
        else window._ViewportHistory = [];
      }
      if (res?.text) {
        // Server has already written text directly to Windows PC clipboard via set_windows_clipboard_text().
        // Do NOT call navigator.clipboard.writeText() to avoid changing Windows clipboard sequence number!
        if (btnCopyIphone) {
          if (res?.source === "qr_bridge" || res?.source === "url_scanner") {
            btnCopyIphone.textContent = "✓ Link Copied to PC!";
          } else {
            btnCopyIphone.textContent = "✓ Copied to PC!";
          }
        }
      } else {
        if (btnCopyIphone) btnCopyIphone.textContent = "✓ Copied (iOS)";
      }
      if (btnCopyIphone) btnCopyIphone.classList.add("is-saved");
      return res;
    } catch (_) {
      if (btnCopyIphone) btnCopyIphone.textContent = "Copy Failed";
    } finally {
      setTimeout(() => {
        if (btnCopyIphone) {
          btnCopyIphone.textContent = COPY_IPHONE_LABEL;
          btnCopyIphone.classList.remove("is-saved");
          btnCopyIphone.disabled = false;
        }
      }, 1500);
    }
  };

  window.triggerPastePcTextWithFeedback = async function () {
    const btnPastePcNow = document.querySelector("#btn-paste-pc-now");
    if (btnPastePcNow) {
      btnPastePcNow.disabled = true;
      btnPastePcNow.textContent = "⏳ Pasting…";
    }
    try {
      if (window.HandsetKeys?.pasteFromPc) {
        await window.HandsetKeys.pasteFromPc(true);
      } else if (window.HandsetKeys?.triggerPasteFromPc) {
        await window.HandsetKeys.triggerPasteFromPc(false);
      } else {
        await postJson("/api/keyboard/paste", { force: true });
      }
      if (btnPastePcNow) {
        btnPastePcNow.textContent = "✓ Pasted!";
        btnPastePcNow.classList.add("is-saved");
      }
    } catch (_) {
      if (btnPastePcNow) btnPastePcNow.textContent = "Paste Failed";
    } finally {
      setTimeout(() => {
        if (btnPastePcNow) {
          btnPastePcNow.textContent = PASTE_PC_LABEL;
          btnPastePcNow.classList.remove("is-saved");
          btnPastePcNow.disabled = false;
        }
      }, 1200);
    }
  };

  window.triggerClearCopyCacheWithFeedback = async function () {
    const btnClearCache = document.querySelector("#btn-clear-copy-cache");
    if (btnClearCache) {
      btnClearCache.disabled = true;
      btnClearCache.textContent = "🧹 Clearing…";
    }
    try {
      window._ViewportHistory = [];
      const mirrorFrame = document.querySelector("#mirror-frame");
      if (mirrorFrame?.contentWindow) {
        mirrorFrame.contentWindow.postMessage({ type: "clear-viewport-history" }, "*");
        if (typeof mirrorFrame.contentWindow.clearViewportHistory === "function") {
          mirrorFrame.contentWindow.clearViewportHistory();
        } else if (typeof mirrorFrame.contentWindow.clearOcrSessionMemory === "function") {
          mirrorFrame.contentWindow.clearOcrSessionMemory();
        }
      }
      await Promise.allSettled([
        postJson("/api/keyboard/shortcut", { action: "clear-copy-cache" }),
        fetch("/api/ocr/clear-chunks", { method: "POST" }),
      ]);
      if (btnClearCache) {
        btnClearCache.textContent = "✓ Cache Cleared!";
        btnClearCache.classList.add("copied");
        btnClearCache.classList.add("is-saved");
      }
    } catch (_) {
      if (btnClearCache) btnClearCache.textContent = "Clear Failed";
    } finally {
      setTimeout(() => {
        if (btnClearCache) {
          btnClearCache.textContent = CLEAR_COPY_CACHE_LABEL || "🧹 Clear Copy Cache (Alt+R)";
          btnClearCache.classList.remove("copied");
          btnClearCache.classList.remove("is-saved");
          btnClearCache.disabled = false;
        }
      }, 1500);
    }
  };

  const btnPastePcNow = document.querySelector("#btn-paste-pc-now");
  btnPastePcNow?.addEventListener("click", () => {
    window.triggerPastePcTextWithFeedback();
  });

  const btnCopyIphone = document.querySelector("#btn-copy-iphone-now");
  btnCopyIphone?.addEventListener("click", () => {
    window.triggerCopyIphoneTextWithFeedback();
  });

  window.triggerScanQrLinkWithFeedback = async function () {
    const btnScanQr = document.querySelector("#btn-scan-qr-link");
    if (btnScanQr) {
      btnScanQr.disabled = true;
      btnScanQr.textContent = "⏳ Grabbing Copied Link…";
    }
    try {
      if (typeof window.HandsetKeys?.flushTypedQueue === "function") {
        await window.HandsetKeys.flushTypedQueue();
      }

      // Tahap 112: Single fast cycle (Capture -> Capture Head -> Close Spotlight IMMEDIATELY -> 1x batch OCR)
      // Step 1: Open Spotlight, paste clipboard, collapse selection with Right Arrow (handled in screen.py)
      await postJson("/api/device/spotlight-paste", {});

      // Step 2: frameTail = full clean paste frame (iPhone 17 Pro: whole multi-line URL; iPhone 11 Pro Max: Safari card + bottom bar tail)
      await new Promise((r) => setTimeout(r, 220));
      const frameTail = window.HandsetKeys?.captureMirrorCanvasDataUrl ? window.HandsetKeys.captureMirrorCanvasDataUrl() : null;

      // Step 3: Move cursor to head (Up Arrow + Cmd+Left, NO Cmd+A) and capture frameHead
      await postJson("/api/device/spotlight-move-head", {});
      await new Promise((r) => setTimeout(r, 180));
      const frameHead = window.HandsetKeys?.captureMirrorCanvasDataUrl ? window.HandsetKeys.captureMirrorCanvasDataUrl() : null;

      // Step 4: Close Spotlight RIGHT NOW (before OCR) so iPhone returns to the app in ~1.3s
      await postJson("/api/device/spotlight-close", {}).catch(() => { });

      // Step 5: Single OCR call with both frames
      const res = await postJson("/api/qr/scan", {
        frameDataUrl: frameTail || frameHead,
        tailFrameDataUrl: frameHead || frameTail,
        frames: [frameTail, frameHead].filter(Boolean),
        waitResult: true,
      });
      if (res?.ok && (res?.text || res?.extractedText)) {
        const textToCopy = res.text || res.extractedText;
        if (textToCopy && typeof document !== "undefined" && document.hasFocus() && navigator.clipboard?.writeText) {
          try {
            navigator.clipboard.writeText(textToCopy).catch(() => { });
          } catch (_) { }
        }
        if (btnScanQr) {
          btnScanQr.textContent = "✓ Link Copied to PC!";
          btnScanQr.classList.add("is-saved");
        }
      } else {
        if (btnScanQr) {
          btnScanQr.textContent = "⚠️ Link Belum Tersalin di iOS";
        }
      }
      return res;
    } catch (_) {
      if (btnScanQr) btnScanQr.textContent = "Grab Failed";
    } finally {
      setTimeout(() => {
        if (btnScanQr) {
          btnScanQr.textContent = SCAN_QR_LABEL;
          btnScanQr.classList.remove("is-saved");
          btnScanQr.disabled = false;
        }
      }, 1500);
    }
  };

  const btnClearCache = document.querySelector("#btn-clear-copy-cache");
  btnClearCache?.addEventListener("click", () => {
    window.triggerClearCopyCacheWithFeedback();
  });

  const btnScanQrLink = document.querySelector("#btn-scan-qr-link");
  btnScanQrLink?.addEventListener("click", () => {
    window.triggerScanQrLinkWithFeedback();
  });

  const btnQrSetupGuide = document.querySelector("#btn-qr-setup-guide");
  const qrSetupGuideBox = document.querySelector("#qr-setup-guide-box");
  btnQrSetupGuide?.addEventListener("click", () => {
    if (qrSetupGuideBox) {
      const isHidden = qrSetupGuideBox.style.display === "none" || !qrSetupGuideBox.style.display;
      qrSetupGuideBox.style.display = isHidden ? "block" : "none";
    }
  });

  // Global keydown on window to catch cross-platform shortcuts (Alt+C, Alt+V, Alt+R, Alt+G) with capture phase
  window.addEventListener(
    "keydown",
    (e) => {
      const target = e.target;
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) return;

      if (e.altKey && !e.ctrlKey && !e.metaKey && (e.code === "KeyC" || e.key === "c" || e.key === "C")) {
        e.preventDefault();
        e.stopPropagation();
        window.triggerCopyIphoneTextWithFeedback();
      } else if (e.altKey && !e.ctrlKey && !e.metaKey && (e.code === "KeyV" || e.key === "v" || e.key === "V")) {
        e.preventDefault();
        e.stopPropagation();
        window.triggerPastePcTextWithFeedback();
      } else if (e.altKey && !e.ctrlKey && !e.metaKey && (e.code === "KeyR" || e.key === "r" || e.key === "R")) {
        e.preventDefault();
        e.stopPropagation();
        window.triggerClearCopyCacheWithFeedback();
      } else if (e.altKey && !e.ctrlKey && !e.metaKey && (e.code === "KeyB" || e.key === "b" || e.key === "B")) {
        e.preventDefault();
        e.stopPropagation();
        window.triggerScanQrLinkWithFeedback();
      }
    },
    true,
  );

  const btnToggleOnscreen = document.querySelector("#btn-toggle-onscreen-kb");
  btnToggleOnscreen?.addEventListener("click", async () => {
    btnToggleOnscreen.disabled = true;
    const oldText = btnToggleOnscreen.textContent;
    btnToggleOnscreen.textContent = "Toggling…";
    try {
      if (window.HandsetKeys?.toggleOnscreenKeyboard) {
        await window.HandsetKeys.toggleOnscreenKeyboard();
      } else {
        await postJson("/api/keyboard/toggle-onscreen", {});
      }
      btnToggleOnscreen.textContent = "✓ Toggled!";
      btnToggleOnscreen.classList.add("is-saved");
    } catch (_) {
      btnToggleOnscreen.textContent = "Failed";
    } finally {
      setTimeout(() => {
        btnToggleOnscreen.textContent = oldText;
        btnToggleOnscreen.classList.remove("is-saved");
        btnToggleOnscreen.disabled = false;
      }, 1000);
    }
  });
}

function syncSettingsToIframe() {
  syncThemeToIframe();
  applyFitMode(currentFitMode);
  applyCrop(currentCrop, true);
  applyTrackpadSpeed(currentTrackpadSpeed, true);
  applyKeyboardSettings(currentKeyboardSettings, true);
}


function initThemeAndFitControls() {
  document.querySelectorAll("[data-theme-choice]").forEach((btn) => {
    btn.onclick = () => {
      applyTheme(btn.dataset.themeChoice);
    };
  });

  document.querySelectorAll("[data-fit-choice]").forEach((btn) => {
    btn.onclick = () => {
      applyFitMode(btn.dataset.fitChoice);
    };
  });
}


async function triggerWifiScan(udid) {
  try {
    const res = await postJson("/api/wifi/scan", { udid: udid || selectedUdid });
    if (res?.wifi) {
      currentWifiState = res.wifi;
      renderWifi();
      renderPills({ wifi: res.wifi, devices: usbConnected ? [{ udid }] : [] });
    }
  } catch (_) { }
}

function setTransport(value, reload = true) {
  transport = value === "wifi" ? "wifi" : "usb";
  localStorage.setItem("handset-transport", transport);
  document.querySelectorAll("[data-transport]").forEach((btn) => {
    const active = btn.dataset.transport === transport;
    btn.classList.toggle("is-active", active);
    btn.setAttribute("aria-checked", String(active));
  });
  if (transport === "wifi" && (selectedUdid || (currentWifiState?.paired || []).length)) {
    triggerWifiScan(selectedUdid || currentWifiState.paired[0]);
  }
  if (reload) initAutoRotateControls();
  load();
}

// ======================================================================
// Tahap 47: iPhone Screen & Stealth Control (Wake, Lock, Stealth Dim, PIN)
// ======================================================================


function initTransportControls() {
  document.querySelectorAll("[data-transport]").forEach((btn) => {
    btn.onclick = () => setTransport(btn.dataset.transport);
  });
  if (wifiPair) wifiPair.onclick = () => setUpWifi();
  if (wifiScan) {
    wifiScan.onclick = async () => {
      wifiScan.disabled = true;
      wifiScan.textContent = "Scanning…";
      try {
        await triggerWifiScan(selectedUdid);
      } finally {
        wifiScan.disabled = false;
        wifiScan.textContent = "🔍 Scan";
      }
    };
  }
}



function initPinControls() {
  // Tahap 55: Keep PIN strictly empty until applyCleanSlatePinGuard verifies folderInstanceId
  if (inputUnlockPin) inputUnlockPin.value = "";
  try {
    if (!localStorage.getItem("iphone_link_folder_instance_id")) {
      localStorage.removeItem("iphone_link_unlock_pin");
      localStorage.removeItem("pane-unlock-pin");
    }
  } catch (_) { }

  inputUnlockPin?.addEventListener("input", () => {
    inputUnlockPin.value = inputUnlockPin.value.replace(/\D/g, "").slice(0, 6);
    try {
      localStorage.setItem("iphone_link_unlock_pin", inputUnlockPin.value);
      localStorage.setItem("pane-unlock-pin", inputUnlockPin.value);
    } catch (_) { }
  });

  btnTogglePinVisibility?.addEventListener("click", () => {
    if (!inputUnlockPin) return;
    const isPass = inputUnlockPin.type === "password";
    inputUnlockPin.type = isPass ? "text" : "password";
    btnTogglePinVisibility.textContent = isPass ? "🔒" : "👁️";
  });

  // Tahap 52: Clean Wipe PIN & Privacy Data
  btnWipePrivacy?.addEventListener("click", async () => {
    btnWipePrivacy.disabled = true;
    btnWipePrivacy.textContent = "🗑️ Wiping Data...";
    try {
      // 1. Remove all PIN keys from storage
      try {
        localStorage.removeItem("iphone_link_unlock_pin");
        localStorage.removeItem("pane-unlock-pin");
        localStorage.removeItem("iphone_link_folder_instance_id");
        sessionStorage.clear();
      } catch (_) { }

      // 2. Clear input
      if (inputUnlockPin) inputUnlockPin.value = "";

      // 3. Clear viewport history in index.html and mirror.html
      window._ViewportHistory = [];
      try {
        screenView?.contentWindow?.postMessage({ type: "clear-viewport-history" }, "*");
      } catch (_) { }

      // 4. Call backend to wipe privacy data from memory and disk
      await postJson("/api/device/wipe-privacy-data", {});

      // 5. Visual feedback
      btnWipePrivacy.classList.add("wiped");
      btnWipePrivacy.textContent = "✓ PIN & Data Wiped Clean!";
      setTimeout(() => {
        btnWipePrivacy.classList.remove("wiped");
        btnWipePrivacy.textContent = "🗑️ Clear Saved PIN & Privacy Data";
        btnWipePrivacy.disabled = false;
      }, 2000);
    } catch (err) {
      console.error("Wipe privacy error:", err);
      btnWipePrivacy.classList.add("wiped");
      btnWipePrivacy.textContent = "✓ PIN & Data Wiped Clean!";
      setTimeout(() => {
        btnWipePrivacy.classList.remove("wiped");
        btnWipePrivacy.textContent = "🗑️ Clear Saved PIN & Privacy Data";
        btnWipePrivacy.disabled = false;
      }, 2000);
    }
  });
}


function updateStealthDimUI(active) {
  currentStealthActive = Boolean(active);
  if (btnStealthMode) {
    btnStealthMode.classList.toggle("is-active", currentStealthActive);
    btnStealthMode.textContent = currentStealthActive ? "🌑 Stealth Dim Mode: ON" : "🌑 Stealth Dim Mode: OFF";
  }
  if (stealthModeHint) {
    stealthModeHint.textContent = currentStealthActive ? "ON" : "OFF";
    stealthModeHint.style.color = currentStealthActive ? "var(--accent)" : "";
  }
}

async function triggerWakeUnlockIphone() {
  if (btnWakeUnlock) {
    btnWakeUnlock.disabled = true;
    btnWakeUnlock.textContent = "🔓 Waking…";
  }
  try {
    const pin = (inputUnlockPin?.value || "").trim();
    const res = await postJson("/api/device/wake-unlock", { passcode: pin });
    if (res?.ok) {
      if (screenNote) {
        screenNote.textContent = pin ? "🔓 Layar iPhone dibangunkan & PIN diinjeksi." : "🔓 Layar iPhone telah dibangunkan.";
        screenNote.className = "ok";
      }
      // Tahap 53: Do NOT break the active stream! Send wake-stream-pli to request keyframes cleanly
      try {
        screenView?.contentWindow?.postMessage({ type: "wake-stream-pli" }, "*");
      } catch (_) { }
      setTimeout(() => load(), 200);
    }
  } catch (err) {
    console.error("Wake/unlock error:", err);
    if (screenNote) {
      screenNote.textContent = "Gagal membangunkan iPhone: " + (err.message || err);
      screenNote.className = "warn";
    }
  } finally {
    if (btnWakeUnlock) {
      btnWakeUnlock.disabled = false;
      btnWakeUnlock.textContent = "🔓 Wake / Unlock (Alt+U)";
    }
  }
}

async function triggerLockIphone() {
  if (btnLockIphone) {
    btnLockIphone.disabled = true;
    btnLockIphone.textContent = "🔒 Locking…";
  }
  try {
    const res = await postJson("/api/device/lock", {});
    if (res?.ok) {
      if (screenNote) {
        screenNote.textContent = '🔒 iPhone Terkunci (Standby) — Layar fisik dimatikan. Klik "🔓 Wake / Unlock (Alt+U)" untuk membuka kembali.';
        screenNote.className = "calm-standby";
      }
      // Tahap 53: Inform mirror iframe to hide status pill completely during locked mode
      try {
        screenView?.contentWindow?.postMessage({ type: "device-locked" }, "*");
      } catch (_) { }
      setTimeout(() => load(), 200);
    }
  } catch (err) {
    console.error("Lock error:", err);
    if (screenNote) {
      screenNote.textContent = "Gagal mengunci iPhone: " + (err.message || err);
      screenNote.className = "warn";
    }
  } finally {
    if (btnLockIphone) {
      btnLockIphone.disabled = false;
      btnLockIphone.textContent = "🔒 Lock iPhone (Alt+L)";
    }
  }
}

async function triggerToggleStealthDim(explicitState = null) {
  const targetState = explicitState !== null ? Boolean(explicitState) : !currentStealthActive;
  if (btnStealthMode) {
    btnStealthMode.disabled = true;
    btnStealthMode.textContent = targetState ? "🌑 Mengaktifkan Stealth Dim…" : "🌑 Mengembalikan Kecerahan…";
  }
  try {
    const res = await postJson("/api/device/stealth", { enabled: targetState, restoreSteps: 4 });
    if (res?.ok) {
      updateStealthDimUI(res.stealth);
      if (screenNote) {
        screenNote.textContent = res.stealth
          ? "🌑 Stealth Dim Mode ON: Layar fisik iPhone 0% (gelap), stream PC tetap terang."
          : "☀️ Stealth Dim Mode OFF: Kecerahan fisik layar iPhone dikembalikan ke ~25% (4 bar).";
        screenNote.className = "ok";
      }
    }
  } catch (err) {
    console.error("Stealth dim error:", err);
  } finally {
    if (btnStealthMode) {
      btnStealthMode.disabled = false;
      updateStealthDimUI(currentStealthActive);
    }
  }
}

btnWakeUnlock?.addEventListener("click", triggerWakeUnlockIphone);
btnLockIphone?.addEventListener("click", triggerLockIphone);
btnStealthMode?.addEventListener("click", () => triggerToggleStealthDim());

window.triggerWakeUnlockIphone = triggerWakeUnlockIphone;
window.triggerLockIphone = triggerLockIphone;
window.triggerToggleStealthDim = triggerToggleStealthDim;

function renderWifi() {
  const wifi = currentWifiState || {};
  if (wifi.statusText) {
    wifiState.textContent = wifi.statusText;
    wifiState.className = "wifi-state" + (wifi.statusTone ? ` ${wifi.statusTone}` : "");
  } else {
    const ready = wifi.reachable === true;
    wifiState.textContent = ready ? "✓ Wi‑Fi ready" : "Wi‑Fi not set up";
    wifiState.className = "wifi-state" + (ready ? " ok" : "");
  }
  const isPairedAndEnabled = Boolean(wifi.paired && wifi.paired.length > 0 && wifi.enabled);
  wifiPair.hidden = Boolean(wifi.reachable || isPairedAndEnabled);
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

// screenEmpty already declared above


function initDeviceActionControls() {
  btnWakeUnlock?.addEventListener("click", triggerWakeUnlockIphone);
  btnLockIphone?.addEventListener("click", triggerLockIphone);
  btnStealthMode?.addEventListener("click", () => triggerToggleStealthDim());

  window.triggerWakeUnlockIphone = triggerWakeUnlockIphone;
  window.triggerLockIphone = triggerLockIphone;
  window.triggerToggleStealthDim = triggerToggleStealthDim;

  function renderWifi() {
    const wifi = currentWifiState || {};
    if (wifi.statusText) {
      wifiState.textContent = wifi.statusText;
      wifiState.className = "wifi-state" + (wifi.statusTone ? ` ${wifi.statusTone}` : "");
    } else {
      const ready = wifi.reachable === true;
      wifiState.textContent = ready ? "✓ Wi‑Fi ready" : "Wi‑Fi not set up";
      wifiState.className = "wifi-state" + (ready ? " ok" : "");
    }
    const isPairedAndEnabled = Boolean(wifi.paired && wifi.paired.length > 0 && wifi.enabled);
    wifiPair.hidden = Boolean(wifi.reachable || isPairedAndEnabled);
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

  // screenEmpty already declared above
  const placeholder = screenEmpty;
  screenEmpty?.addEventListener("click", () => {
    if (!showScreen.disabled && !showScreen.classList.contains("is-disconnect")) showPhone();
  });

  refresh?.addEventListener("click", () => load());
  showScreen?.addEventListener("click", () => {
    if (showScreen.classList.contains("is-disconnect")) {
      stopMirror();
    } else {
      showPhone();
    }
  });
}



function initWindowAndBarControls() {
  stopScreen?.addEventListener("click", () => stopMirror());
  restartScreen?.addEventListener("click", () => restartMirror());
  btnRotateLandscape?.addEventListener("click", () => toggleLandscapeMode());

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !controlPane?.hidden) {
      toggleSettings(false);
    }
  });

  appBar?.addEventListener("mousedown", (e) => {
    if (e.target.closest("button, input, select, a, .no-drag, [data-action], .win-btn, .bar-btn, .icon-btn")) {
      return;
    }
    if (window.pywebview?.api?.start_window_drag) {
      window.pywebview.api.start_window_drag();
    }
  });

  ["mousedown", "pointerdown"].forEach((evtName) => {
    sideDrawer?.addEventListener(evtName, (e) => {
      e.stopPropagation();
    });
  });

  document.querySelectorAll("input").forEach((inp) => {
    inp.addEventListener("mousedown", (e) => e.stopPropagation());
    inp.addEventListener("pointerdown", (e) => e.stopPropagation());
  });
}


function applyFullsizeState(full, forceDrawerClose = true) {
  isFullSize = Boolean(full);
  window.isFullSize = isFullSize;
  if (appRoot) {
    if (isFullSize) {
      appRoot.classList.add("is-fullsize");
    } else {
      appRoot.classList.remove("is-fullsize");
    }
    if (isFullSize && forceDrawerClose) {
      isDrawerOpen = false;
      appRoot.classList.remove("drawer-open");
      if (controlPane) {
        controlPane.classList.remove("is-open");
        controlPane.classList.remove("open");
        controlPane.hidden = true;
      }
    }
  }
  try {
    const iframe = document.getElementById("screen-view");
    if (iframe && iframe.contentWindow) {
      iframe.contentWindow.dispatchEvent(new Event("resize"));
      iframe.contentWindow.postMessage({ type: "resize" }, "*");
    }
  } catch (e) { }
  return isFullSize;
}
window.applyFullsizeState = applyFullsizeState;

function syncFullsizeClassFromViewport() {
  // Lebar maksimum jendela normal adalah 1200px (Landscape + Drawer).
  // Maka setiap lebar >= 1220px atau >= availWidth - 60 pasti merupakan Full Size / Maximized!
  const availW = (window.screen && window.screen.availWidth) ? window.screen.availWidth : 1920;
  if (window.innerWidth >= 1220 || window.innerWidth >= (availW - 60)) {
    return applyFullsizeState(true, true);
  } else if (!isFullSize && window.innerWidth <= 1200 && !document.fullscreenElement) {
    return applyFullsizeState(false, false);
  }
  return isFullSize;
}
window.syncFullsizeClassFromViewport = syncFullsizeClassFromViewport;

async function toggleFullSize(targetFull) {
  try {
    const currentlyFull = Boolean(isFullSize || (appRoot && appRoot.classList.contains("is-fullsize")) || window.innerWidth >= 1220);
    const targetState = (typeof targetFull === "boolean") ? targetFull : !currentlyFull;

    // Jika hendak masuk ke Full Size dan Drawer sedang terbuka, tutup Drawer lebih dulu agar layar iPhone tepat di tengah monitor:
    if (targetState && (isDrawerOpen || (appRoot && appRoot.classList.contains("drawer-open")) || (!controlPane?.hidden))) {
      isDrawerOpen = false;
      window.isDrawerOpen = false;
      if (appRoot) appRoot.classList.remove("drawer-open");
      if (controlPane) {
        controlPane.classList.remove("is-open");
        controlPane.classList.remove("open");
        controlPane.hidden = true;
      }
    }

    applyFullsizeState(targetState, true);

    if (window.pywebview && window.pywebview.api && typeof window.pywebview.api.toggle_fullscreen === "function") {
      const res = await window.pywebview.api.toggle_fullscreen(targetState);
      if (res && typeof res.fullscreen === "boolean") {
        applyFullsizeState(res.fullscreen, true);
      }
    } else if (!document.fullscreenElement && targetState) {
      document.documentElement.requestFullscreen?.().catch(() => { });
      applyFullsizeState(true, true);
    } else if (document.fullscreenElement && !targetState) {
      document.exitFullscreen?.().catch(() => { });
      applyFullsizeState(false, true);
    }

    if (!targetState) {
      const displayWidth = isLandscapeMode ? "840px" : "346px";
      document.documentElement.style.setProperty("--display-width", displayWidth);
      appRoot?.style.setProperty("--display-width", displayWidth);
      const iframe = document.getElementById("screen-view");
      if (iframe && iframe.contentWindow) {
        [0, 60, 150, 300].forEach((ms) => {
          setTimeout(() => {
            try {
              iframe.contentWindow.dispatchEvent(new Event("resize"));
              iframe.contentWindow.postMessage({ type: "resize" }, "*");
            } catch (_) { }
          }, ms);
        });
      }
    }
  } catch (e) {
    console.warn("toggleFullSize error:", e);
  }
}
window.toggleFullSize = toggleFullSize;

function bindFullsizeControls() {
  const btnMax = document.querySelector("#win-max");
  if (btnMax && !btnMax._boundFullsize) {
    btnMax._boundFullsize = true;
    btnMax.addEventListener("click", toggleFullSize);
  }
  const btnFs = document.querySelector("#btn-fullsize");
  if (btnFs && !btnFs._boundFullsize) {
    btnFs._boundFullsize = true;
    btnFs.addEventListener("click", toggleFullSize);
  }
}
bindFullsizeControls();

toggleGuide?.addEventListener("click", () => {
  const open = runGuide.hidden;
  runGuide.hidden = !open;
  toggleGuide.setAttribute("aria-expanded", String(open));
  if (open) runGuide.scrollIntoView({ behavior: "smooth", block: "start" });
});




// Tahap 95: Real-time Wallpaper Signature Sync (< 0.8s reaction, 0.00% CPU)
function applyWallpaperSignature(sig) {
  if (!sig || sig === currentWallpaperSig) return;
  currentWallpaperSig = sig;
  const imgUrl = `/api/system/wallpaper?v=${encodeURIComponent(sig)}_${Date.now()}`;
  document.documentElement.style.setProperty("--wallpaper-bg-image", `url("${imgUrl}")`);
  const appEl = document.querySelector(".app");
  if (appEl) {
    appEl.style.setProperty("--wallpaper-bg-image", `url("${imgUrl}")`);
  }
  const img = new Image();
  img.onload = () => {
    document.documentElement.style.setProperty("--wallpaper-bg-image", `url("${imgUrl}")`);
    if (appEl) {
      appEl.style.setProperty("--wallpaper-bg-image", `url("${imgUrl}")`);
    }
  };
  img.src = imgUrl;
}
window.applyWallpaperSignature = applyWallpaperSignature;

let _wallpaperCheckTimer = null;
let _isCheckingWallpaper = false;

async function checkWallpaperSig() {
  if (_isCheckingWallpaper) return;
  _isCheckingWallpaper = true;
  try {
    const res = await getJson("/api/system/wallpaper-sig");
    if (res && res.wallpaperSig) {
      applyWallpaperSignature(res.wallpaperSig);
    }
  } catch (_) {
  } finally {
    _isCheckingWallpaper = false;
  }
}
window.checkWallpaperSig = checkWallpaperSig;

function startWallpaperSigWatcher() {
  if (_wallpaperCheckTimer) return;
  _wallpaperCheckTimer = window.setInterval(checkWallpaperSig, 1200);
  window.addEventListener("focus", () => checkWallpaperSig());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") checkWallpaperSig();
  });
}
window.startWallpaperSigWatcher = startWallpaperSigWatcher;

async function load() {
  refresh.disabled = true;
  detail.hidden = true;
  detail.replaceChildren();
  try {
    const state = await getJson("/api/status");
    wifiPaired = state.wifi?.paired || [];
    currentWifiState = state.wifi || {};
    usbConnected = (state.devices || []).length > 0;
    renderStatus(state);
    renderDevices(state.devices || []);
    renderWifi();
    renderScreen(state.screen);
    renderPills(state);
    if (state.audio) renderAudio(state.audio);
    // Tahap 95: Update wallpaper background image instantly when wallpaperSig changes
    if (state.wallpaperSig) {
      applyWallpaperSignature(state.wallpaperSig);
    }
    startWallpaperSigWatcher();
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
  const wifi = state.wifi || currentWifiState || {};
  const overWifi = transport === "wifi";
  if (state.screen?.intentionally_locked) {
    setPill(pillConnection, "Standby · Locked", "ok");
  } else if (state.screen?.phase === "running") {
    setPill(pillConnection, state.screen.transport === "wifi" ? "Mirroring · Wi‑Fi" : "Mirroring", "ok");
  } else if (overWifi && wifi.reachable) {
    setPill(pillConnection, wifi.ip ? `Wi‑Fi · ${wifi.ip}` : "Wi‑Fi Ready", "ok");
  } else if (overWifi && (wifi.paired || []).length > 0) {
    setPill(pillConnection, "Wi‑Fi Paired (Offline)", "warn");
  } else if (overWifi) {
    setPill(pillConnection, "Plug USB 1x", "bad");
  } else if (state.error) {
    setPill(pillConnection, "Error", "bad");
  } else if (!driver.ready) {
    setPill(pillConnection, "Install Apple Devices", "warn");
  } else if (!connected) {
    setPill(pillConnection, "Ready", "ok");
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
    const wifi = currentWifiState || {};
    const connLabel = wifi.reachable ? (wifi.ip ? `Wi‑Fi (${wifi.ip})` : "Wi‑Fi (Online)") : "Wi‑Fi (Offline)";
    list = wifiPaired.map((udid) => ({ udid, connection: connLabel, wifiOnly: true, reachable: Boolean(wifi.reachable) }));
  }
  if (!list.length) {
    const item = document.createElement("li");
    item.className = "empty";
    item.textContent = "No iPhone detected.";
    devicesEl.append(item);
    selectedUdid = null;
    _cachedDeviceDetails.clear();
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
      openDevice(device.udid);
      renderWifi();
    });
    item.append(button);
    devicesEl.append(item);
  }
  if (list.length === 1) openDevice(list[0].udid);
}



async function openDevice(udid, force = false) {
  detail.hidden = false;
  if (!force && _cachedDeviceDetails.has(udid)) {
    const cachedDev = _cachedDeviceDetails.get(udid);
    renderDetail(cachedDev);
    if (cachedDev.info?.DeviceName) {
      pillDevice.hidden = false;
      pillDevice.textContent = cachedDev.info.DeviceName;
    }
    return;
  }
  detail.textContent = "Reading device…";
  try {
    const device = await getJson(`/api/devices/${encodeURIComponent(udid)}`);
    if (device && device.paired) {
      _cachedDeviceDetails.set(udid, device);
    }
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

function sendAppMetrics() {
  try {
    const getEl = (sel) => {
      const el = document.querySelector(sel);
      if (!el) return null;
      const r = el.getBoundingClientRect();
      const s = window.getComputedStyle(el);
      return {
        rect: { top: r.top, left: r.left, width: r.width, height: r.height },
        computedHeight: s.height,
        computedWidth: s.width,
        computedDisplay: s.display,
        computedPosition: s.position,
        aspectRatio: s.aspectRatio,
      };
    };

    const metrics = {
      source: "app.js",
      elements: {
        mainBody: getEl(".main-body"),
        displayCol: getEl(".display-col"),
        workspace: getEl(".workspace"),
        stage: getEl(".stage"),
        phoneShell: getEl("#phone-shell"),
        phoneScreen: getEl("#phone-screen"),
        screenView: getEl("iframe#screen-view"),
      },
      window: {
        innerWidth: window.innerWidth,
        innerHeight: window.innerHeight,
        devicePixelRatio: window.devicePixelRatio,
      },
    };
    fetch("/debug-metrics", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(metrics),
    }).catch(() => { });
  } catch (err) {
    console.error("sendAppMetrics error:", err);
  }
}

function renderScreen(screen) {
  if (!screen) return;
  if (typeof screen.stealth_dim === "boolean") {
    updateStealthDimUI(screen.stealth_dim);
  }
  const mirroring = screen.phase === "running";
  if (mirroring) {
    workspace.classList.add("is-live");
    phoneShell.classList.add("live");
    remotePanel.hidden = false;
    restartScreen.hidden = false;
    restartScreen.disabled = false;
    kbToggle.hidden = false;
    if (screenNote) {
      screenNote.textContent = "";
      screenNote.hidden = true;
    }
    if (screen.intentionally_locked && screenErrorBanner) {
      screenErrorBanner.style.display = "none";
    }
    showScreen.textContent = "⏹ Disconnect Mirroring";
    showScreen.classList.add("is-disconnect");
    showScreen.disabled = false;
    if (stopScreen) {
      stopScreen.hidden = false;
      stopScreen.removeAttribute("hidden");
      stopScreen.classList.remove("is-hidden");
      stopScreen.style.removeProperty("display");
      stopScreen.disabled = false;
    }

    // Sembunyikan placeholder secara mutlak
    if (screenEmpty) {
      screenEmpty.hidden = true;
      screenEmpty.style.display = "none";
    }

    // Tampilkan screen-view (iframe)
    screenView.hidden = false;
    screenView.style.display = "block";

    // Jangan me-reload iframe berulang-ulang pada setiap polling /status jika sudah memuat mirror.html
    const curSrc = screenView.getAttribute("src") || "";
    if (!curSrc.startsWith("/mirror.html")) {
      screenView.src = MIRROR_PAGE;
    }

    // Picu fitCanvas ulang dan sinkronkan tema/fit mode di dalam iframe setelah unhide
    setTimeout(() => {
      syncSettingsToIframe();
      screenView.contentWindow?.postMessage({ type: "resize" }, "*");
    }, 50);
    setTimeout(() => {
      syncSettingsToIframe();
      screenView.contentWindow?.postMessage({ type: "resize" }, "*");
    }, 200);

    // Kirim telemetri ukuran elemen DOM cukup 1x per sesi agar tidak memicu spike disk I/O
    if (!window._sentInitialAppMetrics) {
      window._sentInitialAppMetrics = true;
      setTimeout(sendAppMetrics, 250);
    }
    return;
  }

  workspace.classList.remove("is-live");
  phoneShell.classList.remove("live");
  remotePanel.hidden = true;
  restartScreen.hidden = false;
  restartScreen.disabled = false;
  if (stopScreen) {
    stopScreen.hidden = true;
    stopScreen.setAttribute("hidden", "");
    stopScreen.classList.add("is-hidden");
    stopScreen.style.setProperty("display", "none", "important");
  }
  kbToggle.hidden = true;

  // Tampilkan kembali placeholder
  if (screenEmpty) {
    screenEmpty.hidden = false;
    screenEmpty.style.display = "flex";
  }

  // Sembunyikan iframe
  screenView.hidden = true;
  screenView.style.display = "none";
  if (screenView.getAttribute("src") !== "about:blank") {
    screenView.src = "about:blank";
  }

  showScreen.textContent = "Start mirroring";
  showScreen.classList.remove("is-disconnect");
  showScreen.disabled = false;

  if (screen.phase === "starting") {
    if (screenNote) {
      screenNote.textContent = "";
      screenNote.hidden = true;
    }
    showScreen.disabled = true;
    showScreen.textContent = "Connecting…";
    restartScreen.disabled = true;
    if (screenErrorBanner) screenErrorBanner.style.display = "none";
    return;
  }
  if (screen.error) {
    if (screenNote) {
      screenNote.textContent = "";
      screenNote.hidden = true;
    }
    if (screenErrorBanner) {
      screenErrorBanner.textContent = screen.error;
      screenErrorBanner.style.display = "block";
    }
    return;
  }
  screenNote.textContent = "";
  screenNote.className = "";
  if (screenErrorBanner) {
    screenErrorBanner.style.display = "none";
  }
}

async function showPhone() {
  showScreen.disabled = true;
  showScreen.textContent = "Connecting…";
  if (screenErrorBanner) screenErrorBanner.style.display = "none";
  try {
    renderScreen(await postJson("/api/screen", { udid: selectedUdid, transport }));
    watchScreen();
    load();
  } catch (error) {
    screenNote.textContent = error.message;
    screenNote.className = "warn";
    if (screenErrorBanner) {
      screenErrorBanner.textContent = error.message;
      screenErrorBanner.style.display = "block";
    }
    showScreen.textContent = "Start mirroring";
    showScreen.classList.remove("is-disconnect");
    showScreen.disabled = false;
  }
}

async function stopMirror() {
  if (stopScreen) stopScreen.disabled = true;
  showScreen.disabled = true;
  showScreen.textContent = "Disconnecting…";
  try {
    await postJson("/api/screen/stop", {});
  } catch (err) {
    console.warn("stopMirror error:", err);
  }
  window.clearInterval(screenTimer);
  renderScreen({ phase: "idle" });
  load();
}

function watchScreen() {
  window.clearInterval(screenTimer);
  screenTimer = window.setInterval(async () => {
    try {
      const screen = await getJson("/api/screen");
      renderScreen(screen);
      if (screen.phase !== "starting") {
        window.clearInterval(screenTimer);
        const lastErr = screen.error;
        await load();
        if (lastErr && screenErrorBanner) {
          screenErrorBanner.textContent = lastErr;
          screenErrorBanner.style.display = "block";
        }
      }
    } catch (error) {
      screenNote.textContent = error.message;
      if (screenErrorBanner) {
        screenErrorBanner.textContent = error.message;
        screenErrorBanner.style.display = "block";
      }
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
    screenView.src = `${MIRROR_PAGE}&${stamp}`;
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
  } catch (_) { }
}

function clearOcrSessionMemory() {
  try {
    const iframe = document.querySelector("#screen-view");
    if (iframe && iframe.contentWindow && typeof iframe.contentWindow.clearOcrSessionMemory === "function") {
      iframe.contentWindow.clearOcrSessionMemory();
    }
  } catch (_) { }
  fetch("/api/ocr/clear-chunks", { method: "POST" }).catch(() => { });
}

// Tahap 110: Smart Motion PLI Booster for Bottom Navigation & Gestures
function sendPostMotionPliSettler() {
  setTimeout(() => {
    fetch("/mirror/pli", { method: "POST" }).catch(() => {});
  }, 180);
  setTimeout(() => {
    fetch("/mirror/pli", { method: "POST" }).catch(() => {});
  }, 420);
}

async function swipe(direction) {
  clearOcrSessionMemory();
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
  sendPostMotionPliSettler();
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
  clearOcrSessionMemory();
  for (let i = 0; i < times; i++) {
    if (i) await sleep(120);
    await mirrorPost("/button", { name: "home", state: "press" });
  }
  sendPostMotionPliSettler();
}

async function gesture(name) {
  clearOcrSessionMemory();
  if (name === "home") {
    try {
      screenView?.contentWindow?.postMessage({
        type: "user-nav-home-clicked",
        autoRotate: Boolean(autoRotateEnabled),
        currentlyLandscape: Boolean(isLandscapeMode),
      }, "*");
    } catch (_) { }
    if (isLandscapeMode && autoRotateEnabled) {
      setTimeout(() => {
        setLandscapeOrientation(false, "auto");
      }, 350);
    }
  } else if (name === "switcher" || name === "recent") {
    try {
      screenView?.contentWindow?.postMessage({
        type: "user-nav-recent-clicked",
        autoRotate: Boolean(autoRotateEnabled),
        currentlyLandscape: Boolean(isLandscapeMode),
      }, "*");
    } catch (_) { }
    if (isLandscapeMode && autoRotateEnabled) {
      setTimeout(() => {
        setLandscapeOrientation(false, "auto");
      }, 350);
    }
  }
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
  sendPostMotionPliSettler();
}

function toggleFullscreen() {
  if (document.fullscreenElement) {
    document.exitFullscreen().catch(() => { });
    return;
  }
  phoneScreen.requestFullscreen?.().catch(() => { });
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
  kbToggle?.addEventListener("click", () => setKeyboard(!window.handsetKeyboardOn));
  setKeyboard(true);
  // A focused button would swallow Space and Enter instead of typing them.
  document.addEventListener("pointerup", (e) => {
    const btn = e.target.closest?.("button");
    if (btn) btn.blur();
  });
  const onHomeClicked = () => {
    try {
      screenView?.contentWindow?.postMessage({
        type: "user-nav-home-clicked",
        autoRotate: Boolean(autoRotateEnabled),
        currentlyLandscape: Boolean(isLandscapeMode),
      }, "*");
    } catch (_) { }
    if (isLandscapeMode && autoRotateEnabled) {
      setTimeout(() => {
        setLandscapeOrientation(false, "auto");
      }, 350);
    }
  };
  const onRecentClicked = () => {
    try {
      screenView?.contentWindow?.postMessage({
        type: "user-nav-recent-clicked",
        autoRotate: Boolean(autoRotateEnabled),
        currentlyLandscape: Boolean(isLandscapeMode),
      }, "*");
    } catch (_) { }
    if (isLandscapeMode && autoRotateEnabled) {
      setTimeout(() => {
        setLandscapeOrientation(false, "auto");
      }, 350);
    }
  };
  document.querySelector("#nav-home")?.addEventListener("click", onHomeClicked);
  document.querySelectorAll('[data-nav="home"]').forEach(el => el.addEventListener("click", onHomeClicked));
  document.querySelector("#nav-recent")?.addEventListener("click", onRecentClicked);
  document.querySelectorAll('[data-nav="recent"]').forEach(el => el.addEventListener("click", onRecentClicked));
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
      if (name === "home") clearOcrSessionMemory();
      if (name) mirrorPost("/button", { name, state: "press" });
      sendPostMotionPliSettler();
    });
  });
  document.querySelectorAll("[data-swipe]").forEach((btn) => {
    btn.addEventListener("click", () => swipe(btn.dataset.swipe));
  });
  document.querySelector("#zoom-in")?.addEventListener("click", () => mirrorMessage("zoom-in"));
  document.querySelector("#zoom-out")?.addEventListener("click", () => mirrorMessage("zoom-out"));
  document.querySelector("#zoom-reset")?.addEventListener("click", () => mirrorMessage("zoom-reset"));
  document.querySelector("#rotate-cw")?.addEventListener("click", () => {
    screenView?.contentWindow?.postMessage({ type: "rotate-step", delta: 90 }, "*");
  });
  document.querySelector("#rotate-ccw")?.addEventListener("click", () => {
    screenView?.contentWindow?.postMessage({ type: "rotate-step", delta: -90 }, "*");
  });
}

// Tahap 69: Atomic Synchronization for Canvas Rotation & Window Dimensions
function setLandscapeOrientation(targetLandscape, source = "manual") {
  isLandscapeMode = Boolean(targetLandscape);
  window.isLandscapeMode = isLandscapeMode;
  appRoot?.classList.toggle("is-landscape", isLandscapeMode);
  const btnRot = document.querySelector("#btn-rotate-landscape");
  btnRot?.classList.toggle("active", isLandscapeMode);

  // Tahap 75: Pertahankan state isFullSize jika sedang berada di Full Screen
  if (isFullSize || (appRoot && appRoot.classList.contains("is-fullsize"))) {
    isFullSize = true;
    window.isFullSize = true;
    appRoot?.classList.add("is-fullsize");
  }

  const displayWidth = isLandscapeMode ? "840px" : "346px";
  document.documentElement.style.setProperty("--display-width", displayWidth);
  appRoot?.style.setProperty("--display-width", displayWidth);

  const isDrawerOpen = !controlPane?.hidden && (controlPane?.classList.contains("open") || appRoot?.classList.contains("drawer-open"));
  if (window.pywebview?.api?.set_orientation) {
    window.pywebview.api.set_orientation(isLandscapeMode, isDrawerOpen);
  } else if (window.pywebview?.api?.set_drawer) {
    window.pywebview.api.set_drawer(isDrawerOpen);
  }

  const notifyFrame = () => {
    try {
      screenView?.contentWindow?.postMessage({
        type: "set-orientation",
        landscape: isLandscapeMode,
        rotation: isLandscapeMode ? 270 : 0,
        source: source,
        manualOverride: (source === "manual"),
      }, "*");
      screenView?.contentWindow?.postMessage({ type: "resize" }, "*");
    } catch (_) { }
  };
  notifyFrame();
  setTimeout(notifyFrame, 60);
  setTimeout(notifyFrame, 180);

  if (source === "auto") {
    showToastNotification(isLandscapeMode ? "🎮 Auto-Rotate: Landscape" : "📱 Auto-Rotate: Portrait");
  } else if (source === "manual") {
    showToastNotification(isLandscapeMode ? "🔄 Landscape Mode" : "📱 Portrait Mode");
  }
}
window.setLandscapeOrientation = setLandscapeOrientation;

function applyOrientationUI(isLandscape) {
  setLandscapeOrientation(isLandscape, "sync");
}
window.applyOrientationUI = applyOrientationUI;

function toggleLandscapeMode() {
  setLandscapeOrientation(!isLandscapeMode, "manual");
}
window.toggleLandscapeMode = toggleLandscapeMode;

function handleDeviceDimensions(width, height) {
  if (!width || !height) return;
  lastDeviceDimensions = { width, height };
  if (isLandscapeMode || isFullSize || (appRoot && appRoot.classList.contains("is-fullsize")) || appRoot?.classList.contains("is-landscape")) return;
  const aspect = width / height;
  if (aspect < 0.35 || aspect > 2.5) return;
  lastDeviceAspect = aspect;

  if (currentFitMode === "autofit") {
    // 838 window height - 36px Top Bar - 44px Quick Nav = 756px calibrated stage height (Tahap 69: Zero Black Bar)
    const stageH = 756;
    const idealW = Math.max(336, Math.min(500, Math.round(stageH * aspect)));
    const displayWidth = `${idealW}px`;
    document.documentElement.style.setProperty("--display-width", displayWidth);
    appRoot?.style.setProperty("--display-width", displayWidth);
    const isDrawerOpen = !controlPane?.hidden && (controlPane?.classList.contains("open") || appRoot?.classList.contains("drawer-open"));
    if (window.pywebview?.api?.sync_device_aspect) {
      window.pywebview.api.sync_device_aspect(idealW, isDrawerOpen);
    }
  } else {
    const displayWidth = "346px";
    document.documentElement.style.setProperty("--display-width", displayWidth);
    appRoot?.style.setProperty("--display-width", displayWidth);
  }
}

window.addEventListener("message", (e) => {
  if (e.data && e.data.type === "video-dimensions") {
    handleDeviceDimensions(e.data.width, e.data.height);
  }
  if (e.data && e.data.type === "rotation-changed") {
    const isLand = Boolean(e.data.isLandscape);
    if (isLand !== isLandscapeMode) {
      setLandscapeOrientation(isLand, e.data.source || "sync");
    }
  }
  if (e.data && e.data.type === "trigger-clear-copy-cache") {
    window.triggerClearCopyCacheWithFeedback();
  }
  if (e.data && e.data.type === "auto-rotate-request" && autoRotateEnabled) {
    const targetLandscape = Boolean(e.data.landscape);
    if (targetLandscape !== isLandscapeMode) {
      setLandscapeOrientation(targetLandscape, "auto");
    }
  }
});

window.addEventListener("resize", () => {
  // Tahap 73 & 75: Atomic fullsize state sync on window resize (locks fullscreen against transient resize)
  if (isFullSize || (appRoot && appRoot.classList.contains("is-fullsize"))) {
    isFullSize = true;
    window.isFullSize = true;
    appRoot?.classList.add("is-fullsize");
  } else {
    syncFullsizeClassFromViewport();
  }

  const displayWidth = isLandscapeMode ? "840px" : "346px";
  document.documentElement.style.setProperty("--display-width", displayWidth);
  appRoot?.style.setProperty("--display-width", displayWidth);
});

// Tahap 63: Unified Bulletproof Binding for Display Fit, Auto-Rotate & Crop Controls
function bindDisplayFitAndAutoRotateControls() {
  initAutoRotateControls();

  document.querySelectorAll("[data-fit-choice]").forEach((btn) => {
    btn.onclick = (e) => {
      e.preventDefault();
      const choice = btn.dataset.fitChoice;
      const mode = (choice === "fill" || choice === "contain") ? choice : "autofit";
      applyFitMode(mode);
      postJson("/api/settings", { fitMode: mode }).catch(() => { });
    };
  });
  applyFitMode(currentFitMode);

  const btnReset = document.querySelector("#btn-reset-crop");
  if (btnReset) {
    btnReset.onclick = (e) => {
      e.preventDefault();
      currentCrop = { top: 0, bottom: 0, left: 0, right: 0 };
      localStorage.setItem("pane-crop-edges", JSON.stringify(currentCrop));
      ["top", "bottom", "left", "right"].forEach((side) => {
        const slider = document.querySelector(`#crop-${side}-slider`);
        const numInput = document.querySelector(`#crop-${side}-num`);
        if (slider) slider.value = "0";
        if (numInput) numInput.value = "0";
      });
      applyCrop(currentCrop, true);
      postJson("/api/settings", { crop: currentCrop }).catch(() => { });
      btnReset.textContent = "✓ Reset to 0";
      setTimeout(() => {
        btnReset.textContent = "Reset (0 px)";
      }, 1000);
    };
  }

  const btnSaveCrop = document.querySelector("#btn-save-crop");
  if (btnSaveCrop) {
    btnSaveCrop.onclick = (e) => {
      e.preventDefault();
      saveUserSettings();
    };
  }
}


// ======================================================================
// Tahap 68 & 70: Pure JS Screen Parallax Tracker (0% Python IPC)
// ======================================================================
// Tahap 68, 70 & 94: Energy-Efficient Pure JS Screen Parallax Tracker
// Tahap 94: Removed infinite 60fps requestAnimationFrame loop!
// Only updates CSS variables on resize, focus, or 1500ms interval when moved >= 2px.
function startPureJsScreenParallax() {
  if (window._parallaxStarted) return;
  window._parallaxStarted = true;
  let lastX = null, lastY = null, lastW = null, lastH = null;

  function updatePos(force = false) {
    try {
      const sw = (window.screen && window.screen.width) || 1920;
      const sh = (window.screen && window.screen.height) || 1080;
      const wx = Math.max(0, Math.min(sw - 100, typeof window.screenX === "number" ? window.screenX : 0));
      const wy = Math.max(0, Math.min(sh - 100, typeof window.screenY === "number" ? window.screenY : 0));

      const moved = lastX === null || force || Math.abs(wx - lastX) >= 2 || Math.abs(wy - lastY) >= 2 || sw !== lastW || sh !== lastH;
      if (moved) {
        lastX = wx; lastY = wy; lastW = sw; lastH = sh;
        const st = document.documentElement.style;
        st.setProperty("--screen-w", sw + "px");
        st.setProperty("--screen-h", sh + "px");
        st.setProperty("--win-x", wx + "px");
        st.setProperty("--win-y", wy + "px");
      }
    } catch (e) { }
  }

  // Initial immediate update
  updatePos(true);

  // Event-driven updates
  window.addEventListener("resize", () => updatePos(false), { passive: true });
  window.addEventListener("focus", () => updatePos(false), { passive: true });

  // Lightweight 1500ms fallback interval
  setInterval(() => updatePos(false), 1500);
}
window.startPureJsScreenParallax = startPureJsScreenParallax;

window.addEventListener("focus", () => {
  if (typeof load === "function") load();
});

// ======================================================================
// Tahap 70: Orderly Bootstrap Sequence at Bottom of File (Anti-TDZ 100%)
// ======================================================================
function bootstrapApp() {
  try { setupEarlyControls(); } catch (e) { console.error("setupEarlyControls error:", e); }
  try { initWindowAndBarControls(); } catch (e) { console.error("initWindowAndBarControls error:", e); }
  try { bindFullsizeControls(); syncFullsizeClassFromViewport(); } catch (e) { console.error("bindFullsizeControls error:", e); }
  try { wireRemoteControls(); } catch (e) { console.error("wireRemoteControls error:", e); }
  try { initPinControls(); } catch (e) { console.error("initPinControls error:", e); }
  try { initDeviceActionControls(); } catch (e) { console.error("initDeviceActionControls error:", e); }
  try { initTransportControls(); } catch (e) { console.error("initTransportControls error:", e); }
  try { initThemeAndFitControls(); } catch (e) { console.error("initThemeAndFitControls error:", e); }
  try { initCropControls(); } catch (e) { console.error("initCropControls error:", e); }
  try { initTrackpadSpeedControls(); } catch (e) { console.error("initTrackpadSpeedControls error:", e); }
  try { initKeyboardSettingsControls(); } catch (e) { console.error("initKeyboardSettingsControls error:", e); }
  try { initAudioControls(); } catch (e) { console.error("initAudioControls error:", e); }
  try { bindDisplayFitAndAutoRotateControls(); } catch (e) { console.error("bindDisplayFitAndAutoRotateControls error:", e); }
  try { applyTheme(currentTheme); } catch (e) { console.error("applyTheme error:", e); }
  try { applyFitMode(currentFitMode); } catch (e) { console.error("applyFitMode error:", e); }
  try { applyCrop(currentCrop, true); } catch (e) { console.error("applyCrop error:", e); }
  try { applyTrackpadSpeed(currentTrackpadSpeed, true); } catch (e) { console.error("applyTrackpadSpeed error:", e); }
  try { applyKeyboardSettings(currentKeyboardSettings, true); } catch (e) { console.error("applyKeyboardSettings error:", e); }
  try { setTransport(transport, false); } catch (e) { console.error("setTransport error:", e); }
  try { startPureJsScreenParallax(); } catch (e) { console.error("startPureJsScreenParallax error:", e); }
  try { startWallpaperSigWatcher(); } catch (e) { console.error("startWallpaperSigWatcher error:", e); }
  try { fetchUserSettings(); } catch (e) { console.error("fetchUserSettings error:", e); }
  try { load(); } catch (e) { console.error("load error:", e); }
}

screenView?.addEventListener("load", syncSettingsToIframe);

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", bootstrapApp);
} else {
  bootstrapApp();
}
