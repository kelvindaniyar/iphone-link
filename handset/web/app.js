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
const stopScreen = document.querySelector("#stop-screen");
const phoneScreen = document.querySelector("#phone-screen");
const kbToggle = document.querySelector("#kb-toggle");
const wifiState = document.querySelector("#wifi-state");
const wifiPair = document.querySelector("#wifi-pair");
const MIRROR_PAGE = "/mirror.html?v=20261024_tahap39_centralized_ocr_and_bidirectional_stitch";
const MIRROR_API = "/mirror";

let currentTheme = localStorage.getItem("pane-theme") || "dark";
let currentFitMode = localStorage.getItem("pane-fit-mode") || "fill";
let currentTrackpadSpeed = 100;
try {
  const savedSpeed = parseInt(localStorage.getItem("pane-trackpad-speed"), 10);
  if (!isNaN(savedSpeed) && savedSpeed >= 30 && savedSpeed <= 200) {
    currentTrackpadSpeed = savedSpeed;
  }
} catch (_) {
  currentTrackpadSpeed = 100;
}

// Cross-Platform Keyboard Shortcuts & Clipboard Settings
let currentKeyboardSettings = {
  crossPlatformShortcuts: true,
  pastePcClipboard: true,
};
try {
  const sShortcuts = localStorage.getItem("pane-shortcuts");
  if (sShortcuts !== null) currentKeyboardSettings.crossPlatformShortcuts = sShortcuts === "true";
  const sPaste = localStorage.getItem("pane-paste-pc");
  if (sPaste !== null) currentKeyboardSettings.pastePcClipboard = sPaste === "true";
} catch (_) {}
window.HandsetKeyboardSettings = currentKeyboardSettings;

// 4-Sided Crop Management (Top, Bottom, Left, Right)
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
} catch (_) {
  currentCrop = { ...DEFAULT_CROP };
}

// Persistent Settings via /api/settings
async function fetchUserSettings() {
  try {
    const res = await getJson("/api/settings");
    if (res) {
      if (res.crop) {
        currentCrop = {
          top: Math.max(0, Math.min(120, parseInt(res.crop.top, 10) || 0)),
          bottom: Math.max(0, Math.min(120, parseInt(res.crop.bottom, 10) || 0)),
          left: Math.max(0, Math.min(120, parseInt(res.crop.left, 10) || 0)),
          right: Math.max(0, Math.min(120, parseInt(res.crop.right, 10) || 0)),
        };
        localStorage.setItem("pane-crop-edges", JSON.stringify(currentCrop));
        applyCrop(currentCrop, true);
      }
      if (res.fitMode) {
        applyFitMode(res.fitMode);
      }
      if (res.theme) {
        applyTheme(res.theme);
      }
      if (res.trackpadSpeed !== undefined) {
        applyTrackpadSpeed(res.trackpadSpeed, true);
      }
      if (
        res.crossPlatformShortcuts !== undefined ||
        res.pastePcClipboard !== undefined
      ) {
        applyKeyboardSettings(
          {
            crossPlatformShortcuts: res.crossPlatformShortcuts,
            pastePcClipboard: res.pastePcClipboard,
          },
          true
        );
      }
      if (res.audioVolume !== undefined) {
        const volSlider = document.querySelector("#audio-vol-slider");
        const volVal = document.querySelector("#audio-vol-val");
        if (volSlider) volSlider.value = String(res.audioVolume);
        if (volVal) volVal.textContent = `${res.audioVolume}%`;
      }
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
let currentAudioState = { enabled: false, active: false, device: "iPhone", state: "Off", volume: 100 };

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
    } catch (_) {}
  });
}

function syncThemeToIframe() {
  try {
    if (screenView && screenView.contentDocument) {
      screenView.contentDocument.documentElement.setAttribute("data-theme", currentTheme);
      screenView.contentDocument.body?.setAttribute("data-theme", currentTheme);
    }
  } catch (_) {}
  try {
    screenView?.contentWindow?.postMessage({ type: "set-theme", theme: currentTheme }, "*");
  } catch (_) {}
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

  if (window.pywebview?.api?.set_theme) {
    window.pywebview.api.set_theme(currentTheme);
  }
}

function applyFitMode(mode) {
  currentFitMode = mode === "contain" ? "contain" : "fill";
  localStorage.setItem("pane-fit-mode", currentFitMode);
  document.querySelectorAll("[data-fit-choice]").forEach((btn) => {
    const active = btn.dataset.fitChoice === currentFitMode;
    btn.classList.toggle("is-active", active);
    btn.setAttribute("aria-checked", String(active));
  });
  try {
    screenView?.contentWindow?.postMessage({ type: "set-fit-mode", mode: currentFitMode }, "*");
  } catch (_) {}
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
  } catch (_) {}
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
    } catch (_) {}
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
  } catch (_) {}
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
  } catch (_) {}
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
        if (btnCopyIphone) btnCopyIphone.textContent = "✓ Copied to PC!";
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
      btnClearCache.textContent = "⏳ Clearing…";
    }
    try {
      if (typeof window.clearOcrSessionMemory === "function") {
        window.clearOcrSessionMemory();
      } else {
        window._ViewportHistory = [];
      }
      if (window.HandsetKeys?.clearCopyCache) {
        await window.HandsetKeys.clearCopyCache();
      } else {
        await postJson("/api/keyboard/shortcut", { action: "clear-copy-cache" });
      }
      if (btnClearCache) {
        btnClearCache.textContent = "✓ Cache Cleared!";
        btnClearCache.classList.add("is-saved");
      }
    } catch (_) {
      if (btnClearCache) btnClearCache.textContent = "Clear Failed";
    } finally {
      setTimeout(() => {
        if (btnClearCache) {
          btnClearCache.textContent = CLEAR_COPY_CACHE_LABEL;
          btnClearCache.classList.remove("is-saved");
          btnClearCache.disabled = false;
        }
      }, 1200);
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

  const btnClearCache = document.querySelector("#btn-clear-copy-cache");
  btnClearCache?.addEventListener("click", () => {
    window.triggerClearCopyCacheWithFeedback();
  });

  // Global keydown on window to catch cross-platform shortcuts (Alt+C, Alt+V, Alt+R) even when focus is on outer page
  window.addEventListener("keydown", (e) => {
    const target = e.target;
    if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) return;

    if (e.altKey && !e.ctrlKey && !e.metaKey && e.code === "KeyC") {
      e.preventDefault();
      window.triggerCopyIphoneTextWithFeedback();
    } else if (e.altKey && !e.ctrlKey && !e.metaKey && e.code === "KeyV") {
      e.preventDefault();
      window.triggerPastePcTextWithFeedback();
    } else if (e.altKey && !e.ctrlKey && !e.metaKey && e.code === "KeyR") {
      e.preventDefault();
      window.triggerClearCopyCacheWithFeedback();
    }
  });

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

