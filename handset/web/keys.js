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

  async function triggerShortcut(action) {
    try {
      await fetch("/api/keyboard/shortcut", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action }),
      });
    } catch (err) {
      console.warn("triggerShortcut error:", err);
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
      await fetch("/api/keyboard/paste", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text: clipText, force: force }),
      });
    } catch (err) {
      console.warn("triggerPasteFromPc error:", err);
    }
  }

  async function triggerToggleOnscreenKeyboard() {
    try {
      await fetch("/api/keyboard/toggle-onscreen", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });
    } catch (err) {
      console.warn("triggerToggleOnscreenKeyboard error:", err);
    }
  }

  function isTextField(el) {
    return !!el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));
  }

  function attach({ post, enabled }) {
    const pressed = new Set();
    let chain = Promise.resolve();

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
      (e) => {
        if (!enabled() || isTextField(e.target)) return;

        // Shortcut Ctrl + K: Toggle iPhone On-Screen Keyboard cleanly
        if ((e.ctrlKey || e.metaKey) && !e.altKey && e.code === "KeyK") {
          e.preventDefault();
          e.stopPropagation();
          triggerToggleOnscreenKeyboard();
          return;
        }

        // Cross-platform shortcuts (Ctrl+A, Ctrl+C, Ctrl+V, Ctrl+X, Ctrl+Z)
        if (window.HandsetKeyboardSettings?.crossPlatformShortcuts) {
          // If Control key alone is pressed, DO NOT forward 0xE0 to iOS to avoid modifier conflicts
          if (e.code === "ControlLeft" || e.code === "ControlRight") {
            return;
          }

          if ((e.ctrlKey || e.metaKey) && !e.altKey) {
            if (e.code === "KeyA") {
              e.preventDefault();
              e.stopPropagation();
              pressed.clear();
              flush();
              triggerShortcut("select-all");
              return;
            }
            if (e.code === "KeyC") {
              e.preventDefault();
              e.stopPropagation();
              pressed.clear();
              flush();
              triggerShortcut("copy");
              return;
            }
            if (e.code === "KeyV") {
              e.preventDefault();
              e.stopPropagation();
              pressed.clear();
              flush();
              triggerPasteFromPc(false);
              return;
            }
            if (e.code === "KeyX") {
              e.preventDefault();
              e.stopPropagation();
              pressed.clear();
              flush();
              triggerShortcut("cut");
              return;
            }
            if (e.code === "KeyZ") {
              e.preventDefault();
              e.stopPropagation();
              pressed.clear();
              flush();
              triggerShortcut("undo");
              return;
            }
          }
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
    triggerShortcut,
    pasteFromPc: (force = false) => triggerPasteFromPc(force),
    toggleOnscreenKeyboard: triggerToggleOnscreenKeyboard,
  };
})();
