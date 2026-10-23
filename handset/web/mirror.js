const MIRROR = "/mirror";
const canvas = document.getElementById("c");
const ctx = canvas.getContext("2d");
const statusEl = document.getElementById("mirror-status");

let visualRotation = 0;
let zoom = 1;
const ZOOM_STEP = 1.2;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 3;

let fitMode = localStorage.getItem("pane-fit-mode") || "fill";
let currentCrop = { top: 0, bottom: 0, left: 0, right: 0 };
try {
  const savedCrop = JSON.parse(localStorage.getItem("pane-crop-edges") || "{}");
  if (savedCrop && typeof savedCrop === "object") {
    currentCrop = {
      top: Math.max(0, parseInt(savedCrop.top, 10) || 0),
      bottom: Math.max(0, parseInt(savedCrop.bottom, 10) || 0),
      left: Math.max(0, parseInt(savedCrop.left, 10) || 0),
      right: Math.max(0, parseInt(savedCrop.right, 10) || 0),
    };
  }
} catch (_) {}

// Sync initial theme from parent if accessible
try {
  const pTheme = window.parent?.currentTheme || window.parent?.document?.documentElement?.getAttribute("data-theme");
  if (pTheme) {
    document.documentElement.setAttribute("data-theme", pTheme);
    document.body?.setAttribute("data-theme", pTheme);
  }
} catch (_) {}

let pendingFrame = null;
let rafScheduled = false;
let lastFrame = null;
let lastFrameLandscape = null;
let frameCount = 0;
let runGeneration = 0;
let abortStream = null;

function setStatus(text, tone) {
  if (!statusEl) return;
  statusEl.textContent = text;
  statusEl.className = "mirror-status" + (tone ? ` ${tone}` : "");
}

function postDebugLog(event, data = {}) {
  try {
    fetch("/debug-metrics", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ source: "mirror.js", event, ...data }),
    }).catch(() => {});
  } catch (_) {}
}

postDebugLog("mirror_js_loaded", {
  innerWidth: window.innerWidth,
  innerHeight: window.innerHeight,
  dpr: window.devicePixelRatio,
});

function applyCropStyles(crop) {
  if (crop && typeof crop === "object") {
    currentCrop = {
      top: Math.max(0, parseInt(crop.top, 10) || 0),
      bottom: Math.max(0, parseInt(crop.bottom, 10) || 0),
      left: Math.max(0, parseInt(crop.left, 10) || 0),
      right: Math.max(0, parseInt(crop.right, 10) || 0),
    };
    try {
      localStorage.setItem("pane-crop-edges", JSON.stringify(currentCrop));
    } catch (_) {}
  }
  fitCanvas();
}

function fitCanvas() {
  if (!canvas.width || !canvas.height) return;
  const parent = canvas.parentElement || document.body;
  if (!parent) return;
  const rect = parent.getBoundingClientRect();
  const naturalW = canvas.width;
  const naturalH = canvas.height;
  const w = rect.width || window.innerWidth;
  const h = rect.height || window.innerHeight;
  if (!w || !h) return;

  const { top, bottom, left, right } = currentCrop;
  const hasCrop = top > 0 || bottom > 0 || left > 0 || right > 0;

  if (fitMode === "fill" && zoom === 1) {
    document.body.classList.add("fit-fill");
    document.body.classList.remove("fit-contain");
    if (hasCrop) {
      canvas.style.position = "absolute";
      canvas.style.top = `-${top}px`;
      canvas.style.left = `-${left}px`;
      canvas.style.width = `calc(100% + ${left + right}px)`;
      canvas.style.height = `calc(100% + ${top + bottom}px)`;
    } else {
      canvas.style.position = "";
      canvas.style.top = "";
      canvas.style.left = "";
      canvas.style.width = "100%";
      canvas.style.height = "100%";
    }
    return;
  }

  document.body.classList.remove("fit-fill");
  document.body.classList.add("fit-contain");
  const base = Math.min(w / naturalW, h / naturalH);
  const scale = base * zoom;
  const cw = Math.round(naturalW * scale);
  const ch = Math.round(naturalH * scale);
  if (hasCrop) {
    canvas.style.position = "absolute";
    canvas.style.top = `-${top}px`;
    canvas.style.left = `-${left}px`;
    canvas.style.width = `${cw + left + right}px`;
    canvas.style.height = `${ch + top + bottom}px`;
  } else {
    canvas.style.position = "";
    canvas.style.top = "";
    canvas.style.left = "";
    canvas.style.width = `${cw}px`;
    canvas.style.height = `${ch}px`;
  }
}