screenView?.addEventListener("load", syncSettingsToIframe);

applyTheme(currentTheme);
applyFitMode(currentFitMode);
applyCrop(currentCrop, true);
applyTrackpadSpeed(currentTrackpadSpeed, true);
applyKeyboardSettings(currentKeyboardSettings, true);
initCropControls();
initTrackpadSpeedControls();
initKeyboardSettingsControls();
initAudioControls();
fetchUserSettings();

document.querySelectorAll("[data-theme-choice]").forEach((btn) => {
  btn.addEventListener("click", () => {
    applyTheme(btn.dataset.themeChoice);
  });
});

document.querySelectorAll("[data-fit-choice]").forEach((btn) => {
  btn.addEventListener("click", () => {
    applyFitMode(btn.dataset.fitChoice);
  });
});

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

const screenEmpty = document.querySelector("#screen-empty") || document.querySelector("#placeholder");
const placeholder = screenEmpty;
screenEmpty?.addEventListener("click", () => {
  if (!showScreen.disabled && !showScreen.classList.contains("is-disconnect")) showPhone();
});

refresh.addEventListener("click", () => load());
showScreen.addEventListener("click", () => {
  if (showScreen.classList.contains("is-disconnect")) {
    stopMirror();
  } else {
    showPhone();
  }
});
stopScreen?.addEventListener("click", () => stopMirror());
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

// Native Win32 Window Dragging (only on empty area of .app-bar)
const appBar = document.querySelector(".app-bar");
appBar?.addEventListener("mousedown", (e) => {
  if (e.target.closest("button, input, select, a, .no-drag, [data-action], .win-btn, .bar-btn, .icon-btn")) {
    return;
  }
  if (window.pywebview?.api?.start_window_drag) {
    window.pywebview.api.start_window_drag();
  }
});

// Stop propagation on side drawer and input elements to prevent hijacking drags or window move
const sideDrawer = document.querySelector(".side-drawer");
["mousedown", "pointerdown"].forEach((evtName) => {
  sideDrawer?.addEventListener(evtName, (e) => {
    e.stopPropagation();
  });
});

document.addEventListener("DOMContentLoaded", () => {
  document.querySelectorAll("input").forEach((inp) => {
    inp.addEventListener("mousedown", (e) => e.stopPropagation());
    inp.addEventListener("pointerdown", (e) => e.stopPropagation());
  });
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
    if (state.audio) renderAudio(state.audio);
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
    }).catch(() => {});
  } catch (err) {
    console.error("sendAppMetrics error:", err);
  }
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
    showScreen.textContent = "⏹ Disconnect Mirroring";
    showScreen.classList.add("is-disconnect");
    showScreen.disabled = false;
    if (stopScreen) {
      stopScreen.hidden = false;
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

    // Kirim telemetri ukuran elemen DOM saat mirroring aktif
    setTimeout(sendAppMetrics, 100);
    setTimeout(sendAppMetrics, 600);
    return;
  }

  workspace.classList.remove("is-live");
  phoneShell.classList.remove("live");
  remotePanel.hidden = true;
  restartScreen.hidden = false;
  restartScreen.disabled = false;
  if (stopScreen) stopScreen.hidden = true;
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
  } catch (_) {}
}

function clearOcrSessionMemory() {
  try {
    const iframe = document.querySelector("#screen-view");
    if (iframe && iframe.contentWindow && typeof iframe.contentWindow.clearOcrSessionMemory === "function") {
      iframe.contentWindow.clearOcrSessionMemory();
    }
  } catch (_) {}
  fetch("/api/ocr/clear-chunks", { method: "POST" }).catch(() => {});
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
}

async function gesture(name) {
  clearOcrSessionMemory();
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
      if (name === "home") clearOcrSessionMemory();
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

let lastDeviceAspect = null;
function handleDeviceDimensions(width, height) {
  if (!width || !height) return;
  const aspect = width / height;
  if (aspect < 0.35 || aspect > 2.5) return;
  lastDeviceAspect = aspect;

  // Pertahankan lebar kolom display tetap stabil di 380px tanpa auto-shrink jendela
  document.documentElement.style.setProperty("--display-width", "380px");
  document.querySelector("#app-root")?.style.setProperty("--display-width", "380px");
}

window.addEventListener("message", (e) => {
  if (e.data && e.data.type === "video-dimensions") {
    handleDeviceDimensions(e.data.width, e.data.height);
  }
});

window.addEventListener("resize", () => {
  document.documentElement.style.setProperty("--display-width", "380px");
  document.querySelector("#app-root")?.style.setProperty("--display-width", "380px");
});

load();
