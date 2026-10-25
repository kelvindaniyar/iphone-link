// Forwards the PC keyboard to the iPhone as a hardware keyboard. Keys are
// sent by physical position (KeyboardEvent.code -> HID usage); the iPhone's
// own keyboard layout decides which character is typed.
(function () {
  const CODE_TO_HID = (() => {
    const m = {};
    for (let i = 0; i < 26; i++) m[`Key${String.fromCharCode(65 + i)}`] = 0x04 + i;
    for (let i = 1; i <= 9; i++) m[`Digit${i}`] = 0x1d + i;
    m.Digit0 = 0x27;
    Object.assign(m, {
      Enter: 0x28, NumpadEnter: 0x28, Escape: 0x29, Backspace: 0x2a, Tab: 0x2b, Space: 0x2c,
      Minus: 0x2d, Equal: 0x2e, BracketLeft: 0x2f, BracketRight: 0x30, Backslash: 0x31,
      Semicolon: 0x33, Quote: 0x34, Backquote: 0x35, Comma: 0x36, Period: 0x37, Slash: 0x38,
      IntlBackslash: 0x64, CapsLock: 0x39, Delete: 0x4c, Home: 0x4a, End: 0x4d,
      PageUp: 0x4b, PageDown: 0x4e,
      ArrowRight: 0x4f, ArrowLeft: 0x50, ArrowDown: 0x51, ArrowUp: 0x52,
      ShiftLeft: 0xe1, ShiftRight: 0xe5, AltLeft: 0xe2, AltRight: 0xe6,
    });
    return m;
  })();

  // Global Keyboard Settings (persisted and synced across parent and iframe)
  window.HandsetKeyboardSettings = window.HandsetKeyboardSettings || {
    crossPlatformShortcuts: true,
    pastePcClipboard: true,
  };

  // Typed Shadow Buffer tracking
  let typedQueue = "";
  let typedTimer = null;

  async function flushTypedQueue() {
    clearTimeout(typedTimer);
    typedTimer = null;
    if (!typedQueue) return;
    const text = typedQueue;
    typedQueue = "";
    try {
      await fetch("/api/keyboard/typed", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text }),
      });
    } catch (_) {}
  }

  function queueTypedKey(key) {
    if (key === "Backspace") {
      if (typedQueue.length > 0) {
        typedQueue = typedQueue.slice(0, -1);
      } else {
        fetch("/api/keyboard/typed", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ key: "Backspace" }),
        }).catch(() => {});
      }
      return;
    }
    if (key === "Enter") {
      typedQueue += "\n";
      flushTypedQueue();
      return;
    }
    if (typeof key === "string" && key.length === 1) {
      typedQueue += key;
      clearTimeout(typedTimer);
      typedTimer = setTimeout(flushTypedQueue, 30);
    }
  }

  document.addEventListener("pointerdown", () => {
    flushTypedQueue();
  });

  function captureMirrorCanvasDataUrl() {
    try {
      // 1. Check native full-resolution capture from mirror.js in current window
      if (typeof window.captureFullResFrameDataUrl === "function") {
        const url = window.captureFullResFrameDataUrl();
        if (url) return url;
      }
      // 2. Check native full-resolution capture from iframe window
      const iframe = document.getElementById("screen-view");
      const iframeWin = iframe?.contentWindow;
      if (typeof iframeWin?.captureFullResFrameDataUrl === "function") {
        const url = iframeWin.captureFullResFrameDataUrl();
        if (url) return url;
      }
      // 3. In mirror.html (inside iframe) direct canvas
      const localCanvas = document.getElementById("c");
      if (localCanvas && localCanvas.width > 0 && localCanvas.height > 0) {
        return localCanvas.toDataURL("image/png");
      }
      // 4. In index.html (parent frame) direct canvas
      const iframeDoc = iframe?.contentDocument || iframeWin?.document;
      const iframeCanvas = iframeDoc?.getElementById("c");
      if (iframeCanvas && iframeCanvas.width > 0 && iframeCanvas.height > 0) {
        return iframeCanvas.toDataURL("image/png");
      }
    } catch (e) {
      console.warn("captureMirrorCanvasDataUrl notice:", e);
    }
    return null;
  }

  function getMirrorFrameHistory() {
    try {
      if (typeof window.getRecentSelectionFrames === "function") {
        return window.getRecentSelectionFrames();
      }
      const iframe = document.getElementById("mirror-frame") || document.querySelector("iframe");
      const iframeWin = iframe?.contentWindow;
      if (typeof iframeWin?.getRecentSelectionFrames === "function") {
        return iframeWin.getRecentSelectionFrames();
      }
    } catch (e) {
      console.warn("getMirrorFrameHistory notice:", e);
    }
    return [];
  }

  async function triggerShortcut(action, frameDataUrl = null, waitResult = false, frameHistory = [], skipOcr = false) {
    try {
      const resp = await fetch("/api/keyboard/shortcut", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action, frameDataUrl, frameHistory, waitResult, skipOcr }),
      });
      const data = await resp.json();
      if (data?.autoClearedCache === true) {
        if (typeof window.clearOcrSessionMemory === "function") window.clearOcrSessionMemory();
        else window._ViewportHistory = [];
      }
      return data;
    } catch (err) {
      console.warn("triggerShortcut error:", err);
      return { ok: false, error: String(err) };
    }
  }

  async function triggerClearCopyCache() {
    try {
      if (typeof window.clearViewportHistory === "function") {
        window.clearViewportHistory();
      } else if (typeof window.clearOcrSessionMemory === "function") {
        window.clearOcrSessionMemory();
      } else {
        window._ViewportHistory = [];
      }
      if (window.parent && window.parent !== window) {
        try {
          window.parent.postMessage({ type: "trigger-clear-copy-cache" }, "*");
        } catch (_) {}
      }
      const resp = await fetch("/api/keyboard/shortcut", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action: "clear-copy-cache" }),
      });
      fetch("/api/ocr/clear-chunks", { method: "POST" }).catch(() => {});
      return await resp.json();
    } catch (err) {
      console.warn("triggerClearCopyCache error:", err);
      return { ok: false, error: String(err) };
    }
  }

  async function triggerPasteFromPc(force = false) {
    try {
      let clipText = null;
      if (navigator.clipboard?.readText) {
        try {
          clipText = await navigator.clipboard.readText();
        } catch (_) {}
      }
      const pastePcSetting = window.HandsetKeyboardSettings?.pastePcClipboard;
      const resp = await fetch("/api/keyboard/paste", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text: clipText, force: force, pastePcClipboard: pastePcSetting }),
      });
      return await resp.json();
    } catch (err) {
      console.warn("triggerPasteFromPc error:", err);
      return { ok: false, error: String(err) };
    }
  }

  async function triggerToggleOnscreenKeyboard() {
    try {
      const resp = await fetch("/api/keyboard/toggle-onscreen", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });
      return await resp.json();
    } catch (err) {
      console.warn("triggerToggleOnscreenKeyboard error:", err);
      return { ok: false, error: String(err) };
    }
  }

  function isTextField(el) {
    return !!el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));
  }

  function attach({ post, enabled }) {
    const pressed = new Set();
    let chain = Promise.resolve();
    let ignoreKeyUpUntil = 0;

    const flush = () => {
      const snapshot = [...pressed];
      chain = chain.then(() => post({ usages: snapshot })).catch(() => {});
    };

    const releaseAll = () => {
      if (!pressed.size) return;
      pressed.clear();
      flush();
    };

    window.addEventListener(
      "keydown",
      async (e) => {
        if (!enabled() || isTextField(e.target)) return;

        // Shortcut Ctrl + K: Toggle iPhone On-Screen Keyboard cleanly
        if ((e.ctrlKey || e.metaKey) && !e.altKey && e.code === "KeyK") {
          e.preventDefault();
          e.stopPropagation();
          ignoreKeyUpUntil = Date.now() + 400;
          triggerToggleOnscreenKeyboard();
          return;
        }

        // Cross-platform shortcuts
        if (window.HandsetKeyboardSettings?.crossPlatformShortcuts) {
          // If Control key alone is pressed, DO NOT forward 0xE0 to iOS to avoid modifier conflicts
          if (e.code === "ControlLeft" || e.code === "ControlRight") {
            return;
          }

          const isCtrlOrMeta = (e.ctrlKey || e.metaKey);
          const isAlt = e.altKey;

          // Cross-platform shortcuts via Alt:
          // Alt + C: Copy iPhone Text to PC Clipboard (triggers OCR with UI feedback)
          if (isAlt && !isCtrlOrMeta && e.code === "KeyC") {
            e.preventDefault();
            e.stopPropagation();
            ignoreKeyUpUntil = Date.now() + 400;
            pressed.clear();
            flush();
            if (typeof window.top?.triggerCopyIphoneTextWithFeedback === "function") {
              window.top.triggerCopyIphoneTextWithFeedback();
            } else if (typeof window.triggerCopyIphoneTextWithFeedback === "function") {
              window.triggerCopyIphoneTextWithFeedback();
            }
            return;
          }

          // Alt + V: Force Paste PC Clipboard Text to iPhone (with UI feedback)
          if (isAlt && !isCtrlOrMeta && e.code === "KeyV") {
            e.preventDefault();
            e.stopPropagation();
            ignoreKeyUpUntil = Date.now() + 500;
            pressed.clear();
            flush();
            if (typeof window.top?.triggerPastePcTextWithFeedback === "function") {
              window.top.triggerPastePcTextWithFeedback();
            } else if (typeof window.triggerPastePcTextWithFeedback === "function") {
              window.triggerPastePcTextWithFeedback();
            } else {
              triggerPasteFromPc(true);
            }
            return;
          }

          function sampleViewportNow() {
            try {
              if (typeof window.sampleViewportIfChanged === "function") {
                window.sampleViewportIfChanged(true, 0);
              }
              const iframe = document.getElementById("mirror-frame") || document.querySelector("iframe");
              if (typeof iframe?.contentWindow?.sampleViewportIfChanged === "function") {
                iframe.contentWindow.sampleViewportIfChanged(true, 0);
              }
            } catch (_) {}
          }

          // Pure Internal iPhone Shortcuts via Ctrl/Cmd (Zero-Delay, Native iOS):
          if (isCtrlOrMeta && !isAlt) {
            // Ctrl + C: Instant native iOS Copy (Cmd+C, skip OCR, keep Windows clipboard intact)
            if (e.code === "KeyC") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 400;
              pressed.clear();
              flush();
              sampleViewportNow();
              triggerShortcut("copy", null, false, [], true);
              return;
            }

            // Ctrl + V: Smart Paste (instant native iOS Cmd+V if internal copy, or PC text if copied on PC)
            if (e.code === "KeyV") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 500;
              pressed.clear();
              flush();
              triggerPasteFromPc(false);
              return;
            }

            // Ctrl + A: Native iOS Select All
            if (e.code === "KeyA") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 400;
              pressed.clear();
              flush();
              sampleViewportNow();
              triggerShortcut("select-all");
              return;
            }

            // Ctrl + X: Native iOS Cut (Cmd+X, skip OCR)
            if (e.code === "KeyX") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 400;
              pressed.clear();
              flush();
              triggerShortcut("cut", null, false, [], true);
              return;
            }

            // Ctrl + Z: Native iOS Undo
            if (e.code === "KeyZ") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 400;
              pressed.clear();
              flush();
              triggerShortcut("undo");
              return;
            }
          }

          if (isAlt && !isCtrlOrMeta) {
            // Alt + R: Clear copy cache (Tahap 40 / 51)
            if (e.code === "KeyR" || e.key === "r" || e.key === "R") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 400;
              pressed.clear();
              flush();
              if (typeof window.clearViewportHistory === "function") {
                window.clearViewportHistory();
              } else if (typeof window.clearOcrSessionMemory === "function") {
                window.clearOcrSessionMemory();
              } else {
                window._ViewportHistory = [];
              }

              if (window.parent && window.parent !== window) {
                try {
                  window.parent.postMessage({ type: "trigger-clear-copy-cache" }, "*");
                  if (typeof window.parent.triggerClearCopyCacheWithFeedback === "function") {
                    window.parent.triggerClearCopyCacheWithFeedback();
                  }
                } catch (_) {}
              } else if (typeof window.triggerClearCopyCacheWithFeedback === "function") {
                window.triggerClearCopyCacheWithFeedback();
              } else {
                triggerClearCopyCache();
              }
              return;
            }

            // Alt + U: Wake / Unlock iPhone (Tahap 47)
            if (e.code === "KeyU") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 500;
              pressed.clear();
              flush();
              if (typeof window.top?.triggerWakeUnlockIphone === "function") {
                window.top.triggerWakeUnlockIphone();
              } else if (typeof window.triggerWakeUnlockIphone === "function") {
                window.triggerWakeUnlockIphone();
              }
              return;
            }

            // Alt + L: Lock iPhone (Tahap 47)
            if (e.code === "KeyL") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 500;
              pressed.clear();
              flush();
              if (typeof window.top?.triggerLockIphone === "function") {
                window.top.triggerLockIphone();
              } else if (typeof window.triggerLockIphone === "function") {
                window.triggerLockIphone();
              }
              return;
            }

            // Alt + D: Toggle Stealth Dim Mode (Tahap 47)
            if (e.code === "KeyD") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 500;
              pressed.clear();
              flush();
              if (typeof window.top?.triggerToggleStealthDim === "function") {
                window.top.triggerToggleStealthDim();
              } else if (typeof window.triggerToggleStealthDim === "function") {
                window.triggerToggleStealthDim();
              }
              return;
            }

            // Alt + H: Toggle Horizontal / Landscape Mode (Tahap 49)
            if (e.code === "KeyH") {
              e.preventDefault();
              e.stopPropagation();
              ignoreKeyUpUntil = Date.now() + 500;
              pressed.clear();
              flush();
              if (typeof window.top?.toggleLandscapeMode === "function") {
                window.top.toggleLandscapeMode();
              } else if (typeof window.toggleLandscapeMode === "function") {
                window.toggleLandscapeMode();
              }
              return;
            }
          }
        }

        // Record regular keystroke into typed shadow buffer
        if (e.key === "Backspace") {
          queueTypedKey("Backspace");
        } else if (e.key === "Enter") {
          queueTypedKey("Enter");
        } else if (e.key && e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
          queueTypedKey(e.key);
        }

        // Regular keystroke
        const usage = CODE_TO_HID[e.code];
        if (usage === undefined) return;
        e.preventDefault();
        e.stopPropagation();
        if (e.repeat) return;
        pressed.add(usage);
        flush();
      },
      true,
    );

    window.addEventListener(
      "keyup",
      (e) => {
        if (Date.now() < ignoreKeyUpUntil) {
          e.preventDefault();
          return;
        }
        if (e.code === "ControlLeft" || e.code === "ControlRight") {
          if (window.HandsetKeyboardSettings?.crossPlatformShortcuts) {
            return;
          }
        }
        const usage = CODE_TO_HID[e.code];
        if (usage !== undefined && pressed.delete(usage)) {
          e.preventDefault();
          flush();
        }
      },
      true,
    );

    window.addEventListener("blur", releaseAll);
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState !== "visible") releaseAll();
    });
    return { releaseAll };
  }

  window.HandsetKeys = {
    attach,
    captureMirrorCanvasDataUrl,
    getMirrorFrameHistory,
    triggerShortcut,
    triggerClearCopyCache,
    clearCopyCache: triggerClearCopyCache,
    flushTypedQueue,
    pasteFromPc: (force = false) => triggerPasteFromPc(force),
    toggleOnscreenKeyboard: triggerToggleOnscreenKeyboard,
    wakeUnlockIphone: () => (window.top?.triggerWakeUnlockIphone || window.triggerWakeUnlockIphone)?.(),
    lockIphone: () => (window.top?.triggerLockIphone || window.triggerLockIphone)?.(),
    toggleStealthDim: () => (window.top?.triggerToggleStealthDim || window.triggerToggleStealthDim)?.(),
  };
})();
