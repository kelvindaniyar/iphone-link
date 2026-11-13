/* ==========================================================================
   iPhone Link v4.0.0 - Official Interactive Website Script (Anti-AI-Slop Edition)
   Zero Emoji - 100% Inline Vector SVG Icons - Obsidian Carbon & Apple Daylight
   ========================================================================== */
(function () {
  "use strict";

  var siteLang = "id";
  var siteTheme = "dark";

  var SVG_MOON = '<svg class="ui-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/></svg>';
  var SVG_SUN = '<svg class="ui-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/></svg>';

  var APP_SVGS = {
    notes: '<svg viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" x2="8" y1="13" y2="13"/><line x1="16" x2="8" y1="17" y2="17"/></svg>',
    instagram: '<svg viewBox="0 0 24 24"><rect width="20" height="20" x="2" y="2" rx="5"/><path d="M16 11.37A4 4 0 1 1 12.63 8 4 4 0 0 1 16 11.37z"/><line x1="17.5" x2="17.51" y1="6.5" y2="6.5"/></svg>',
    efootball: '<svg viewBox="0 0 24 24"><line x1="6" x2="10" y1="12" y2="12"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="15" x2="15.01" y1="13" y2="13"/><line x1="18" x2="18.01" y1="11" y2="11"/><rect width="20" height="12" x="2" y="6" rx="2"/></svg>',
    youtube: '<svg viewBox="0 0 24 24"><polygon points="6 3 20 12 6 21 6 3"/></svg>',
    control: '<svg viewBox="0 0 24 24"><line x1="4" x2="20" y1="21" y2="21"/><line x1="4" x2="20" y1="14" y2="14"/><line x1="4" x2="20" y1="7" y2="7"/><circle cx="9" cy="7" r="2"/><circle cx="15" cy="14" r="2"/><circle cx="10" cy="21" r="2"/></svg>',
    recents: '<svg viewBox="0 0 24 24"><polygon points="12 2 2 7 12 12 22 7 12 2"/><polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/></svg>',
    lockscreen: '<svg viewBox="0 0 24 24"><rect width="18" height="11" x="3" y="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>',
    safari: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polygon points="16.24 7.76 14.12 14.12 7.76 16.24 9.88 9.88 16.24 7.76"/></svg>',
    photos: '<svg viewBox="0 0 24 24"><rect width="18" height="18" x="3" y="3" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/></svg>',
    calendar: '<svg viewBox="0 0 24 24"><rect width="18" height="18" x="3" y="4" rx="2"/><line x1="16" x2="16" y1="2" y2="6"/><line x1="8" x2="8" y1="2" y2="6"/><line x1="3" x2="21" y1="10" y2="10"/></svg>',
    clock: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>',
    files: '<svg viewBox="0 0 24 24"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>'
  };

  /* --------------------------------------------------------------------------
     1. THEME SWITCHER (OBSIDIAN CARBON #09090B <-> APPLE DAYLIGHT #F5F5F7)
     -------------------------------------------------------------------------- */
  var rootHtml = document.documentElement;
  var siteThemeBtn = document.getElementById("btn-site-theme");
  var siteThemeIcon = document.getElementById("site-theme-icon");
  var metaThemeColor = document.querySelector('meta[name="theme-color"]');

  function applySiteTheme(theme) {
    siteTheme = theme === "light" ? "light" : "dark";
    rootHtml.setAttribute("data-theme", siteTheme);
    if (siteThemeIcon) {
      siteThemeIcon.innerHTML = siteTheme === "light" ? SVG_SUN : SVG_MOON;
    }
    if (metaThemeColor) {
      metaThemeColor.setAttribute("content", siteTheme === "light" ? "#F5F5F7" : "#09090B");
    }
    try {
      localStorage.setItem("il_site_theme", siteTheme);
    } catch (e) {}
  }

  try {
    var savedTheme = localStorage.getItem("il_site_theme");
    if (savedTheme === "light" || savedTheme === "dark") {
      applySiteTheme(savedTheme);
    }
  } catch (e) {}

  if (siteThemeBtn) {
    siteThemeBtn.addEventListener("click", function () {
      applySiteTheme(siteTheme === "dark" ? "light" : "dark");
    });
  }

  /* --------------------------------------------------------------------------
     2. OPTICAL LIQUID GLASS NAVBAR, MOBILE NAV & SCROLL REVEAL
     -------------------------------------------------------------------------- */
  var floatingNav = document.getElementById("floating-nav");
  var mobileToggle = document.getElementById("btn-mobile-nav");
  var navLinksEl = document.getElementById("nav-links");
  var navSlidingPill = document.getElementById("nav-sliding-pill");
  var activeNavLink = null;
  var syncNavPillToActive = function () {};

  if (floatingNav) {
    floatingNav.addEventListener("pointermove", function (e) {
      var rect = floatingNav.getBoundingClientRect();
      var px = Math.round(((e.clientX - rect.left) / Math.max(1, rect.width)) * 100);
      var py = Math.round(((e.clientY - rect.top) / Math.max(1, rect.height)) * 100);
      floatingNav.style.setProperty("--nav-mx", px + "%");
      floatingNav.style.setProperty("--nav-my", py + "%");
    });

    var updateNavScrolled = function () {
      floatingNav.classList.toggle("is-scrolled", window.scrollY > 24);
    };
    updateNavScrolled();
    window.addEventListener("scroll", updateNavScrolled, { passive: true });
  }

  if (navLinksEl) {
    var navAnchors = Array.prototype.slice.call(navLinksEl.querySelectorAll("a"));
    if (navAnchors.length > 0) {
      activeNavLink = navAnchors[0];
    }

    var moveNavSlidingPill = function (targetLink) {
      if (!navSlidingPill) return;
      if (!targetLink || window.innerWidth <= 1080) {
        navSlidingPill.style.opacity = "0";
        return;
      }
      var linksRect = navLinksEl.getBoundingClientRect();
      var linkRect = targetLink.getBoundingClientRect();
      if (linkRect.width <= 0 || linksRect.width <= 0) return;
      var x = Math.round((linkRect.left - linksRect.left + navLinksEl.scrollLeft) * 10) / 10;
      var w = Math.round(linkRect.width * 10) / 10;
      navSlidingPill.dataset.x = String(x);
      navSlidingPill.dataset.w = String(w);
      navSlidingPill.style.width = w + "px";
      navSlidingPill.style.transform = "translate3d(" + x + "px, 0, 0)";
      navSlidingPill.style.opacity = "1";
    };

    syncNavPillToActive = function () {
      navAnchors.forEach(function (a) {
        a.classList.toggle("is-active", a === activeNavLink);
      });
      if (activeNavLink) {
        moveNavSlidingPill(activeNavLink);
      }
    };

    navAnchors.forEach(function (a) {
      a.addEventListener("mouseenter", function () {
        moveNavSlidingPill(a);
      });
      a.addEventListener("focus", function () {
        moveNavSlidingPill(a);
      });
      a.addEventListener("click", function () {
        activeNavLink = a;
        syncNavPillToActive();
        navLinksEl.classList.remove("open", "is-open");
      });
    });

    navLinksEl.addEventListener("mouseleave", function () {
      syncNavPillToActive();
    });

    window.addEventListener("resize", function () {
      if (window.innerWidth > 1080) {
        navLinksEl.classList.remove("open", "is-open");
      }
      syncNavPillToActive();
    });

    if ("IntersectionObserver" in window) {
      var linkById = {};
      navAnchors.forEach(function (a) {
        var href = a.getAttribute("href") || "";
        if (href.charAt(0) === "#" && href.length > 1) {
          linkById[href.slice(1)] = a;
        }
      });
      var navSpy = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          if (entry.isIntersecting && linkById[entry.target.id]) {
            activeNavLink = linkById[entry.target.id];
            syncNavPillToActive();
          }
        });
      }, { rootMargin: "-35% 0px -50% 0px", threshold: 0.01 });
      Object.keys(linkById).forEach(function (id) {
        var sec = document.getElementById(id);
        if (sec) navSpy.observe(sec);
      });
    }

    requestAnimationFrame(syncNavPillToActive);
    if (document.fonts && document.fonts.ready) {
      document.fonts.ready.then(function () {
        syncNavPillToActive();
      });
    }
  }

  if (mobileToggle && navLinksEl) {
    mobileToggle.addEventListener("click", function () {
      var willOpen = !(navLinksEl.classList.contains("is-open") || navLinksEl.classList.contains("open"));
      navLinksEl.classList.toggle("is-open", willOpen);
      navLinksEl.classList.toggle("open", willOpen);
    });
  }

  var revealEls = document.querySelectorAll(".reveal");
  if ("IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          entry.target.classList.add("visible");
          io.unobserve(entry.target);
        }
      });
    }, { threshold: 0.08, rootMargin: "0px 0px -30px 0px" });
    revealEls.forEach(function (el) { io.observe(el); });
  } else {
    revealEls.forEach(function (el) { el.classList.add("visible"); });
  }

  /* --------------------------------------------------------------------------
     3. BENTO SPOTLIGHT MOUSE TRACKING & GITHUB STARS
     -------------------------------------------------------------------------- */
  document.querySelectorAll(".bento-card").forEach(function (card) {
    card.addEventListener("pointermove", function (e) {
      var rect = card.getBoundingClientRect();
      card.style.setProperty("--mx", (e.clientX - rect.left) + "px");
      card.style.setProperty("--my", (e.clientY - rect.top) + "px");
    });
  });

  var ghStarNumEl = document.getElementById("gh-star-num");
  if (ghStarNumEl && typeof fetch === "function") {
    fetch("https://api.github.com/repos/kelvindaniyar/iphone-link")
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (d) {
        if (d && typeof d.stargazers_count === "number" && d.stargazers_count > 0) {
          ghStarNumEl.textContent = String(d.stargazers_count);
        }
      })
      .catch(function () {});
  }

  /* --------------------------------------------------------------------------
     4. HERO 3D INTERACTIVE DEVICE STACK, 3D GLASS BUBBLES & AUTO-MORPHING UI
     -------------------------------------------------------------------------- */
  var HERO_ORDER = ["iphone17", "iphone11", "ipad", "gaming"];
  var HERO_META = {
    iphone17: {
      title: { id: "iPhone 17 Pro", en: "iPhone 17 Pro" },
      sub: { id: "iOS 27.2 · Dynamic Island · 60fps H.265", en: "iOS 27.2 · Dynamic Island · 60fps H.265" }
    },
    iphone11: {
      title: { id: "iPhone 11 Pro Max", en: "iPhone 11 Pro Max" },
      sub: { id: "iOS 27.2 · Notch Terkalibrasi · USB / Wi-Fi", en: "iOS 27.2 · Calibrated Notch · USB / Wi-Fi" }
    },
    ipad: {
      title: { id: "iPad (4:3)", en: "iPad (4:3)" },
      sub: { id: "iPadOS 27 · Mode Split View 924x832 & 564x832", en: "iPadOS 27 · Split View Mode 924x832 & 564x832" }
    },
    gaming: {
      title: { id: "Landscape Gaming 270 deg", en: "Landscape Gaming 270 deg" },
      sub: { id: "eFootball 2027 · 60 FPS H.265 & 120Hz HID", en: "eFootball 2027 · 60 FPS H.265 & 120Hz HID" }
    }
  };

  var activeHeroDev = "iphone17";
  var heroHovered = false;
  var heroMorphState = "bubbles";
  var heroStage = document.getElementById("hero-stack-stage");
  var heroCaptionTitle = document.getElementById("hero-caption-title");
  var heroCaptionText = document.getElementById("hero-caption-text");
  var heroPills = document.querySelectorAll("[data-hero-dev]");
  var heroDots = document.querySelectorAll("[data-stack-target]");
  var stackCards = document.querySelectorAll("[data-stack-device]");
  var heroBubble = document.getElementById("hero-liquid-bubble");
  var heroBubbleSm = document.getElementById("hero-liquid-bubble-sm");
  var heroBubblePulseTimer = null;
  var heroMorphTimer = null;
  var heroCarouselTimer = null;

  function triggerHeroBubblePulse() {
    [heroBubble, heroBubbleSm].forEach(function (el) {
      if (!el) return;
      el.classList.remove("is-pulsing");
      void el.offsetWidth;
      el.classList.add("is-pulsing");
    });
    clearTimeout(heroBubblePulseTimer);
    heroBubblePulseTimer = setTimeout(function () {
      if (heroBubble) heroBubble.classList.remove("is-pulsing");
      if (heroBubbleSm) heroBubbleSm.classList.remove("is-pulsing");
    }, 720);
  }

  function setHeroScreenMorphState(nextState) {
    heroMorphState = nextState === "widgets" ? "widgets" : "bubbles";
    if (heroStage) {
      heroStage.setAttribute("data-morph-state", heroMorphState);
    }
    return heroMorphState;
  }

  function startHeroMorphTimer() {
    clearInterval(heroMorphTimer);
    heroMorphTimer = setInterval(function () {
      setHeroScreenMorphState(heroMorphState === "bubbles" ? "widgets" : "bubbles");
    }, 2700);
  }

  window.toggleHeroScreenMorphState = function (forceState) {
    var next = (forceState === "bubbles" || forceState === "widgets")
      ? forceState
      : (heroMorphState === "bubbles" ? "widgets" : "bubbles");
    setHeroScreenMorphState(next);
    if (heroStage) {
      startHeroMorphTimer();
    }
    return next;
  };

  function updateHeroStack(devKey) {
    if (!HERO_META[devKey]) return;
    var changed = activeHeroDev !== devKey;
    activeHeroDev = devKey;
    var idx = HERO_ORDER.indexOf(devKey);
    var slots = ["front", "right", "back", "left"];

    stackCards.forEach(function (card) {
      var cKey = card.getAttribute("data-stack-device");
      var cIdx = HERO_ORDER.indexOf(cKey);
      var offset = (cIdx - idx + HERO_ORDER.length) % HERO_ORDER.length;
      var slot = slots[offset];
      card.setAttribute("data-pos", slot);
      card.classList.toggle("is-front", slot === "front");
    });

    heroPills.forEach(function (btn) {
      btn.classList.toggle("is-active", btn.getAttribute("data-hero-dev") === devKey);
    });
    heroDots.forEach(function (dot) {
      var isAct = dot.getAttribute("data-stack-target") === devKey;
      dot.classList.toggle("is-active", isAct);
      dot.setAttribute("aria-selected", isAct ? "true" : "false");
    });

    var meta = HERO_META[devKey];
    if (heroCaptionTitle) {
      heroCaptionTitle.textContent = meta.title[siteLang] || meta.title.id;
    }
    if (heroCaptionText) {
      heroCaptionText.textContent = meta.sub[siteLang] || meta.sub.id;
    }
    if (changed) {
      triggerHeroBubblePulse();
    }
  }

  function startHeroCarouselTimer() {
    clearInterval(heroCarouselTimer);
    heroCarouselTimer = setInterval(function () {
      if (!heroHovered) {
        var nextIdx = (HERO_ORDER.indexOf(activeHeroDev) + 1) % HERO_ORDER.length;
        updateHeroStack(HERO_ORDER[nextIdx]);
      }
    }, 5000);
  }

  heroPills.forEach(function (btn) {
    btn.addEventListener("click", function () {
      updateHeroStack(btn.getAttribute("data-hero-dev"));
      startHeroCarouselTimer();
    });
  });

  heroDots.forEach(function (dot) {
    dot.addEventListener("click", function () {
      updateHeroStack(dot.getAttribute("data-stack-target"));
      startHeroCarouselTimer();
    });
  });

  stackCards.forEach(function (card) {
    card.addEventListener("click", function () {
      var devKey = card.getAttribute("data-stack-device");
      if (devKey === activeHeroDev) {
        window.toggleHeroScreenMorphState();
      } else {
        updateHeroStack(devKey);
      }
      startHeroCarouselTimer();
    });
  });

  if (heroStage) {
    setHeroScreenMorphState("bubbles");
    heroStage.addEventListener("mouseenter", function () { heroHovered = true; });
    heroStage.addEventListener("mouseleave", function () {
      heroHovered = false;
      stackCards.forEach(function (c) {
        c.style.removeProperty("--tilt-x");
        c.style.removeProperty("--tilt-y");
      });
    });
    heroStage.addEventListener("pointermove", function (e) {
      var rect = heroStage.getBoundingClientRect();
      var nx = (e.clientX - rect.left) / Math.max(rect.width, 1) - 0.5;
      var ny = (e.clientY - rect.top) / Math.max(rect.height, 1) - 0.5;
      var frontCard = heroStage.querySelector('.stack-card[data-pos="front"]');
      if (frontCard) {
        frontCard.style.setProperty("--tilt-y", (nx * 16).toFixed(2) + "deg");
        frontCard.style.setProperty("--tilt-x", (-ny * 14).toFixed(2) + "deg");
        frontCard.style.setProperty("--gx", Math.round((nx + 0.5) * 100) + "%");
        frontCard.style.setProperty("--gy", Math.round((ny + 0.5) * 100) + "%");
      }
    });
    startHeroCarouselTimer();
    startHeroMorphTimer();
  }

  /* --------------------------------------------------------------------------
     5. LIVE INTERACTIVE APP SIMULATOR ("COBA SEBELUM MENGUNDUH")
     -------------------------------------------------------------------------- */
  var SIM_TEXTS = {
    id: {
      secAppearance: "TAMPILAN & BAHASA",
      secScale: "UKURAN & RASIO LAYAR",
      autoRotateLabel: "Putar Otomatis (Auto-Rotate)",
      secCrop: "POTONG TEPI (EDGE CROP)",
      saveCropBtn: "Simpan Crop",
      secAudio: "AUDIO TANPA KABEL (iPhone -> PC)",
      audioPcBtn: "Di PC Ini",
      audioPhoneBtn: "Hanya di iPhone",
      volLabel: "Volume PC",
      bufLow: "Buffer: Rendah",
      bufHigh: "Tinggi",
      lipActive: "Aktif",
      secOcr: "KEYBOARD & PINTASAN OCR",
      btnPaste: "Tempel PC (Alt+V)",
      btnOcr: "Salin OCR (Alt+C)",
      btnCache: "Clear Cache (Alt+R)",
      btnKb: "Keyboard (Ctrl+K)",
      secStealth: "KONTROL LAYAR & MODE REDUP",
      btnWake: "Buka (Alt+U)",
      btnLock: "Kunci (Alt+L)",
      btnStealth: "Stealth Dim Mode (Alt+D)",
      pinLbl: "PIN 6-digit",
      btnWipe: "Hapus PIN & Data Privasi",
      secUpdater: "KONEKSI & PEMBARUAN PERANGKAT LUNAK",
      connUsb: "Kabel",
      chanStable: "Stabil",
      autoUpdLbl: "Cek otomatis saat dibuka",
      btnCheckUpd: "Periksa Pembaruan",
      updReady: "v4.0.0 Native Rust Siap",
      updLatest: "iPhone Link v4.0.0 sudah versi terbaru!",
      themeDark: "Gelap",
      themeLight: "Terang",
      langId: "ID Indonesia",
      langEn: "EN English",
      liquidGlassLbl: "Liquid Glass",
      glassThickLbl: "Ketebalan Kaca (Glass Thickness)"
    },
    en: {
      secAppearance: "APPEARANCE & LANGUAGE",
      secScale: "SIZE & ASPECT RATIO",
      autoRotateLabel: "Auto-Rotate Landscape",
      secCrop: "EDGE CROP (TRIM BEZEL)",
      saveCropBtn: "Save Crop",
      secAudio: "WIRELESS AUDIO (iPhone -> PC)",
      audioPcBtn: "On This PC",
      audioPhoneBtn: "iPhone Only",
      volLabel: "PC Volume",
      bufLow: "Buffer: Low",
      bufHigh: "High",
      lipActive: "Active",
      secOcr: "KEYBOARD & OCR SHORTCUTS",
      btnPaste: "Paste PC (Alt+V)",
      btnOcr: "Copy OCR (Alt+C)",
      btnCache: "Clear Cache (Alt+R)",
      btnKb: "Keyboard (Ctrl+K)",
      secStealth: "SCREEN & STEALTH CONTROL",
      btnWake: "Wake/Unlock (Alt+U)",
      btnLock: "Lock (Alt+L)",
      btnStealth: "Stealth Dim Mode (Alt+D)",
      pinLbl: "6-digit PIN",
      btnWipe: "Clear PIN & Privacy Data",
      secUpdater: "CONNECTION & SOFTWARE UPDATE",
      connUsb: "USB Cable",
      chanStable: "Stable",
      autoUpdLbl: "Check automatically on startup",
      btnCheckUpd: "Check for Updates",
      updReady: "v4.0.0 Native Rust Ready",
      updLatest: "iPhone Link v4.0.0 is up to date!",
      themeDark: "Dark",
      themeLight: "Light",
      langId: "ID Indonesia",
      langEn: "EN English",
      liquidGlassLbl: "Liquid Glass",
      glassThickLbl: "Glass Thickness"
    }
  };

  var SIM_SCREENS_CYCLE = ["home", "notes", "instagram", "youtube", "efootball"];

  var simState = {
    device: "iphone17",
    conn: "usb",
    theme: "dark",
    lang: "id",
    drawerOpen: true,
    fit: "autofit",
    aspect: "auto",
    autoRotate: true,
    cropPx: 0,
    audioTarget: "pc",
    volume: 85,
    buffer: "normal",
    lipsync: "active",
    btA2dp: false,
    keyboardOpen: false,
    stealth: false,
    landscape: false,
    stopped: false,
    screen: "home",
    channel: "stable",
    notesPasted: false,
    liquidGlass: true,
    glassThickness: 58
  };

  var simWindowEl = document.getElementById("sim-app-window");
  var simBezelEl = document.getElementById("sim-bezel");
  var simScreenBodyEl = document.getElementById("sim-screen-body");
  var simReadoutEl = document.getElementById("sim-device-readout");
  var simBadgeMirrorEl = document.getElementById("sim-badge-mirror");
  var simStealthVeilEl = document.getElementById("sim-stealth-veil");
  var simKbEl = document.getElementById("sim-ios-kb");
  var simCropFrameEl = document.getElementById("sim-crop-frame");
  var simToastEl = document.getElementById("sim-toast");
  var simToastTimer = null;

  var simDrawerEl = document.getElementById("sim-drawer");
  var simLiquidGlassBtn = document.getElementById("sim-liquid-glass-toggle");
  var simGlassSlider = document.getElementById("sim-glass-thickness");
  var simGlassValEl = document.getElementById("sim-glass-thick-val");

  function applySimGlassPhysics() {
    if (!simWindowEl) return;
    var pct = Math.max(0, Math.min(100, Number(simState.glassThickness)));
    var on = Boolean(simState.liquidGlass);
    simWindowEl.setAttribute("data-liquid-glass", on ? "on" : "off");
    simWindowEl.setAttribute("data-glass-thickness", String(pct));

    if (simLiquidGlassBtn) {
      simLiquidGlassBtn.textContent = on ? "ON" : "OFF";
      simLiquidGlassBtn.classList.toggle("is-active", on);
    }
    if (simGlassSlider) {
      simGlassSlider.value = String(pct);
      simGlassSlider.disabled = !on;
    }
    if (simGlassValEl) {
      simGlassValEl.textContent = pct + "%";
    }

    if (!on) {
      simWindowEl.style.setProperty("--sim-glass-blur", "0px");
      simWindowEl.style.setProperty("--sim-glass-alpha", "0.98");
      simWindowEl.style.setProperty("--sim-glass-drawer-alpha", "0.98");
      simWindowEl.style.backdropFilter = "none";
      simWindowEl.style.webkitBackdropFilter = "none";
      if (simDrawerEl) {
        simDrawerEl.style.backdropFilter = "none";
        simDrawerEl.style.webkitBackdropFilter = "none";
      }
      return;
    }

    var blurPx = Math.round(10 + (pct * 0.30));
    var winAlpha = simState.theme === "light"
      ? (0.94 - (pct / 100) * 0.24).toFixed(3)
      : (0.94 - (pct / 100) * 0.26).toFixed(3);
    var drawerAlpha = simState.theme === "light"
      ? (0.92 - (pct / 100) * 0.26).toFixed(3)
      : (0.92 - (pct / 100) * 0.28).toFixed(3);

    var filterCss = "blur(" + blurPx + "px) saturate(" + Math.round(145 + pct * 0.7) + "%)";
    simWindowEl.style.setProperty("--sim-glass-blur", blurPx + "px");
    simWindowEl.style.setProperty("--sim-glass-alpha", winAlpha);
    simWindowEl.style.setProperty("--sim-glass-drawer-alpha", drawerAlpha);
    simWindowEl.style.backdropFilter = filterCss;
    simWindowEl.style.webkitBackdropFilter = filterCss;
    if (simDrawerEl) {
      simDrawerEl.style.backdropFilter = filterCss;
      simDrawerEl.style.webkitBackdropFilter = filterCss;
    }
  }

  function showSimToast(msg) {
    if (!simToastEl) return;
    simToastEl.textContent = msg;
    simToastEl.classList.add("show");
    clearTimeout(simToastTimer);
    simToastTimer = setTimeout(function () {
      simToastEl.classList.remove("show");
    }, 2200);
  }

  function applySimDrawerTranslations() {
    var dict = SIM_TEXTS[simState.lang] || SIM_TEXTS.id;
    document.querySelectorAll("[data-sim-txt]").forEach(function (el) {
      var k = el.getAttribute("data-sim-txt");
      if (dict[k]) el.textContent = dict[k];
    });
    if (simWindowEl) simWindowEl.setAttribute("data-sim-lang", simState.lang);
  }

  function renderSimViewport() {
    if (!simScreenBodyEl || !simBezelEl || !simWindowEl) return;

    simWindowEl.setAttribute("data-sim-theme", simState.theme);
    simWindowEl.setAttribute("data-sim-drawer", simState.drawerOpen ? "open" : "closed");

    var isLandscape = simState.landscape || simState.device === "gaming" || simState.screen === "efootball" || (simState.autoRotate && simState.screen === "youtube");
    var isIpad = !isLandscape && (simState.device === "ipad" || simState.aspect === "ipad");

    simBezelEl.className = "sim-bezel";
    if (isLandscape) {
      simBezelEl.classList.add("mode-landscape");
    } else if (isIpad) {
      simBezelEl.classList.add("mode-ipad");
    } else if (simState.device === "iphone11") {
      simBezelEl.classList.add("mode-iphone11");
    }

    if (simCropFrameEl) {
      simCropFrameEl.style.setProperty("--sim-crop", Math.round(simState.cropPx * 0.45) + "px");
    }
    if (simStealthVeilEl) {
      simStealthVeilEl.classList.toggle("is-active", simState.stealth);
    }
    if (simKbEl) {
      simKbEl.classList.toggle("is-open", simState.keyboardOpen && !simState.stopped);
    }

    var devLabel = "iPhone 17 Pro (1178x2598)";
    if (simState.device === "iphone11") devLabel = "iPhone 11 Pro Max (1242x2688)";
    if (isIpad) devLabel = "iPad Pro 4:3 (924x832)";
    if (isLandscape) devLabel = "eFootball 2027 (270 deg)";
    var connLabel = simState.conn === "wifi" ? "Wi-Fi Low-Latency" : "USB 60fps";
    if (simReadoutEl) simReadoutEl.textContent = devLabel + " - " + connLabel;

    if (simBadgeMirrorEl) {
      if (simState.stopped) {
        simBadgeMirrorEl.textContent = simState.lang === "en" ? "Standby" : "Siap";
      } else {
        simBadgeMirrorEl.textContent = "Mirroring";
      }
    }

    var isEn = simState.lang === "en";

    if (simState.stopped) {
      var devSub = simState.conn === "wifi" ? "192.168.100.38 · Wi-Fi Ready" : "192.168.100.38 · USB Connected";
      simScreenBodyEl.innerHTML =
        '<div class="sim-launchpad-wrap">' +
        '<img src="logo.svg" alt="iPhone Link" width="38" height="38" style="border-radius:10px;">' +
        '<strong style="font-size:0.86rem;">' + (isEn ? "iPhone 17 Pro ready" : "iPhone 17 Pro siap") + '</strong>' +
        '<span style="font-size:0.70rem;opacity:0.78;">' + (isEn ? "Unlock, then click Start mirroring." : "Buka kunci, lalu tekan Mulai mirroring.") + '</span>' +
        '<div class="sim-launchpad-device">' +
          '<span class="sim-launchpad-dot"></span>' +
          '<div style="flex:1;min-width:0;">' +
            '<div style="font-size:0.72rem;font-weight:700;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">iPhone</div>' +
            '<div style="font-size:0.62rem;opacity:0.78;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">' + devSub + '</div>' +
          '</div>' +
        '</div>' +
        '<div class="sim-launchpad-transport" role="group">' +
          '<button type="button" class="sim-launchpad-pill' + (simState.conn === "usb" ? " is-active" : "") + '" data-sim-lp-conn="usb">' + (isEn ? "Cable (USB)" : "Kabel (USB)") + '</button>' +
          '<button type="button" class="sim-launchpad-pill' + (simState.conn === "wifi" ? " is-active" : "") + '" data-sim-lp-conn="wifi">Wi-Fi</button>' +
        '</div>' +
        '<button type="button" class="sim-launchpad-start-pill" id="sim-launchpad-start">' +
        (isEn ? "Start mirroring" : "Mulai mirroring") + '</button></div>';
      document.querySelectorAll("[data-sim-lp-conn]").forEach(function (cBtn) {
        cBtn.addEventListener("click", function () {
          var nextC = cBtn.getAttribute("data-sim-lp-conn");
          if (nextC) syncSimConnection(nextC);
        });
      });
      var lpBtn = document.getElementById("sim-launchpad-start");
      if (lpBtn) {
        lpBtn.addEventListener("click", function () {
          simState.stopped = false;
          renderSimViewport();
          showSimToast(isEn ? "Mirroring resumed (60fps)" : "Mirroring dilanjutkan (60fps)");
        });
      }
      return;
    }

    if (simState.screen === "lockscreen") {
      simScreenBodyEl.innerHTML =
        '<div class="sim-subscreen" style="align-items:center;justify-content:center;text-align:center;background:linear-gradient(160deg,#09090B,#111827);">' +
        '<div style="font-size:0.72rem;opacity:0.8;">' + (isEn ? "Friday, 14 Nov" : "Jumat, 14 Nov") + '</div>' +
        '<div style="font-size:2.3rem;font-weight:800;letter-spacing:-0.03em;">09:41</div>' +
        '<div style="font-size:0.74rem;color:#A1A1AA;">' + (isEn ? "Locked - Click Wake/Unlock (Alt+U)" : "Terkunci - Klik Buka (Alt+U)") + '</div>' +
        '<div id="sim-lock-dots" style="font-family:var(--font-mono);font-size:1rem;letter-spacing:0.25em;margin:8px 0;">o o o o o o</div>' +
        '<button type="button" class="sim-act-btn is-active" id="sim-quick-unlock" style="justify-content:center;">' + (isEn ? "Unlock with PIN" : "Buka dengan PIN") + '</button>' +
        '</div>';
      var quBtn = document.getElementById("sim-quick-unlock");
      if (quBtn) quBtn.addEventListener("click", triggerSimUnlock);
      return;
    }

    if (simState.screen === "notes") {
      var noteExtra = simState.notesPasted
        ? '<p style="margin:6px 0 0;color:#2997FF;font-weight:700;">' + (isEn ? "[Pasted via Alt+V] Hello from Windows 11!" : "[Ditempel via Alt+V] Halo dari PC Windows 11!") + '</p>'
        : "";
      simScreenBodyEl.innerHTML =
        '<div class="sim-subscreen">' +
        '<div style="display:flex;justify-content:space-between;align-items:center;">' +
        '<strong style="font-size:0.8rem;">Apple Notes (OCR Demo)</strong>' +
        '<button type="button" class="sim-qnav-btn" data-open-screen="home" style="padding:3px 8px;font-size:0.68rem;">Home</button></div>' +
        '<div class="sim-notes-box">' +
        '<strong>' + (isEn ? "Meeting & Architecture Notes" : "Catatan Kuliah & Arsitektur v4") + '</strong>' +
        '<p style="margin:6px 0;">' + (isEn ? "Native Rust reduces binary overhead by 93% while locking Acrylic blur." : "Mesin Native Rust memangkas ukuran biner 93% serta mengunci DWM Acrylic Blur.") + '</p>' +
        '<p style="margin:6px 0;">' + (isEn ? "Click Copy OCR (Alt+C) on the right drawer to copy cleanly to Windows!" : "Klik Salin OCR (Alt+C) di drawer kanan untuk menyalin paragraf ke PC!") + '</p>' +
        noteExtra +
        '</div></div>';
      bindScreenButtons();
      return;
    }

    if (simState.screen === "instagram") {
      simScreenBodyEl.innerHTML =
        '<div class="sim-subscreen" style="background:linear-gradient(160deg,#18181B,#0F172A);">' +
        '<div style="display:flex;justify-content:space-between;align-items:center;">' +
        '<strong style="font-size:0.8rem;">Instagram Reels</strong>' +
        '<button type="button" class="sim-qnav-btn" data-open-screen="home" style="padding:3px 8px;font-size:0.68rem;">Home</button></div>' +
        '<div style="flex:1;border-radius:12px;background:rgba(255,255,255,0.06);border:1px solid rgba(255,255,255,0.1);display:flex;flex-direction:column;justify-content:flex-end;padding:12px;">' +
        '<div style="font-size:0.78rem;font-weight:750;">@kelvin.daniyar</div>' +
        '<div style="font-size:0.7rem;color:#D4D4D8;">iPhone Link v4.0.0 Native Rust Spatial Preview on Windows 11</div>' +
        '</div></div>';
      bindScreenButtons();
      return;
    }

    if (simState.screen === "efootball") {
      simScreenBodyEl.innerHTML =
        '<div class="sim-subscreen" style="background:linear-gradient(180deg,#064E3B,#047857);">' +
        '<div style="display:flex;justify-content:space-between;font-size:0.74rem;font-weight:750;">' +
        '<span>eFootball 2027 (270 deg Auto-Rotate)</span>' +
        '<button type="button" class="sim-qnav-btn" data-open-screen="home" style="padding:2px 8px;font-size:0.66rem;">Exit</button></div>' +
        '<div class="mini-pitch" style="flex:1;">' +
        '<span style="position:relative;z-index:2;font-weight:800;font-size:0.86rem;background:rgba(9,9,11,0.68);padding:4px 14px;border-radius:999px;">HOME 3 - 1 AWAY | 60 FPS</span>' +
        '</div></div>';
      bindScreenButtons();
      return;
    }

    if (simState.screen === "youtube") {
      simScreenBodyEl.innerHTML =
        '<div class="sim-subscreen" style="background:#09090B;">' +
        '<div style="display:flex;justify-content:space-between;align-items:center;">' +
        '<strong style="font-size:0.78rem;">YouTube Fullscreen</strong>' +
        '<button type="button" class="sim-qnav-btn" data-open-screen="home" style="padding:2px 8px;font-size:0.66rem;">Home</button></div>' +
        '<div style="flex:1;border-radius:12px;background:linear-gradient(135deg,#DC2626,#18181B);display:flex;align-items:center;justify-content:center;color:#FFFFFF;">' +
        '<svg class="ui-icon ico-lg" style="width:28px;height:28px;" viewBox="0 0 24 24"><polygon points="6 3 20 12 6 21 6 3"/></svg></div>' +
        '<div style="height:5px;border-radius:999px;background:rgba(255,255,255,0.2);overflow:hidden;"><div style="width:62%;height:100%;background:#EF4444;"></div></div>' +
        '</div>';
      bindScreenButtons();
      return;
    }

    if (simState.screen === "control") {
      simScreenBodyEl.innerHTML =
        '<div class="sim-subscreen" style="justify-content:space-between;">' +
        '<div style="display:flex;justify-content:space-between;align-items:center;"><strong style="font-size:0.78rem;">Control Center</strong><button type="button" class="sim-qnav-btn" data-open-screen="home" style="padding:2px 8px;font-size:0.66rem;">Close</button></div>' +
        '<div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;flex:1;margin-top:6px;">' +
        '<div style="border-radius:14px;background:rgba(41,151,255,0.16);border:1px solid rgba(41,151,255,0.28);padding:10px;font-size:0.7rem;"><strong>AirPlay / Mirroring</strong><div>' + connLabel + ' Active</div></div>' +
        '<div style="border-radius:14px;background:rgba(16,185,129,0.16);border:1px solid rgba(16,185,129,0.28);padding:10px;font-size:0.7rem;"><strong>Audio Output</strong><div>' + (simState.audioTarget === "pc" ? "Windows PC (" + simState.volume + "%)" : "iPhone Speakers") + '</div></div>' +
        '</div></div>';
      bindScreenButtons();
      return;
    }

    if (simState.screen === "recents") {
      simScreenBodyEl.innerHTML =
        '<div class="sim-subscreen">' +
        '<div style="display:flex;justify-content:space-between;align-items:center;"><strong style="font-size:0.78rem;">App Switcher</strong><button type="button" class="sim-qnav-btn" data-open-screen="home" style="padding:2px 8px;font-size:0.66rem;">Home</button></div>' +
        '<div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;flex:1;margin-top:6px;">' +
        '<button type="button" class="sim-act-btn" data-open-screen="notes">Apple Notes OCR</button>' +
        '<button type="button" class="sim-act-btn" data-open-screen="instagram">Instagram Reels</button>' +
        '<button type="button" class="sim-act-btn" data-open-screen="efootball">eFootball 2027</button>' +
        '<button type="button" class="sim-act-btn" data-open-screen="youtube">YouTube Full</button>' +
        '</div></div>';
      bindScreenButtons();
      return;
    }

    // Default: Home Screen with clickable vector SVG app squircles + dock
    var tilesHtml =
      tileMarkup("notes", "Notes OCR", "linear-gradient(180deg,#F59E0B,#D97706)", "notes", true) +
      tileMarkup("instagram", "Instagram", "linear-gradient(135deg,#F97316,#EA580C)", "instagram", true) +
      tileMarkup("efootball", "eFootball", "linear-gradient(180deg,#10B981,#059669)", "efootball", true) +
      tileMarkup("youtube", "YouTube", "linear-gradient(180deg,#EF4444,#DC2626)", "youtube", true) +
      tileMarkup("control", "Controls", "linear-gradient(180deg,#3F3F46,#27272A)", "control", false) +
      tileMarkup("recents", "Switcher", "linear-gradient(180deg,#2997FF,#0071E3)", "recents", false) +
      tileMarkup("lockscreen", "Lock", "linear-gradient(180deg,#27272A,#18181B)", "lockscreen", false) +
      tileMarkup("home", "Safari", "linear-gradient(180deg,#38BDF8,#0284C7)", "safari", false) +
      tileMarkup("home", "Photos", "linear-gradient(180deg,#0EA5E9,#0369A1)", "photos", false) +
      tileMarkup("home", "Calendar", "#FAFAFA", "calendar", false) +
      tileMarkup("home", "Clock", "#18181B", "clock", false) +
      tileMarkup("home", "Files", "linear-gradient(180deg,#0071E3,#005BB5)", "files", false);

    simScreenBodyEl.innerHTML =
      '<div class="sim-app-grid">' + tilesHtml + '</div>' +
      '<div class="sim-dock">' +
      dockBtn("notes", "linear-gradient(180deg,#F59E0B,#D97706)", "notes") +
      dockBtn("instagram", "linear-gradient(135deg,#F97316,#EA580C)", "instagram") +
      dockBtn("efootball", "linear-gradient(180deg,#10B981,#059669)", "efootball") +
      dockBtn("control", "linear-gradient(180deg,#3F3F46,#27272A)", "control") +
      '</div>';

    bindScreenButtons();
  }

  function tileMarkup(target, label, bg, iconKey, hl) {
    var txtColor = bg === "#FAFAFA" ? "#1D1D1F" : "#FFFFFF";
    var svg = APP_SVGS[iconKey] || APP_SVGS.notes;
    return '<button type="button" class="sim-app-tile' + (hl ? ' highlight-demo' : '') + '" data-open-screen="' + target + '">' +
      '<span class="tile-icon" style="background:' + bg + ';color:' + txtColor + ';">' + svg + '</span>' +
      '<span class="tile-name">' + label + '</span></button>';
  }

  function dockBtn(target, bg, iconKey) {
    var svg = APP_SVGS[iconKey] || APP_SVGS.notes;
    return '<button type="button" class="sim-app-tile" data-open-screen="' + target + '" aria-label="' + target + '">' +
      '<span class="tile-icon" style="width:36px;height:36px;background:' + bg + ';color:#FFFFFF;">' + svg + '</span></button>';
  }

  function bindScreenButtons() {
    if (!simScreenBodyEl) return;
    simScreenBodyEl.querySelectorAll("[data-open-screen]").forEach(function (btn) {
      btn.addEventListener("click", function () {
        var nextScreen = btn.getAttribute("data-open-screen");
        if (nextScreen === "efootball") {
          simState.landscape = true;
        } else if (nextScreen === "home") {
          simState.landscape = false;
        }
        simState.screen = nextScreen;
        renderSimViewport();
      });
    });
  }

  function triggerSimUnlock() {
    var dots = document.getElementById("sim-lock-dots");
    if (dots) dots.textContent = "* * * * * *";
    setTimeout(function () {
      simState.screen = "home";
      renderSimViewport();
      showSimToast(simState.lang === "en" ? "Unlocked via PIN (Alt+U)" : "Terbuka via PIN (Alt+U)");
    }, 320);
  }

  function setActiveInGroup(attrName, activeVal) {
    document.querySelectorAll("[" + attrName + "]").forEach(function (btn) {
      btn.classList.toggle("is-active", btn.getAttribute(attrName) === activeVal);
    });
  }

  // Simulator Top Bar Device & Connection Selectors
  document.querySelectorAll("[data-sim-device]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var dev = btn.getAttribute("data-sim-device");
      simState.device = dev;
      simState.stopped = false;
      simState.landscape = (dev === "gaming");
      simState.screen = dev === "gaming" ? "efootball" : "home";
      setActiveInGroup("data-sim-device", dev);
      if (dev === "ipad") {
        setActiveInGroup("data-sim-aspect", "ipad");
        simState.aspect = "ipad";
      } else {
        setActiveInGroup("data-sim-aspect", "auto");
        simState.aspect = "auto";
      }
      renderSimViewport();
    });
  });

  function syncSimConnection(conn) {
    simState.conn = conn;
    setActiveInGroup("data-sim-conn", conn);
    setActiveInGroup("data-sim-conn-drawer", conn);
    renderSimViewport();
  }

  document.querySelectorAll("[data-sim-conn]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      syncSimConnection(btn.getAttribute("data-sim-conn"));
      showSimToast("Connection: " + simState.conn.toUpperCase());
    });
  });
  document.querySelectorAll("[data-sim-conn-drawer]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      syncSimConnection(btn.getAttribute("data-sim-conn-drawer"));
      showSimToast("Connection: " + simState.conn.toUpperCase());
    });
  });

  // Topbar Buttons
  var sRestart = document.getElementById("sim-btn-restart");
  var sStop = document.getElementById("sim-btn-stop");
  var sRotate = document.getElementById("sim-btn-rotate");
  var sSettings = document.getElementById("sim-btn-settings");
  var sFs = document.getElementById("sim-btn-fs");
  var sMin = document.getElementById("sim-btn-min");
  var sClose = document.getElementById("sim-btn-close");

  if (sRestart) sRestart.addEventListener("click", function () {
    simState.stopped = false;
    renderSimViewport();
    showSimToast("Keyframe PLI Restarted (0ms)");
  });
  if (sStop) sStop.addEventListener("click", function () {
    simState.stopped = !simState.stopped;
    renderSimViewport();
  });
  if (sRotate) sRotate.addEventListener("click", function () {
    simState.landscape = !simState.landscape;
    renderSimViewport();
    showSimToast(simState.landscape ? "Landscape 270 deg (Alt+H)" : "Portrait 0 deg (Alt+H)");
  });
  if (sSettings) sSettings.addEventListener("click", function () {
    simState.drawerOpen = !simState.drawerOpen;
    sSettings.classList.toggle("is-active", simState.drawerOpen);
    renderSimViewport();
  });
  if (sFs) sFs.addEventListener("click", function () {
    showSimToast("1:1 Spatial Mirror Viewport");
  });
  if (sMin) sMin.addEventListener("click", function () {
    showSimToast("Window Hidden to Windows System Tray");
  });
  if (sClose) sClose.addEventListener("click", function () {
    simState.stopped = true;
    renderSimViewport();
  });

  // Quick Nav Bar
  document.querySelectorAll("[data-qnav]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var act = btn.getAttribute("data-qnav");
      simState.stopped = false;
      if (act === "home") {
        simState.screen = "home";
        simState.landscape = false;
      } else if (act === "recent") {
        simState.screen = "recents";
      } else {
        var idx = SIM_SCREENS_CYCLE.indexOf(simState.screen);
        if (idx < 0) idx = 0;
        var nextIdx = act === "next"
          ? (idx + 1) % SIM_SCREENS_CYCLE.length
          : (idx - 1 + SIM_SCREENS_CYCLE.length) % SIM_SCREENS_CYCLE.length;
        simState.screen = SIM_SCREENS_CYCLE[nextIdx];
        simState.landscape = simState.screen === "efootball";
      }
      renderSimViewport();
    });
  });

  // Drawer Theme, Liquid Glass & Lang Buttons
  document.querySelectorAll("[data-sim-theme-btn]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.theme = btn.getAttribute("data-sim-theme-btn");
      setActiveInGroup("data-sim-theme-btn", simState.theme);
      renderSimViewport();
      applySimGlassPhysics();
    });
  });

  if (simLiquidGlassBtn) {
    simLiquidGlassBtn.addEventListener("click", function () {
      simState.liquidGlass = !simState.liquidGlass;
      applySimGlassPhysics();
      showSimToast("Liquid Glass: " + (simState.liquidGlass ? "ON" : "OFF"));
    });
  }

  if (simGlassSlider) {
    var onGlassSliderInput = function () {
      simState.glassThickness = Number(simGlassSlider.value);
      applySimGlassPhysics();
    };
    simGlassSlider.addEventListener("input", onGlassSliderInput);
    simGlassSlider.addEventListener("change", function () {
      onGlassSliderInput();
      showSimToast("Glass Thickness: " + simState.glassThickness + "%");
    });
  }

  document.querySelectorAll("[data-sim-lang-btn]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.lang = btn.getAttribute("data-sim-lang-btn");
      setActiveInGroup("data-sim-lang-btn", simState.lang);
      applySimDrawerTranslations();
      renderSimViewport();
    });
  });

  // Fit, Aspect, Auto-Rotate, Crop
  document.querySelectorAll("[data-sim-fit]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.fit = btn.getAttribute("data-sim-fit");
      setActiveInGroup("data-sim-fit", simState.fit);
      showSimToast("Fit: " + btn.textContent);
    });
  });
  document.querySelectorAll("[data-sim-aspect]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.aspect = btn.getAttribute("data-sim-aspect");
      setActiveInGroup("data-sim-aspect", simState.aspect);
      if (simState.aspect === "ipad") {
        simState.device = "ipad";
        simState.landscape = false;
        setActiveInGroup("data-sim-device", "ipad");
      } else if (simState.aspect === "iphone" && simState.device === "ipad") {
        simState.device = "iphone17";
        setActiveInGroup("data-sim-device", "iphone17");
      }
      renderSimViewport();
    });
  });

  var arBtn = document.getElementById("sim-autorotate-toggle");
  if (arBtn) arBtn.addEventListener("click", function () {
    simState.autoRotate = !simState.autoRotate;
    arBtn.textContent = simState.autoRotate ? "ON" : "OFF";
    arBtn.classList.toggle("is-active", simState.autoRotate);
    renderSimViewport();
  });

  var cropSlider = document.getElementById("sim-crop-slider");
  var cropValEl = document.getElementById("sim-crop-val");
  var saveCropBtn = document.getElementById("sim-save-crop");
  if (cropSlider) cropSlider.addEventListener("input", function () {
    simState.cropPx = parseInt(cropSlider.value, 10) || 0;
    if (cropValEl) cropValEl.textContent = simState.cropPx + " px";
    renderSimViewport();
  });
  if (saveCropBtn) saveCropBtn.addEventListener("click", function () {
    showSimToast("Edge Crop Saved (" + simState.cropPx + " px)");
  });

  // Audio Controls
  document.querySelectorAll("[data-sim-audio]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.audioTarget = btn.getAttribute("data-sim-audio");
      setActiveInGroup("data-sim-audio", simState.audioTarget);
      renderSimViewport();
    });
  });
  var volSlider = document.getElementById("sim-vol-slider");
  var volValEl = document.getElementById("sim-vol-val");
  if (volSlider) volSlider.addEventListener("input", function () {
    simState.volume = parseInt(volSlider.value, 10) || 0;
    if (volValEl) volValEl.textContent = simState.volume + "%";
  });
  document.querySelectorAll("[data-sim-buf]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.buffer = btn.getAttribute("data-sim-buf");
      setActiveInGroup("data-sim-buf", simState.buffer);
    });
  });
  document.querySelectorAll("[data-sim-lipsync]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.lipsync = btn.getAttribute("data-sim-lipsync");
      setActiveInGroup("data-sim-lipsync", simState.lipsync);
    });
  });
  var btToggleBtn = document.getElementById("sim-bt-toggle");
  if (btToggleBtn) btToggleBtn.addEventListener("click", function () {
    simState.btA2dp = !simState.btA2dp;
    btToggleBtn.textContent = simState.btA2dp ? "BT A2DP: ON" : "BT A2DP: OFF";
    btToggleBtn.classList.toggle("is-active", simState.btA2dp);
    showSimToast(simState.btA2dp ? "Bluetooth A2DP Sink Connected" : "Bluetooth A2DP Disconnected");
  });

  // Keyboard & OCR Controls
  var sPaste = document.getElementById("sim-btn-paste");
  var sOcr = document.getElementById("sim-btn-ocr");
  var sClearCache = document.getElementById("sim-btn-clearcache");
  var sKbBtn = document.getElementById("sim-btn-kb");

  if (sPaste) sPaste.addEventListener("click", function () {
    simState.notesPasted = true;
    simState.screen = "notes";
    simState.stopped = false;
    renderSimViewport();
    showSimToast("HID Text Pasted to iPhone (Alt+V)");
  });
  if (sOcr) sOcr.addEventListener("click", function () {
    simState.screen = "notes";
    simState.stopped = false;
    renderSimViewport();
    showSimToast("Copied to PC!");
  });
  if (sClearCache) sClearCache.addEventListener("click", function () {
    showSimToast("OCR Stitching Cache Cleared!");
  });
  if (sKbBtn) sKbBtn.addEventListener("click", function () {
    simState.keyboardOpen = !simState.keyboardOpen;
    sKbBtn.classList.toggle("is-active", simState.keyboardOpen);
    renderSimViewport();
  });

  // Stealth & Lock Controls
  var sWake = document.getElementById("sim-btn-wake");
  var sLock = document.getElementById("sim-btn-lock");
  var sStealth = document.getElementById("sim-btn-stealth");
  var sPinInput = document.getElementById("sim-pin-input");
  var sWipe = document.getElementById("sim-btn-wipe");

  if (sWake) sWake.addEventListener("click", triggerSimUnlock);
  if (sLock) sLock.addEventListener("click", function () {
    simState.stealth = false;
    if (sStealth) sStealth.classList.remove("is-active");
    simState.screen = "lockscreen";
    simState.stopped = false;
    renderSimViewport();
    showSimToast("iPhone Locked (Alt+L)");
  });
  if (sStealth) sStealth.addEventListener("click", function () {
    simState.stealth = !simState.stealth;
    sStealth.classList.toggle("is-active", simState.stealth);
    renderSimViewport();
    showSimToast(simState.stealth ? "Stealth Dim 0% ON (Alt+D)" : "Brightness Restored ~25%");
  });
  if (sWipe) sWipe.addEventListener("click", function () {
    if (sPinInput) sPinInput.value = "";
    showSimToast("PIN & Local Privacy Data Wiped!");
  });

  // Updater Card
  document.querySelectorAll("[data-sim-channel]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      simState.channel = btn.getAttribute("data-sim-channel");
      setActiveInGroup("data-sim-channel", simState.channel);
    });
  });
  var sCheckUpd = document.getElementById("sim-btn-check-update");
  var sUpdStatus = document.getElementById("sim-upd-status");
  if (sCheckUpd) sCheckUpd.addEventListener("click", function () {
    var msg = (SIM_TEXTS[simState.lang] || SIM_TEXTS.id).updLatest;
    if (sUpdStatus) sUpdStatus.textContent = msg;
    showSimToast(msg);
  });

  /* --------------------------------------------------------------------------
     6. FULL SITE LANGUAGE SWITCHER (ID <-> EN)
     -------------------------------------------------------------------------- */
  var langIdBtn = document.getElementById("btn-lang-id");
  var langEnBtn = document.getElementById("btn-lang-en");

  function applySiteLanguage(lang) {
    siteLang = lang === "en" ? "en" : "id";
    rootHtml.setAttribute("lang", siteLang);
    rootHtml.setAttribute("data-lang", siteLang);

    if (langIdBtn) langIdBtn.classList.toggle("is-active", siteLang === "id");
    if (langEnBtn) langEnBtn.classList.toggle("is-active", siteLang === "en");

    document.querySelectorAll("[data-id][data-en]").forEach(function (el) {
      el.textContent = el.getAttribute("data-" + siteLang);
    });

    var fbDev = document.getElementById("fb-device");
    var fbMsg = document.getElementById("fb-msg");
    if (fbDev) fbDev.placeholder = "iPhone 17 Pro / iPhone 11 Pro Max / iPad (iOS 27.2)";
    if (fbMsg) fbMsg.placeholder = siteLang === "en" ? "Share your experience, feature ideas, or feedback..." : "Ceritakan pengalaman, ide fitur, atau hal yang ingin ditingkatkan...";

    updateHeroStack(activeHeroDev);
    simState.lang = siteLang;
    setActiveInGroup("data-sim-lang-btn", siteLang);
    applySimDrawerTranslations();
    renderSimViewport();
    applySimGlassPhysics();
    requestAnimationFrame(syncNavPillToActive);

    try {
      localStorage.setItem("il_site_lang", siteLang);
    } catch (e) {}
  }

  if (langIdBtn) langIdBtn.addEventListener("click", function () { applySiteLanguage("id"); });
  if (langEnBtn) langEnBtn.addEventListener("click", function () { applySiteLanguage("en"); });

  /* --------------------------------------------------------------------------
     7. SHORTCUTS FILTER TABS, SHA-256 COPY, FEEDBACK FORM & FAQ
     -------------------------------------------------------------------------- */
  document.querySelectorAll("[data-sc-filter]").forEach(function (tab) {
    tab.addEventListener("click", function () {
      var cat = tab.getAttribute("data-sc-filter");
      setActiveInGroup("data-sc-filter", cat);
      document.querySelectorAll(".shortcut-row").forEach(function (row) {
        row.hidden = (cat !== "all" && row.getAttribute("data-sc-cat") !== cat);
      });
    });
  });

  var SHA_TABLE = {
    setup: {
      name: "iPhone-Link-v4.0.0-Setup.exe",
      hash: "1AC11286D3BAF412540630075362E77C4B9B9CE0928EF06E11A01B547B1E332E"
    },
    zip: {
      name: "iPhone-Link-Win64-Native-Rust-v4.0.0.zip",
      hash: "15F9443CC10D28B456B40409C8C0B4D5FB1D055ECB052ECFB364AE4AF2875666"
    },
    exe: {
      name: "iPhone Link.exe",
      hash: "33BFCC2C4ED12B204445931510DF20267B9140A238E8ED61EF4436DD7DAC92BC"
    }
  };

  var shaLbl = document.getElementById("sha-label");
  var shaVal = document.getElementById("sha-value");
  var copyShaBtn = document.getElementById("btn-copy-sha");

  document.querySelectorAll("[data-sha-pick]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var k = btn.getAttribute("data-sha-pick");
      if (!SHA_TABLE[k]) return;
      setActiveInGroup("data-sha-pick", k);
      if (shaLbl) shaLbl.textContent = "SHA-256 (" + SHA_TABLE[k].name + "):";
      if (shaVal) shaVal.textContent = SHA_TABLE[k].hash;
    });
  });

  if (copyShaBtn && shaVal) {
    copyShaBtn.addEventListener("click", function () {
      var txt = shaVal.textContent;
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(txt).catch(function () {});
      }
      copyShaBtn.textContent = siteLang === "en" ? "Copied!" : "Tersalin!";
      setTimeout(function () {
        copyShaBtn.textContent = siteLang === "en" ? "Copy SHA-256" : "Salin SHA-256";
      }, 1500);
    });
  }

  var fbForm = document.getElementById("feedback-form");
  if (fbForm) {
    fbForm.addEventListener("submit", function (e) {
      e.preventDefault();
      var typ = (document.getElementById("fb-type") || {}).value || "Feedback";
      var dev = (document.getElementById("fb-device") || {}).value || "Windows 11 / iOS 27.2";
      var msg = (document.getElementById("fb-msg") || {}).value || "";
      var title = encodeURIComponent("[v4.0.0 " + typ + "] " + dev);
      var body = encodeURIComponent("**Category:** " + typ + "\n**Device / OS:** " + dev + "\n\n**Message:**\n" + msg);
      window.open("https://github.com/kelvindaniyar/iphone-link/issues/new?title=" + title + "&body=" + body, "_blank", "noopener");
    });
  }

  document.querySelectorAll(".faq-q").forEach(function (qBtn) {
    qBtn.addEventListener("click", function () {
      var item = qBtn.closest(".faq-item");
      if (!item) return;
      var willOpen = !item.classList.contains("open");
      item.classList.toggle("open", willOpen);
      qBtn.setAttribute("aria-expanded", willOpen ? "true" : "false");
      var ico = qBtn.querySelector(".faq-ico");
      if (ico) ico.textContent = willOpen ? "-" : "+";
    });
  });

  /* --------------------------------------------------------------------------
     8. INITIAL RENDER
     -------------------------------------------------------------------------- */
  var savedLang = "id";
  try {
    var l = localStorage.getItem("il_site_lang");
    if (l === "id" || l === "en") savedLang = l;
  } catch (e) {}
  applySiteLanguage(savedLang);
})();