window.addEventListener("resize", fitCanvas);
if (window.ResizeObserver && document.body) {
  new ResizeObserver(() => fitCanvas()).observe(document.body);
}
window.addEventListener("message", (e) => {
  if (e.data && e.data.type === "resize") {
    fitCanvas();
  }
});
fitCanvas();

function setZoom(value) {
  zoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, value));
  if (Math.abs(zoom - 1) < 0.02) zoom = 1;
  fitCanvas();
}

function touchCoords(e) {
  const rect = canvas.getBoundingClientRect();
  const sx = ((e.clientX - rect.left) * canvas.width) / rect.width;
  const sy = ((e.clientY - rect.top) * canvas.height) / rect.height;
  const cw = canvas.width;
  const ch = canvas.height;
  const sideways = Math.abs(visualRotation % 180) === 90;
  const dw = sideways ? ch : cw;
  const dh = sideways ? cw : ch;
  const dx = sx - cw / 2;
  const dy = sy - ch / 2;
  const rad = (-visualRotation * Math.PI) / 180;
  const cos = Math.cos(rad);
  const sin = Math.sin(rad);
  const bx = dx * cos - dy * sin + dw / 2;
  const by = dx * sin + dy * cos + dh / 2;
  return {
    x: Math.max(0, Math.min(65535, Math.round((bx / dw) * 65535))),
    y: Math.max(0, Math.min(65535, Math.round((by / dh) * 65535))),
  };
}

async function postJson(path, payload) {
  try {
    await fetch(`${MIRROR}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
  } catch (_) {
    /* stream may be down */
  }
}

let activePointer = null;
let lastCoords = { x: 0, y: 0 };

// The proxy opens one connection per request, so concurrent fetches can land
// out of order and break drags. Send touches strictly in sequence, collapsing
// queued moves into the newest one.
const touchQueue = [];
let touchBusy = false;

function sendTouch(type, coords, isMove = false) {
  const last = touchQueue[touchQueue.length - 1];
  const event = { type, x: coords.x, y: coords.y, isMove };
  if (isMove && last && last.isMove) touchQueue[touchQueue.length - 1] = event;
  else touchQueue.push(event);
  pumpTouches();
}

async function pumpTouches() {
  if (touchBusy) return;
  touchBusy = true;
  while (touchQueue.length) {
    const { type, x, y } = touchQueue.shift();
    await postJson("/touch", { type, x, y });
  }
  touchBusy = false;
}

// iOS system gestures (Home bar, Control Center, Notification Center) only
// trigger when the contact begins on the screen edge itself.
const EDGE = 2500;
function snapToEdge(c) {
  return {
    x: c.x < EDGE ? 0 : c.x > 65535 - EDGE ? 65535 : c.x,
    y: c.y < EDGE ? 0 : c.y > 65535 - EDGE ? 65535 : c.y,
  };
}

// Injected touches arrive too slowly for iOS to read a bottom-edge flick as
// the Home gesture, so a drag that starts on the bottom edge is kept local and
// turned into the Home button (or a double press for the app switcher when
// the finger pauses before lifting).
const HOME_SWIPE_MIN = 5000;
const SWITCHER_HOLD_MS = 450;
let edgeSwipe = null;

async function pressHome(times = 1) {
  for (let i = 0; i < times; i++) {
    if (i) await new Promise((r) => setTimeout(r, 120));
    await postJson("/button", { name: "home", state: "press" });
  }
}

async function triggerBackGesture() {
  const yMid = 32768;
  const xStart = 0;
  const xEnd = 42000;
  const steps = 14;
  const dt = 16;
  await postJson("/touch", { type: "contact", x: xStart, y: yMid });
  for (let i = 1; i <= steps; i++) {
    const x = Math.round(xStart + (xEnd - xStart) * (i / steps));
    await new Promise((r) => setTimeout(r, dt));
    await postJson("/touch", { type: "contact", x, y: yMid });
  }
  await postJson("/touch", { type: "release", x: xEnd, y: yMid });
}

function releaseActive() {
  if (activePointer === null) return;
  activePointer = null;
  if (edgeSwipe) {
    const swipe = edgeSwipe;
    edgeSwipe = null;
    if (swipe.armed) {
      const paused = performance.now() - swipe.lastMoveAt > SWITCHER_HOLD_MS;
      pressHome(paused ? 2 : 1);
    } else {
      sendTouch("contact", swipe.start);
      sendTouch("release", swipe.start);
    }
    return;
  }
  sendTouch("release", lastCoords);
}

// Listen on the whole document so a drag can start on the black margin just
// below the picture; coordinates clamp onto the screen edge.
document.addEventListener("pointerdown", (e) => {
  if (e.button === 3) {
    e.preventDefault();
    triggerBackGesture();
    return;
  }
  if (e.button !== 0) return;
  canvas.focus({ preventScroll: true });
  try {
    document.body.setPointerCapture(e.pointerId);
  } catch (_) {}
  activePointer = e.pointerId;
  lastCoords = snapToEdge(touchCoords(e));
  if (lastCoords.y === 65535) {
    edgeSwipe = { start: lastCoords, armed: false, lastMoveAt: performance.now(), lastY: lastCoords.y };
    return;
  }
  sendTouch("contact", lastCoords);
});

window.addEventListener("auxclick", (e) => {
  if (e.button === 3) {
    e.preventDefault();
    triggerBackGesture();
  }
});

function onMove(e) {
  if (e.pointerId !== activePointer) return;
  e.preventDefault();
  lastCoords = touchCoords(e);
  if (edgeSwipe) {
    if (edgeSwipe.start.y - lastCoords.y > HOME_SWIPE_MIN) edgeSwipe.armed = true;
    if (Math.abs(lastCoords.y - edgeSwipe.lastY) > 600) {
      edgeSwipe.lastMoveAt = performance.now();
      edgeSwipe.lastY = lastCoords.y;
    }
    return;
  }
  sendTouch("contact", lastCoords, true);
}

function onUp(e) {
  if (e.pointerId !== activePointer) return;
  e.preventDefault();
  lastCoords = touchCoords(e);
  releaseActive();
}

window.addEventListener("pointermove", onMove);
window.addEventListener("pointerup", onUp);
window.addEventListener("pointercancel", onUp);
document.body.addEventListener("lostpointercapture", (e) => {
  if (e.pointerId === activePointer) releaseActive();
});
window.addEventListener("blur", releaseActive);
canvas.addEventListener("contextmenu", (e) => {
  e.preventDefault();
});

document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
});

// Precision Touchpad (2-finger scroll / swipe) -> Synthetic HID touch drag
let wheelBusy = false;
let wheelAccumY = 0;
let wheelAccumX = 0;
let wheelTimer = null;

async function performWheelSwipe(deltaX, deltaY, clientX, clientY) {
  if (wheelBusy) return;
  wheelBusy = true;
  try {
    let startPos = { x: 32768, y: 32768 };
    if (clientX != null && clientY != null) {
      try {
        startPos = touchCoords({ clientX, clientY });
      } catch (_) {}
    }

    startPos.x = Math.max(10000, Math.min(55535, startPos.x));
    startPos.y = Math.max(16000, Math.min(49000, startPos.y));

    const isVertical = Math.abs(deltaY) >= Math.abs(deltaX);
    let dx = 0;
    let dy = 0;

    if (isVertical) {
      const distance = Math.max(6000, Math.min(20000, Math.round(Math.abs(deltaY) * 80)));
      dy = deltaY > 0 ? -distance : distance;
    } else {
      const distance = Math.max(6000, Math.min(20000, Math.round(Math.abs(deltaX) * 80)));
      dx = deltaX > 0 ? -distance : distance;
    }

    const endX = Math.max(2500, Math.min(63000, startPos.x + dx));
    const endY = Math.max(2500, Math.min(63000, startPos.y + dy));

    const steps = 6;
    const dt = 12;

    await postJson("/touch", { type: "contact", x: startPos.x, y: startPos.y });
    for (let i = 1; i <= steps; i++) {
      const t = i / steps;
      const curX = Math.round(startPos.x + (endX - startPos.x) * t);
      const curY = Math.round(startPos.y + (endY - startPos.y) * t);
      await new Promise((r) => setTimeout(r, dt));
      await postJson("/touch", { type: "contact", x: curX, y: curY });
    }
    await postJson("/touch", { type: "release", x: endX, y: endY });
  } finally {
    wheelBusy = false;
  }
}

canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    wheelAccumX += e.deltaX;
    wheelAccumY += e.deltaY;

    const threshold = 35;
    if (Math.abs(wheelAccumY) >= threshold || Math.abs(wheelAccumX) >= threshold) {
      const dX = wheelAccumX;
      const dY = wheelAccumY;
      wheelAccumX = 0;
      wheelAccumY = 0;
      performWheelSwipe(dX, dY, e.clientX, e.clientY);
    }

    window.clearTimeout(wheelTimer);
    wheelTimer = window.setTimeout(() => {
      wheelAccumX = 0;
      wheelAccumY = 0;
    }, 200);
  },
  { passive: false }
);

window.addEventListener(
  "keydown",
  (e) => {
    if (e.altKey && (e.key === "ArrowLeft" || e.code === "ArrowLeft")) {
      e.preventDefault();
      e.stopPropagation();
      triggerBackGesture();
    }
  },
  true,
);

window.HandsetKeys.attach({
  post: (payload) => postJson("/key", payload),
  enabled: () => {
    try {
      return window.parent.handsetKeyboardOn !== false;
    } catch (_) {
      return true;
    }
  },
});

window.addEventListener("message", (e) => {
  const d = e.data;
  if (!d) return;

  if (d.type === "set-theme") {
    document.documentElement.setAttribute("data-theme", d.theme);
    document.body?.setAttribute("data-theme", d.theme);
    return;
  }
  if (d.type === "set-fit-mode") {
    fitMode = d.mode === "contain" ? "contain" : "fill";
    try { localStorage.setItem("pane-fit-mode", fitMode); } catch (_) {}
    fitCanvas();
    if (lastFrame) drawFrame(lastFrame);
    return;
  }
  if (d.type === "set-crop") {
    applyCropStyles(d.crop);
    return;
  }

  if (e.origin !== window.location.origin) return;
  if (d.source !== "handset") return;
  if (d.cmd === "back") {
    triggerBackGesture();
  } else if (d.cmd === "rotate") {
    visualRotation = (visualRotation + (d.deg || 0)) % 360;
    if (lastFrame) drawFrame(lastFrame);
    fitCanvas();
  } else if (d.cmd === "zoom-in") {
    setZoom(zoom * ZOOM_STEP);
  } else if (d.cmd === "zoom-out") {
    setZoom(zoom / ZOOM_STEP);
  } else if (d.cmd === "zoom-reset") {
    setZoom(1);
  } else if (d.cmd === "reload") {
    beginRun();
  }
});

async function fetchCodecWithRetry() {
  const delays = [200, 500, 1000, 2000, 3000, 4000];
  let lastErr = "";
  for (let attempt = 0; attempt <= delays.length; attempt++) {
    if (attempt > 0) await new Promise((r) => setTimeout(r, delays[attempt - 1]));
    try {
      const resp = await fetch(`${MIRROR}/codec`, { cache: "no-store" });
      if (!resp.ok) {
        lastErr = `HTTP ${resp.status}`;
        continue;
      }
      const info = await resp.json();
      if (!info.codec || !info.description) {
        lastErr = "waiting for video parameters";
        continue;
      }
      const description = Uint8Array.from(atob(info.description), (c) => c.charCodeAt(0));
      return { codec: info.codec.trim(), description };
    } catch (err) {
      lastErr = err.message || String(err);
    }
  }
  throw new Error(lastErr || "codec unreachable");
}

function sendMirrorMetrics(f, count) {
  try {
    let vr = null;
    if (f.visibleRect) {
      vr = {
        x: f.visibleRect.x,
        y: f.visibleRect.y,
        width: f.visibleRect.width,
        height: f.visibleRect.height,
      };
    }
    const cr = canvas.getBoundingClientRect();
    const br = document.body ? document.body.getBoundingClientRect() : null;
    const payload = {
      source: "mirror.js",
      frameCount: count,
      f_type: f.constructor ? f.constructor.name : typeof f,
      f_dims: {
        codedW: f.codedWidth,
        codedH: f.codedHeight,
        displayW: f.displayWidth,
        displayH: f.displayHeight,
        w: f.width,
        h: f.height,
        visibleRect: vr,
      },
      canvas_attr: {
        width: canvas.width,
        height: canvas.height,
        styleW: canvas.style.width,
        styleH: canvas.style.height,
      },
      canvas_rect: cr ? { top: cr.top, left: cr.left, width: cr.width, height: cr.height } : null,
      body_rect: br ? { top: br.top, left: br.left, width: br.width, height: br.height } : null,
      win_inner: {
        w: window.innerWidth,
        h: window.innerHeight,
        dpr: window.devicePixelRatio,
      },
    };
    fetch("/debug-metrics", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }).catch(() => {});
  } catch (err) {
    console.error("sendMirrorMetrics error:", err);
  }
}

function parseHevcSpsDimensions(description) {
  try {
    if (!description || description.length < 23) return null;
    const bytes = description instanceof Uint8Array ? description : new Uint8Array(description);
    let spsNal = null;

    if (bytes[0] === 1) {
      // ISO/IEC 14496-15 hvcC record
      const numOfArrays = bytes[22];
      let offset = 23;
      for (let i = 0; i < numOfArrays && offset + 3 <= bytes.length; i++) {
        const nalType = bytes[offset] & 0x3f;
        const numNalus = (bytes[offset + 1] << 8) | bytes[offset + 2];
        offset += 3;
        for (let j = 0; j < numNalus && offset + 2 <= bytes.length; j++) {
          const nalLen = (bytes[offset] << 8) | bytes[offset + 1];
          offset += 2;
          if (offset + nalLen <= bytes.length) {
            const nal = bytes.subarray(offset, offset + nalLen);
            offset += nalLen;
            if (nalType === 33) {
              spsNal = nal;
              break;
            }
          }
        }
        if (spsNal) break;
      }
    } else {
      spsNal = bytes;
    }

    if (!spsNal || spsNal.length < 15) return null;

    // Remove emulation prevention bytes (0x00 0x00 0x03 -> 0x00 0x00)
    const rb = [];
    let idx = 2; // skip 2-byte NAL header
    while (idx < spsNal.length) {
      if (idx + 2 < spsNal.length && spsNal[idx] === 0 && spsNal[idx + 1] === 0 && spsNal[idx + 2] === 3) {
        rb.push(spsNal[idx], spsNal[idx + 1]);
        idx += 3;
      } else {
        rb.push(spsNal[idx]);
        idx += 1;
      }
    }

    let pos = 0;
    const readBit = () => {
      const bit = (rb[pos >> 3] >> (7 - (pos & 7))) & 1;
      pos += 1;
      return bit;
    };
    const readBits = (n) => {
      let v = 0;
      for (let k = 0; k < n; k++) v = (v << 1) | readBit();
      return v;
    };
    const readUe = () => {
      let zeros = 0;
      while (readBit() === 0 && pos < rb.length * 8) zeros += 1;
      if (zeros === 0) return 0;
      return ((1 << zeros) | readBits(zeros)) - 1;
    };

    readBits(4); // sps_video_parameter_set_id
    const maxSubLayers = readBits(3);
    readBits(1); // sps_temporal_id_nesting_flag

    // profile_tier_level
    readBits(2 + 1 + 5 + 32 + 48 + 8);
    const subP = [];
    const subL = [];
    for (let s = 0; s < maxSubLayers; s++) {
      subP.push(readBit());
      subL.push(readBit());
    }
    if (maxSubLayers > 0) {
      for (let s = 0; s < 8 - maxSubLayers; s++) readBits(2);
    }
    for (let s = 0; s < maxSubLayers; s++) {
      if (subP[s]) readBits(2 + 1 + 5 + 32 + 48);
      if (subL[s]) readBits(8);
    }

    readUe(); // sps_seq_parameter_set_id
    const chroma = readUe();
    if (chroma === 3) readBit();
    const width = readUe();
    const height = readUe();
    if (width > 0 && height > 0) {
      return { width, height };
    }
  } catch (err) {
    console.warn("parseHevcSpsDimensions failed:", err);
  }
  return null;
}

function drawFrame(f) {
  let sourceFrame = f;
  let clonedFrame = null;

  // Un-crop VideoFrame jika hardware decoder meng-clamp visibleRect ke 720p
  if (f.codedWidth && f.codedHeight && f.visibleRect && f.visibleRect.height < f.codedHeight) {
    try {
      clonedFrame = new VideoFrame(f, {
        visibleRect: { x: 0, y: 0, width: f.codedWidth, height: f.codedHeight },
        displayWidth: f.codedWidth,
        displayHeight: f.codedHeight,
      });
      sourceFrame = clonedFrame;
    } catch (e) {
      console.warn("Un-crop VideoFrame failed:", e);
    }
  }

  try {
    const rawW = (sourceFrame.codedWidth && sourceFrame.codedHeight > sourceFrame.displayHeight)
      ? sourceFrame.codedWidth
      : (sourceFrame.displayWidth || sourceFrame.codedWidth);
    const rawH = (sourceFrame.codedHeight && sourceFrame.codedHeight > sourceFrame.displayHeight)
      ? sourceFrame.codedHeight
      : (sourceFrame.displayHeight || sourceFrame.codedHeight);

    const sideways = Math.abs(visualRotation % 180) === 90;
    const targetW = sideways ? rawH : rawW;
    const targetH = sideways ? rawW : rawH;

    const dimsChanged = targetW !== canvas.width || targetH !== canvas.height;
    if (dimsChanged) {
      canvas.width = targetW;
      canvas.height = targetH;
      fitCanvas();
    }

    const dstW = canvas.width;
    const dstH = canvas.height;
    const fpW = sideways ? dstH : dstW;
    const fpH = sideways ? dstW : dstH;

    ctx.save();
    ctx.translate(dstW / 2, dstH / 2);
    if (visualRotation) ctx.rotate((visualRotation * Math.PI) / 180);
    ctx.drawImage(sourceFrame, -fpW / 2, -fpH / 2, fpW, fpH);
    ctx.restore();

    if (frameCount === 1 || frameCount === 5 || frameCount === 15 || frameCount === 30 || frameCount % 60 === 0) {
      sendMirrorMetrics(sourceFrame, frameCount);
    }
  } finally {
    if (clonedFrame) {
      try {
        clonedFrame.close();
      } catch (_) {}
    }
  }
}

function drawPending() {
  rafScheduled = false;
  const f = pendingFrame;
  pendingFrame = null;
  if (!f) return;

  const rawW = (f.codedWidth && f.codedHeight > f.displayHeight) ? f.codedWidth : f.displayWidth;
  const rawH = (f.codedHeight && f.codedHeight > f.displayHeight) ? f.codedHeight : f.displayHeight;
  const landscape = rawW > rawH;

  if (lastFrameLandscape !== null && landscape !== lastFrameLandscape && visualRotation !== 0) {
    visualRotation = 0;
  }
  lastFrameLandscape = landscape;
  try {
    drawFrame(f);
    if (frameCount === 1) {
      setStatus("Live", "ok");
      fitCanvas();
      setTimeout(fitCanvas, 50);
      setTimeout(fitCanvas, 200);
    }
  } catch (err) {
    console.error("drawFrame error:", err);
  }
  if (lastFrame && lastFrame !== f) {
    try {
      lastFrame.close();
    } catch (_) {}
  }
  lastFrame = f;
}

async function run(generation) {
  setStatus("Connecting to stream…");
  frameCount = 0;
  const startedAt = performance.now();
  const watchdog = window.setInterval(() => {
    if (generation !== runGeneration) {
      window.clearInterval(watchdog);
      return;
    }
    if (frameCount > 0) {
      window.clearInterval(watchdog);
      return;
    }
    if (performance.now() - startedAt > 12000) {
      window.clearInterval(watchdog);
      setStatus("No picture yet — press Restart screen on the right", "warn");
    }
  }, 1000);

  const { codec, description } = await fetchCodecWithRetry();
  if (generation !== runGeneration) return;

  const spsDims = parseHevcSpsDimensions(description);
  const streamWidth = spsDims?.width || 1216;
  const streamHeight = spsDims?.height || 2656;

  const decoderConfig = {
    codec,
    description,
    codedWidth: streamWidth,
    codedHeight: streamHeight,
    displayAspectWidth: streamWidth,
    displayAspectHeight: streamHeight,
    optimizeForLatency: true,
  };
  const support = await VideoDecoder.isConfigSupported({ codec, description });
  postDebugLog("codec_check", {
    codec,
    supported: support.supported,
    descriptionLength: description ? (description.byteLength || description.length) : 0,
    parsedDimensions: spsDims,
    configuredDimensions: { width: streamWidth, height: streamHeight },
  });
  if (!support.supported) {
    setStatus("This browser cannot decode the iPhone video stream.", "warn");
    return;
  }

  let needsResync = false;
  let autoRestartUsed = false;

  const buildDecoder = () =>
    new VideoDecoder({
      output: (frame) => {
        if (frameCount === 0) {
          postDebugLog("first_frame_output", {
            codedW: frame.codedWidth,
            codedH: frame.codedHeight,
            displayW: frame.displayWidth,
            displayH: frame.displayHeight,
          });
        }
        if (pendingFrame) {
          try {
            pendingFrame.close();
          } catch (_) {}
        }
        pendingFrame = frame;
        if (!rafScheduled) {
          rafScheduled = true;
          requestAnimationFrame(drawPending);
        }
        frameCount += 1;
      },
      error: (err) => {
        postDebugLog("decoder_error", {
          message: err ? (err.message || String(err)) : "VideoDecoder reported error",
        });
        needsResync = true;
        if (!autoRestartUsed && frameCount < 5) {
          autoRestartUsed = true;
          fetch(`${MIRROR}/restart`, { method: "POST", cache: "no-store" }).catch(() => {});
        } else {
          fetch(`${MIRROR}/pli`, { method: "POST", cache: "no-store" }).catch(() => {});
        }
      },
    });

  let decoder = buildDecoder();
  decoder.configure(decoderConfig);
  postDebugLog("decoder_configured", { codec, state: decoder.state });
  setStatus("Waiting for video…");

  let resp = null;
  const streamDelays = [300, 600, 1200, 2000, 3000, 5000, 8000];
  for (let a = 0; a <= streamDelays.length; a++) {
    if (generation !== runGeneration) return;
    if (a > 0) {
      setStatus(`Waiting for video stream (retry ${a})…`);
      await new Promise((r) => setTimeout(r, streamDelays[a - 1]));
      if (a === 2) {
        await fetch(`${MIRROR}/pli`, { method: "POST", cache: "no-store" }).catch(() => {});
      }
    }
    try {
      const r = await fetch(`${MIRROR}/live`, { cache: "no-store" });
      if (r.ok) {
        resp = r;
        break;
      }
      if (r.status === 503) {
        setStatus("Stream starting on the iPhone…");
        continue;
      }
      setStatus(`Stream error (HTTP ${r.status})`, "warn");
    } catch (err) {
      setStatus(err.message || "Stream unreachable", "warn");
    }
  }
  if (!resp || generation !== runGeneration) {
    setStatus("Cannot reach the video stream. Press Restart screen.", "warn");
    return;
  }

  const reader = resp.body.getReader();
  abortStream = () => reader.cancel().catch(() => {});
  let buf = new Uint8Array(0);
  let timestamp = 0;
  let gotKey = false;

  while (generation === runGeneration) {
    const { value, done } = await reader.read();
    if (done) break;
    const merged = new Uint8Array(buf.length + value.length);
    merged.set(buf);
    merged.set(value, buf.length);
    buf = merged;
    while (buf.length >= 4) {
      const len = (buf[0] << 24) | (buf[1] << 16) | (buf[2] << 8) | buf[3];
      if (buf.length < 4 + len) break;
      const type = buf[4];
      const data = buf.slice(5, 4 + len);
      buf = buf.subarray(4 + len);

      if (type === 2) {
        try {
          decoder.close();
        } catch (_) {}
        decoder = buildDecoder();
        decoder.configure(decoderConfig);
        needsResync = false;
        gotKey = true;
      } else if (type === 0) {
        gotKey = true;
        if (needsResync) {
          try {
            decoder.close();
          } catch (_) {}
          decoder = buildDecoder();
          decoder.configure(decoderConfig);
          needsResync = false;
        }
      }
      if (!gotKey || needsResync) continue;
      if (decoder.state !== "configured") {
        try {
          decoder.close();
        } catch (_) {}
        decoder = buildDecoder();
        decoder.configure(decoderConfig);
        needsResync = true;
        continue;
      }
      try {
        decoder.decode(
          new EncodedVideoChunk({
            type: type === 0 || type === 2 ? "key" : "delta",
            timestamp,
            data,
          }),
        );
        timestamp += 16666;
      } catch (_) {
        needsResync = true;
      }
    }
  }
  if (generation === runGeneration) {
    setStatus("Stream ended — press Restart screen", "warn");
  }
}

function beginRun() {
  runGeneration += 1;
  const generation = runGeneration;
  if (abortStream) {
    abortStream();
    abortStream = null;
  }
  if (pendingFrame) {
    try {
      pendingFrame.close();
    } catch (_) {}
    pendingFrame = null;
  }
  if (lastFrame) {
    try {
      lastFrame.close();
    } catch (_) {}
    lastFrame = null;
  }
  frameCount = 0;
  run(generation).catch((err) => {
    setStatus(err.message || "Mirror failed", "warn");
  });
}

beginRun();
