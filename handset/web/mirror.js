// Tahap 66: Sticky Hysteresis Latch Anti-Ping-Pong & Landscape Game Detection
// Tahap 77: Precision Landscape Lock on Messi & Yamal and Main Menu eFootball 2027
// Tahap 78: Instant Home Screen & Folder Portrait Return
let autoRotateGameEnabled = false;
try {
  autoRotateGameEnabled = localStorage.getItem("pane-auto-rotate") === "true";
} catch (_) {}

let _lastAutoRotateSampleTs = 0;
let _landscapeVoteCount = 0;
let _portraitVoteCount = 0;
let _autoRotatedToLandscape = false;
let manualRotateCooldownUntil = 0;
let _lastHomeOrRecentNavTs = 0;
let _homeExitGraceUntil = 0;

function analyzeFrameOrientation(sourceFrame, optRotation) {
  if (!sourceFrame) return null;
  // Hardware decoded dimensions already landscape
  if (sourceFrame.codedWidth && sourceFrame.codedHeight && sourceFrame.codedWidth > sourceFrame.codedHeight) {
    return {
      isFullscreenGame: true,
      isLandscapeGameFrame: true,
      hasTruePortraitStatusBar: false,
      hasPortraitStatusBar: false,
      hasPortraitBottomBar: false,
      hasLandscapeSideBar: true,
      hasStrictThinEdgeHomeBar: true,
      hasNakedDynamicIsland: false,
      isPortraitOverlayOrFolder: false,
      isYouTubeFullscreenLandscape: false,
      hasLightPortraitAppChrome: false,
      hasTikTokCenterPlusButton: false,
      hasTikTokOrPortraitBottomBar: false,
      hasInstagramReelsDarkChrome: false,
      hasInstagramDarkModeChrome: false,
      hasPlainWhiteClockStatusBar: false,
      hasIosHomeScreenDock: false,
      exitByInstagramDarkMode: false,
      hasDarkPortraitAppChrome: false,
      isAppSplashScreen: false,
      hasDimmedDarkHomeBar: false,
      isGoogleTranslateDarkLayout: false,
      hasYouTubeDarkLowerWhiteCard: false,
      isPortraitCommentSheetScreen: false,
      isDefinitePortraitAppScreen: false,
      canExitLandscapeToPortrait: false,
      overallAvgLuma: 100,
      hasSymmetricDock: false
    };
  }
  if (sourceFrame.displayWidth && sourceFrame.displayHeight && sourceFrame.displayWidth > sourceFrame.displayHeight) {
    return {
      isFullscreenGame: true,
      isLandscapeGameFrame: true,
      hasTruePortraitStatusBar: false,
      hasPortraitStatusBar: false,
      hasPortraitBottomBar: false,
      hasLandscapeSideBar: true,
      hasStrictThinEdgeHomeBar: true,
      hasNakedDynamicIsland: false,
      isPortraitOverlayOrFolder: false,
      isYouTubeFullscreenLandscape: false,
      hasLightPortraitAppChrome: false,
      hasTikTokCenterPlusButton: false,
      hasTikTokOrPortraitBottomBar: false,
      hasInstagramReelsDarkChrome: false,
      hasInstagramDarkModeChrome: false,
      hasPlainWhiteClockStatusBar: false,
      hasIosHomeScreenDock: false,
      exitByInstagramDarkMode: false,
      hasDarkPortraitAppChrome: false,
      isAppSplashScreen: false,
      hasDimmedDarkHomeBar: false,
      isGoogleTranslateDarkLayout: false,
      hasYouTubeDarkLowerWhiteCard: false,
      isPortraitCommentSheetScreen: false,
      isDefinitePortraitAppScreen: false,
      canExitLandscapeToPortrait: false,
      overallAvgLuma: 100,
      hasSymmetricDock: false
    };
  }

  if (!window._probeCanvas) {
    window._probeCanvas = document.createElement("canvas");
    window._probeCanvas.width = 120;
    window._probeCanvas.height = 260;
  }
  const pCtx = window._probeCanvas.getContext("2d", { willReadFrequently: true });
  pCtx.drawImage(sourceFrame, 0, 0, 120, 260);

  const imgData = pCtx.getImageData(0, 0, 120, 260).data;
  // Tahap 94: Zero-heap allocation reusable ring-buffer for getPixel
  const _pxPool = [
    { r: 0, g: 0, b: 0, luma: 0 },
    { r: 0, g: 0, b: 0, luma: 0 },
    { r: 0, g: 0, b: 0, luma: 0 },
    { r: 0, g: 0, b: 0, luma: 0 },
    { r: 0, g: 0, b: 0, luma: 0 },
    { r: 0, g: 0, b: 0, luma: 0 },
    { r: 0, g: 0, b: 0, luma: 0 },
    { r: 0, g: 0, b: 0, luma: 0 }
  ];
  let _pxIdx = 0;

  const getPixel = (x, y) => {
    const p = _pxPool[_pxIdx];
    _pxIdx = (_pxIdx + 1) & 7;
    if (x < 0 || x >= 120 || y < 0 || y >= 260) {
      p.r = 0; p.g = 0; p.b = 0; p.luma = 0;
      return p;
    }
    const idx = (y * 120 + x) * 4;
    const r = imgData[idx];
    const g = imgData[idx + 1];
    const b = imgData[idx + 2];
    p.r = r;
    p.g = g;
    p.b = b;
    p.luma = (r * 299 + g * 587 + b * 114) / 1000;
    return p;
  };

  const getLuma = (x, y) => {
    if (x < 0 || x >= 120 || y < 0 || y >= 260) return 0;
    const idx = (y * 120 + x) * 4;
    return (imgData[idx] * 299 + imgData[idx + 1] * 587 + imgData[idx + 2] * 114) / 1000;
  };

  // Tahap 89 & 90: Landscape playback status
  const isCurrentlyInLandscape = (typeof optRotation !== "undefined") ?
    Boolean(optRotation === 90 || optRotation === 270) :
    Boolean(
      (typeof window !== "undefined" && (window.currentRotation === 90 || window.currentRotation === 270 || window.__mockRotation === 90 || window.__mockRotation === 270)) ||
      (typeof currentRotation !== "undefined" && (currentRotation === 90 || currentRotation === 270)) ||
      (typeof _autoRotatedToLandscape !== "undefined" && _autoRotatedToLandscape === true)
    );

  // Tahap 109 Pilar 1: iOS Home Screen Bottom Dock Detector (y=218..246, x=6..114)
  // Check eFootball pitch green early to prevent false positives in football matches
  let pitchGreenCount = 0;
  for (let y = 45; y <= 195; y += 4) {
    for (let x = 22; x <= 96; x += 4) {
      const p = getPixel(x, y);
      if (p.g >= 30 && p.g <= 155 && (p.g - p.r) >= 10 && (p.g - p.b) >= 12) {
        pitchGreenCount++;
      }
    }
  }

  // Sample 3 Gaps in bottom dock capsule: Gap 1 (x=30..34), Gap 2 (x=56..61), Gap 3 (x=83..87) on y=224..242
  let gap1Luma = 0, gap1Sat = 0, gap1Cnt = 0;
  for (let y = 224; y <= 242; y++) {
    for (let x = 30; x <= 34; x++) {
      const p = getPixel(x, y);
      gap1Luma += p.luma;
      gap1Sat += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      gap1Cnt++;
    }
  }
  const gap1AvgLuma = gap1Cnt > 0 ? (gap1Luma / gap1Cnt) : 100;
  const gap1AvgSat = gap1Cnt > 0 ? (gap1Sat / gap1Cnt) : 100;

  let gap2Luma = 0, gap2Sat = 0, gap2Cnt = 0;
  for (let y = 224; y <= 242; y++) {
    for (let x = 56; x <= 61; x++) {
      const p = getPixel(x, y);
      gap2Luma += p.luma;
      gap2Sat += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      gap2Cnt++;
    }
  }
  const gap2AvgLuma = gap2Cnt > 0 ? (gap2Luma / gap2Cnt) : 100;
  const gap2AvgSat = gap2Cnt > 0 ? (gap2Sat / gap2Cnt) : 100;

  let gap3Luma = 0, gap3Sat = 0, gap3Cnt = 0;
  for (let y = 224; y <= 242; y++) {
    for (let x = 83; x <= 87; x++) {
      const p = getPixel(x, y);
      gap3Luma += p.luma;
      gap3Sat += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      gap3Cnt++;
    }
  }
  const gap3AvgLuma = gap3Cnt > 0 ? (gap3Luma / gap3Cnt) : 100;
  const gap3AvgSat = gap3Cnt > 0 ? (gap3Sat / gap3Cnt) : 100;

  let dockGapQuietCount = 0;
  if (gap1AvgLuma <= 85 && gap1AvgSat <= 38) dockGapQuietCount++;
  if (gap2AvgLuma <= 85 && gap2AvgSat <= 38) dockGapQuietCount++;
  if (gap3AvgLuma <= 85 && gap3AvgSat <= 38) dockGapQuietCount++;
  const allGapsAvgLuma = (gap1AvgLuma + gap2AvgLuma + gap3AvgLuma) / 3;
  const allGapsAvgSat = (gap1AvgSat + gap2AvgSat + gap3AvgSat) / 3;

  // Sample 4 Dock Slots: Slot 1 (x=11..27), Slot 2 (x=37..53), Slot 3 (x=64..80), Slot 4 (x=90..106) on y=222..243
  let dockGreenSlots = 0;
  let dockSafariSlot2 = false;
  let dockActiveSlots = 0;

  const dockSlotsCoords = [
    { x1: 11, x2: 27 },
    { x1: 37, x2: 53 },
    { x1: 64, x2: 80 },
    { x1: 90, x2: 106 }
  ];

  for (let sIdx = 0; sIdx < dockSlotsCoords.length; sIdx++) {
    const coords = dockSlotsCoords[sIdx];
    let sLuma = 0, sSat = 0, sCnt = 0;
    let sGreenCount = 0;
    let sBlueWhiteCount = 0;
    for (let y = 222; y <= 243; y++) {
      for (let x = coords.x1; x <= coords.x2; x++) {
        const p = getPixel(x, y);
        sLuma += p.luma;
        const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
        sSat += sat;
        sCnt++;
        if (p.g >= 140 && (p.g - p.r) >= 38 && (p.g - p.b) >= 25) {
          sGreenCount++;
        }
        if ((p.b >= 140 && (p.b - p.r) >= 32) || (p.luma >= 180 && sat <= 35)) {
          sBlueWhiteCount++;
        }
      }
    }
    const slotAvgL = sCnt > 0 ? (sLuma / sCnt) : 0;
    const slotAvgS = sCnt > 0 ? (sSat / sCnt) : 0;
    if ((sIdx === 0 || sIdx === 2 || sIdx === 3) && sGreenCount >= 6) {
      dockGreenSlots++;
    }
    if (sIdx === 1 && sBlueWhiteCount >= 5) {
      dockSafariSlot2 = true;
    }
    if ((slotAvgL - allGapsAvgLuma >= 18) || (slotAvgS - allGapsAvgSat >= 18)) {
      dockActiveSlots++;
    }
  }

  const hasIosHomeScreenDock = Boolean(
    pitchGreenCount < 65 &&
    ((dockGreenSlots >= 2 && dockSafariSlot2 && dockGapQuietCount >= 2) ||
     (dockGreenSlots >= 2 && dockActiveSlots >= 3 && dockGapQuietCount >= 2))
  );

  // Preliminary left edge check to ensure splash screen & portrait layouts don't conflict with landscape game side bar
  // Tahap 108 & 109 Pilar 3: Stringent Home Indicator check with top/bot cap isolation & thin width
  let innerColLumaSum98_162 = 0, innerColLumaCnt98_162 = 0;
  for (let y = 98; y <= 162; y += 2) {
    for (let x = 8; x <= 13; x++) {
      innerColLumaSum98_162 += getLuma(x, y);
      innerColLumaCnt98_162++;
    }
  }
  const innerColAvgLuma98_162 = innerColLumaCnt98_162 > 0 ? (innerColLumaSum98_162 / innerColLumaCnt98_162) : 0;

  let hasPreliminarySideBar = false;
  if (!hasIosHomeScreenDock) {
    let topCapEdgeSum = 0, topCapEdgeCnt = 0;
    for (let y = 62; y <= 80; y += 2) {
      for (let x = 1; x <= 3; x++) {
        topCapEdgeSum += getLuma(x, y);
        topCapEdgeCnt++;
      }
    }
    const topCapEdgeLuma = topCapEdgeCnt > 0 ? (topCapEdgeSum / topCapEdgeCnt) : 0;

    let botCapEdgeSum = 0, botCapEdgeCnt = 0;
    for (let y = 180; y <= 198; y += 2) {
      for (let x = 1; x <= 3; x++) {
        botCapEdgeSum += getLuma(x, y);
        botCapEdgeCnt++;
      }
    }
    const botCapEdgeLuma = botCapEdgeCnt > 0 ? (botCapEdgeSum / botCapEdgeCnt) : 0;

    let nearInnerColSum = 0, nearInnerColCnt = 0;
    for (let y = 105; y <= 155; y += 2) {
      for (let x = 5; x <= 7; x++) {
        nearInnerColSum += getLuma(x, y);
        nearInnerColCnt++;
      }
    }
    const nearInnerColLuma = nearInnerColCnt > 0 ? (nearInnerColSum / nearInnerColCnt) : 0;

    for (let xEdge = 1; xEdge <= 3; xEdge++) {
      let midSum = 0, midCnt = 0;
      for (let y = 105; y <= 155; y++) {
        midSum += getLuma(xEdge, y);
        midCnt++;
      }
      const midBarEdgeLuma = midCnt > 0 ? (midSum / midCnt) : 0;
      if (
        midBarEdgeLuma >= 118 &&
        (midBarEdgeLuma - topCapEdgeLuma) >= 24 &&
        (midBarEdgeLuma - botCapEdgeLuma) >= 24 &&
        (midBarEdgeLuma - nearInnerColLuma) >= 22 &&
        (midBarEdgeLuma - innerColAvgLuma98_162) >= 32
      ) {
        hasPreliminarySideBar = true;
        break;
      }
    }
  }

  // 1. Overall average luma (detect dark loading screens)
  let totalLuma = 0;
  let sampleCount = 0;
  for (let y = 15; y < 245; y += 8) {
    for (let x = 12; x < 108; x += 8) {
      totalLuma += getLuma(x, y);
      sampleCount++;
    }
  }
  const overallAvgLuma = sampleCount > 0 ? (totalLuma / sampleCount) : 0;

  // Center video area (y: 25..235, x: 18..102) for YouTube fullscreen detection
  let videoLumaSum = 0, videoSampleCount = 0;
  for (let y = 35; y <= 225; y += 10) {
    for (let x = 20; x <= 100; x += 10) {
      videoLumaSum += getLuma(x, y);
      videoSampleCount++;
    }
  }
  const centerVideoAvgLuma = videoSampleCount > 0 ? (videoLumaSum / videoSampleCount) : 0;

  // 2. Dynamic Island pill at top center (x: 46..74, y: 4..9)
  let islandLumaSum = 0;
  let islandPixelCount = 0;
  let islandMaxLuma = 0;
  for (let y = 4; y <= 9; y++) {
    for (let x = 46; x <= 74; x++) {
      const l = getLuma(x, y);
      islandLumaSum += l;
      islandPixelCount++;
      if (l > islandMaxLuma) islandMaxLuma = l;
    }
  }
  const islandAvgLuma = islandPixelCount > 0 ? (islandLumaSum / islandPixelCount) : 100;

  // Flanks around Dynamic Island (left: x 14..38, right: x 82..106, y 4..10)
  let leftFlankSum = 0, leftFlankCount = 0;
  for (let y = 4; y <= 10; y++) {
    for (let x = 14; x <= 38; x++) {
      leftFlankSum += getLuma(x, y);
      leftFlankCount++;
    }
  }
  const leftFlankAvgLuma = leftFlankCount > 0 ? (leftFlankSum / leftFlankCount) : 0;

  let rightFlankSum = 0, rightFlankCount = 0;
  for (let y = 4; y <= 10; y++) {
    for (let x = 82; x <= 106; x++) {
      rightFlankSum += getLuma(x, y);
      rightFlankCount++;
    }
  }
  const rightFlankAvgLuma = rightFlankCount > 0 ? (rightFlankSum / rightFlankCount) : 0;

  const minFlankLuma = Math.min(leftFlankAvgLuma, rightFlankAvgLuma);
  const maxFlankLuma = Math.max(leftFlankAvgLuma, rightFlankAvgLuma);

  // Tahap 80 Pilar 1 & Tahap 93 Pilar 1: Flank Saturation & Vivid Graphics Check
  let flankSatSum = 0;
  let flankPixelCnt = 0;
  let leftFlankSatSum = 0, leftFlankSatCnt = 0;
  let rightFlankSatSum = 0, rightFlankSatCnt = 0;
  let vividFlankPixelCount = 0;

  for (let y = 3; y <= 12; y++) {
    for (let x = 12; x <= 40; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      flankSatSum += sat;
      flankPixelCnt++;
      leftFlankSatSum += sat;
      leftFlankSatCnt++;
      if (sat >= 55 && p.luma >= 40) {
        vividFlankPixelCount++;
      }
    }
    for (let x = 80; x <= 108; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      flankSatSum += sat;
      flankPixelCnt++;
      rightFlankSatSum += sat;
      rightFlankSatCnt++;
      if (sat >= 55 && p.luma >= 40) {
        vividFlankPixelCount++;
      }
    }
  }
  const avgFlankSat = flankPixelCnt > 0 ? (flankSatSum / flankPixelCnt) : 0;
  const leftFlankAvgSat = leftFlankSatCnt > 0 ? (leftFlankSatSum / leftFlankSatCnt) : 0;
  const rightFlankAvgSat = rightFlankSatCnt > 0 ? (rightFlankSatSum / rightFlankSatCnt) : 0;

  // Tahap 93 Pilar 1: Top Flank Electric Cyan/Blue/Magenta Neon Pixels (x=6..42 & x=78..114, y=2..12)
  let topFlankElectricBlueMagentaPixels = 0;
  for (let y = 2; y <= 12; y++) {
    for (let x = 6; x <= 42; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 60 && p.b >= 110 && ((p.b - p.g) >= 12 || p.r >= 120)) {
        topFlankElectricBlueMagentaPixels++;
      }
    }
    for (let x = 78; x <= 114; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 60 && p.b >= 110 && ((p.b - p.g) >= 12 || p.r >= 120)) {
        topFlankElectricBlueMagentaPixels++;
      }
    }
  }

  // Blue "< Kembali" / Back button detector for eFootball & sub-menus (sampled on y=11..26 below status bar)
  let blueBackBtnCount = 0;
  for (let y = 11; y <= 26; y += 2) {
    for (let x = 3; x <= 28; x += 2) {
      const p = getPixel(x, y);
      if (p.b >= 140 && p.g >= 90 && p.r <= 80 && (p.b - p.r) >= 70) {
        blueBackBtnCount++;
      }
    }
  }

  // Background luma for left header (sampled at y=1..2 and y=11..12 across x=4..35 for Notch & Dynamic Island)
  let bgLumaLeftSum = 0, bgLeftCount = 0;
  for (const y of [1, 2, 11, 12]) {
    for (let x = 4; x <= 35; x++) {
      bgLumaLeftSum += getLuma(x, y);
      bgLeftCount++;
    }
  }
  const bgLumaLeft = bgLeftCount > 0 ? (bgLumaLeftSum / bgLeftCount) : 0;

  // Background luma for right header (sampled at y=1..2 and y=11..12 across x=80..116 for Notch & Dynamic Island)
  let bgLumaRightSum = 0, bgRightCount = 0;
  for (const y of [1, 2, 11, 12]) {
    for (let x = 80; x <= 116; x++) {
      bgLumaRightSum += getLuma(x, y);
      bgRightCount++;
    }
  }
  const bgLumaRight = bgRightCount > 0 ? (bgLumaRightSum / bgRightCount) : 0;

  // 3. Status Bar Flanks & Chromatic Analysis (Tahap 84: Extended x=4..35 left clock & x=80..116 right status for Notch & Island)
  let edgeCountLeft = 0;
  let minLumaLeft = 255, maxLumaLeft = 0;
  let leftClockMinLuma = 255, leftClockMaxLuma = 0;
  let achromaticClockGlyphCount = 0;
  let neonLeftCount = 0;

  for (let y = 2; y <= 10; y++) {
    for (let x = 4; x <= 35; x++) {
      const p1 = getPixel(x, y);
      const p2 = getPixel(x + 1, y);
      if (p1.luma < minLumaLeft) minLumaLeft = p1.luma;
      if (p1.luma > maxLumaLeft) maxLumaLeft = p1.luma;
      if (p1.luma < leftClockMinLuma) leftClockMinLuma = p1.luma;
      if (p1.luma > leftClockMaxLuma) leftClockMaxLuma = p1.luma;
      if (Math.abs(p1.luma - p2.luma) > 40) edgeCountLeft++;

      const sat = Math.max(p1.r, p1.g, p1.b) - Math.min(p1.r, p1.g, p1.b);
      // Check for bright neon pixels on left (luma > 95 and sat > 65)
      if (p1.luma > 95 && sat > 65) {
        neonLeftCount++;
      }

      // Tahap 84 Pilar 1: Clock rows y=2..9 on x=4..35 with dark background tolerance (iPhone 11 Pro Max Notch & iPhone 17 Pro Island)
      if (y >= 2 && y <= 9 && x >= 4 && x <= 35) {
        let isGlyph = false;
        if (bgLumaLeft < 65) {
          // Instagram Reels / dark video / Google Translate dark / iPhone 11 Pro Max clock "20.52"
          isGlyph = (p1.luma >= Math.max(48, bgLumaLeft + 16) && sat <= 34);
        } else if (bgLumaLeft < 145) {
          // Dark / medium background: white / light-gray clock glyphs (TikTok 02:11, Home Screen, Dark Wallpaper)
          isGlyph = (p1.luma >= Math.max(88, bgLumaLeft + 35) && sat <= 34);
        } else {
          // Bright / white background: dark / gray clock glyphs (Threads 01:28, Instagram 01:40)
          isGlyph = (p1.luma <= Math.min(160, bgLumaLeft - 30) && sat <= 34);
          if (Math.abs(bgLumaLeft - bgLumaRight) > 35) {
            isGlyph = false;
          }
        }
        if (isGlyph) {
          achromaticClockGlyphCount++;
        }
      }
    }
  }

  // Top white chrome pixels counter (Instagram / Threads header)
  let topWhitePixelCount = 0;
  for (let y = 1; y <= 11; y++) {
    for (let x = 8; x <= 40; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 170 && sat <= 32) topWhitePixelCount++;
    }
    for (let x = 80; x <= 112; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 170 && sat <= 32) topWhitePixelCount++;
    }
  }

  // Top header max luma for TikTok / video player detection
  let topHeaderMaxLuma = 0;
  for (let y = 4; y <= 14; y += 2) {
    for (let x = 10; x <= 110; x += 4) {
      const l = getLuma(x, y);
      if (l > topHeaderMaxLuma) topHeaderMaxLuma = l;
    }
  }

  let edgeCountRight = 0;
  let minLumaRight = 255, maxLumaRight = 0;
  let rightStatusMinLuma = 255, rightStatusMaxLuma = 0;
  let rightStatusSatSum = 0, rightStatusPixelCnt = 0;
  for (let y = 2; y <= 10; y++) {
    for (let x = 80; x <= 116; x++) {
      const p1 = getPixel(x, y);
      const p2 = getPixel(x + 1, y);
      if (p1.luma < minLumaRight) minLumaRight = p1.luma;
      if (p1.luma > maxLumaRight) maxLumaRight = p1.luma;
      if (p1.luma < rightStatusMinLuma) rightStatusMinLuma = p1.luma;
      if (p1.luma > rightStatusMaxLuma) rightStatusMaxLuma = p1.luma;
      const sat = Math.max(p1.r, p1.g, p1.b) - Math.min(p1.r, p1.g, p1.b);
      rightStatusSatSum += sat;
      rightStatusPixelCnt++;
      if (Math.abs(p1.luma - p2.luma) > 40) edgeCountRight++;
    }
  }
  const rightStatusAvgSat = rightStatusPixelCnt > 0 ? (rightStatusSatSum / rightStatusPixelCnt) : 0;

  // Check for bright neon pixels directly touching/surrounding Dynamic Island (x: 40..80, y: 2..11)
  let neonNearIslandCount = 0;
  for (let y = 2; y <= 11; y++) {
    for (let x = 40; x <= 80; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma > 95 && sat > 70) {
        neonNearIslandCount++;
      }
    }
  }

  // Check bright colorful pixels in left zone x=8..42, y=11..24 (below status bar y=2..10)
  let colorfulLeftZoneCount = 0;
  for (let y = 11; y <= 24; y++) {
    for (let x = 8; x <= 42; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma > 75 && sat > 45) {
        colorfulLeftZoneCount++;
      }
    }
  }

  // Tahap 88 Pilar 1 & Tahap 92 Pilar 1: iOS Notch Colored Clock Pill (Blue, Green, Red) on iPhone 11 Pro Max (x=4..30, y=2..10)
  let clockPillColorPixels = 0;
  let clockPillWhiteDigitPixels = 0;
  for (let y = 2; y <= 10; y++) {
    for (let x = 4; x <= 30; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 175 && sat <= 35) {
        clockPillWhiteDigitPixels++;
      }
      const isColoredPill = Boolean(
        (p.b >= 135 && (p.b - p.r) >= 55) ||
        (p.g >= 135 && (p.g - p.r) >= 45) ||
        (p.r >= 155 && (p.r - p.b) >= 65)
      );
      if (isColoredPill) clockPillColorPixels++;
    }
  }

  // Pixels below clock pill on x=4..28, y=12..25
  let belowClockBluePixels = 0;
  for (let y = 12; y <= 25; y++) {
    for (let x = 4; x <= 28; x++) {
      const p = getPixel(x, y);
      if (p.b >= 135 && p.g >= 80 && (p.b - p.r) >= 60) {
        belowClockBluePixels++;
      }
    }
  }

  let hasNotchColoredClockPillStatusBar = Boolean(
    clockPillColorPixels >= 4 &&
    (leftClockMaxLuma - leftClockMinLuma) >= 35 &&
    belowClockBluePixels <= 2 &&
    (rightStatusMaxLuma - rightStatusMinLuma) >= 18 &&
    edgeCountRight >= 2 &&
    bgLumaRight <= 60
  );

  // Tahap 84 Pilar 1: True iOS Portrait Status Bar (iPhone 11 Pro Max Notch & iPhone 17 Pro Dynamic Island)
  const hasNotchOrIslandStatusBar = Boolean(
    (bgLumaLeft >= 145 && bgLumaRight >= 145) ||
    (islandAvgLuma < 32 && islandMaxLuma < 52)
  );
  const hasLeftClockPresence = Boolean(
    (leftClockMaxLuma - leftClockMinLuma) >= 18 &&
    leftClockMaxLuma >= (bgLumaLeft + 16)
  );
  const hasRightStatusPresence = Boolean(
    (rightStatusMaxLuma - rightStatusMinLuma) >= 18
  );

  // Tahap 90 Pilar 3 & Tahap 109 Pilar 2: YouTube Landscape OSD vertical red scrub progress bar detector (only thin vertical red line <= 3px on 1 column x)
  let maxThinRedColRows = 0;
  if (!hasIosHomeScreenDock) {
    for (let x_col = 18; x_col <= 34; x_col++) {
      let thinRedRows = 0;
      for (let y = 20; y <= 225; y++) {
        const p = getPixel(x_col, y);
        if (p.r >= 170 && p.g <= 55 && p.b <= 70 && (p.r - p.g) >= 115) {
          const pLeft = getPixel(Math.max(0, x_col - 4), y);
          const pRight = getPixel(Math.min(119, x_col + 4), y);
          const leftIsRed = (pLeft.r - pLeft.g) >= 55;
          const rightIsRed = (pRight.r - pRight.g) >= 55;
          if (!leftIsRed && !rightIsRed) {
            thinRedRows++;
          }
        }
      }
      if (thinRedRows > maxThinRedColRows) {
        maxThinRedColRows = thinRedRows;
      }
    }
  }
  let hasYouTubeLandscapeOsd = Boolean(
    maxThinRedColRows >= 35 &&
    !hasIosHomeScreenDock &&
    !hasNotchColoredClockPillStatusBar
  );

  // Tahap 93 Pilar 2: Universal Calm-Flank Status Bar (White clock "15.16" on mid-bright mist wallpaper bgLumaLeft = 60..195)
  const isCalmFlankStatus = Boolean(
    !hasYouTubeLandscapeOsd &&
    leftFlankAvgSat <= 32 &&
    colorfulLeftZoneCount <= 8 &&
    (leftClockMaxLuma - leftClockMinLuma) >= 32 &&
    leftClockMaxLuma >= 175 &&
    edgeCountLeft >= 3 &&
    (rightStatusMaxLuma - rightStatusMinLuma) >= 24 &&
    edgeCountRight >= 3
  );

  // Tahap 108 Pilar 1 & Tahap 109 Pilar 4: Status Bar Color & Plain White Clock Detector (y=3..9, iPhone 11 Pro Max & iPhone 17 Pro)
  let colorfulStatusRowCount = 0;
  for (let y = 3; y <= 9; y++) {
    let rowColorful = false;
    for (let x = 6; x <= 38; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma > 75 && sat > 45) {
        rowColorful = true;
        break;
      }
    }
    if (rowColorful) colorfulStatusRowCount++;
  }

  // Left Clock Zone: x=6..35, y=3..9
  let whiteClockGlyphPixels = 0;
  let clockHorizEdges = 0;
  for (let y = 3; y <= 9; y++) {
    for (let x = 6; x <= 35; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      const lLeft = getLuma(x - 1, y);
      const lRight = getLuma(x + 1, y);
      const lUp = getLuma(x, y - 1);
      const lDown = getLuma(x, y + 1);
      const minNeighborLuma = Math.min(lLeft, lRight, lUp, lDown);
      if (p.luma >= 145 && sat <= 32 && (p.luma - minNeighborLuma) >= 12) {
        whiteClockGlyphPixels++;
      }
      if (Math.abs(p.luma - lLeft) >= 12) {
        clockHorizEdges++;
      }
    }
  }

  // Right Status Zone (signal/WiFi/battery): x=84..115, y=3..9
  let rightStatusGlyphPixels = 0;
  let rightHorizEdges = 0;
  for (let y = 3; y <= 9; y++) {
    for (let x = 84; x <= 115; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      const lLeft = getLuma(x - 1, y);
      const lRight = getLuma(x + 1, y);
      const lUp = getLuma(x, y - 1);
      const lDown = getLuma(x, y + 1);
      const minNeighborLuma = Math.min(lLeft, lRight, lUp, lDown);
      if (p.luma >= 115 && sat <= 42 && (p.luma - minNeighborLuma) >= 12) {
        rightStatusGlyphPixels++;
      }
      if (Math.abs(p.luma - lLeft) >= 12) {
        rightHorizEdges++;
      }
    }
  }

  const hasPlainWhiteClockStatusBar = Boolean(
    whiteClockGlyphPixels >= 4 &&
    clockHorizEdges >= 4 &&
    rightStatusGlyphPixels >= 3 &&
    rightHorizEdges >= 3 &&
    colorfulStatusRowCount <= 8
  );

  let hasTruePortraitStatusBar = Boolean(
    !hasYouTubeLandscapeOsd &&
    (
      hasNotchColoredClockPillStatusBar ||
      hasPlainWhiteClockStatusBar ||
      isCalmFlankStatus ||
      (
        hasNotchOrIslandStatusBar &&
        (
          ((maxLumaLeft - minLumaLeft) >= 36 && edgeCountLeft >= 4 && achromaticClockGlyphCount >= 3) ||
          (hasLeftClockPresence && edgeCountLeft >= 3 && achromaticClockGlyphCount >= 3 && bgLumaLeft < 65 && colorfulLeftZoneCount <= 5 && avgFlankSat <= 30 && blueBackBtnCount <= 3)
        ) &&
        (
          ((maxLumaRight - minLumaRight) >= 36 && edgeCountRight >= 3) ||
          (hasRightStatusPresence && bgLumaLeft < 65)
        ) &&
        (neonLeftCount <= 8) &&
        (neonNearIslandCount <= 6) &&
        (colorfulLeftZoneCount < 10)
      )
    )
  );
  const hasPortraitStatusBar = hasTruePortraitStatusBar;

  // Tahap 86 Pilar 1: Top Bright Pixels in y=2..20 (for TikTok / Translate header detection)
  let topBrightPixels_2_20 = 0;
  for (let y = 2; y <= 20; y++) {
    for (let x = 4; x <= 116; x++) {
      if (getLuma(x, y) >= 45) topBrightPixels_2_20++;
    }
  }

  // Tahap 87 Pilar 1: 6-Block Segment Horizontal Pillarbox Method (x=6..113 -> 6 blocks @ 18px)
  // Block 0: x=6..23, Block 1: x=24..41, Block 2: x=42..59, Block 3: x=60..77, Block 4: x=78..95, Block 5: x=96..113
  const pillarBlocks = [
    [6, 23], [24, 41], [42, 59], [60, 77], [78, 95], [96, 113]
  ];

  // Top Pillarbox: y = 2..8 (Tahap 89 Pilar 1: Precision narrow row avoiding 18:9 widescreen video and OSD)
  let topBlackPixels = 0, topTotalPixels = 0;
  let topPureBlackBlocks = 0;
  let topBlock0IsPure = false, topBlock5IsPure = false;
  let topLeftPillarSum = 0, topLeftPillarCnt = 0;
  let topCenterPillarSum = 0, topCenterPillarCnt = 0;
  let topRightPillarSum = 0, topRightPillarCnt = 0;
  let topLeftMaxLuma = 0, topCenterMaxLuma = 0, topRightMaxLuma = 0;
  let topPillarBrightPixels = 0;

  for (let bi = 0; bi < pillarBlocks.length; bi++) {
    const bx1 = pillarBlocks[bi][0], bx2 = pillarBlocks[bi][1];
    let bSum = 0, bCnt = 0, bMax = 0, bMin = 255;
    for (let y = 2; y <= 8; y++) {
      for (let x = bx1; x <= bx2; x++) {
        const p = getPixel(x, y);
        const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
        if (p.luma <= 18 && sat <= 18) topBlackPixels++;
        topTotalPixels++;
        const l = p.luma;
        bSum += l; bCnt++;
        if (l > bMax) bMax = l;
        if (l < bMin) bMin = l;
        if (l > 34) topPillarBrightPixels++;
        if (x <= 36) {
          topLeftPillarSum += l; topLeftPillarCnt++;
          if (l > topLeftMaxLuma) topLeftMaxLuma = l;
        } else if (x <= 83) {
          topCenterPillarSum += l; topCenterPillarCnt++;
          if (l > topCenterMaxLuma) topCenterMaxLuma = l;
        } else {
          topRightPillarSum += l; topRightPillarCnt++;
          if (l > topRightMaxLuma) topRightMaxLuma = l;
        }
      }
    }
    const bAvg = bCnt > 0 ? (bSum / bCnt) : 100;
    let bSatSum = 0;
    for (let y = 2; y <= 8; y++) {
      for (let x = bx1; x <= bx2; x++) {
        const p = getPixel(x, y);
        bSatSum += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      }
    }
    const bSatAvg = bCnt > 0 ? (bSatSum / bCnt) : 0;
    // Tahap 90 Pilar 2: Hitam Pekat ATAU Ambient Glow Halus
    const isValid = (bAvg <= 18 && bMax <= 32) || (bAvg <= 34 && bMax <= 44 && (bMax - bMin) <= 18 && bSatAvg <= 25);
    if (isValid) {
      topPureBlackBlocks++;
      if (bi === 0) topBlock0IsPure = true;
      if (bi === 5) topBlock5IsPure = true;
    }
  }
  const topPillarBlackRatio = topTotalPixels > 0 ? (topBlackPixels / topTotalPixels) : 0;
  const topPillarIsBlack = Boolean(
    topPureBlackBlocks >= 5 &&
    topBlock0IsPure &&
    topBlock5IsPure &&
    !hasNotchColoredClockPillStatusBar
  );

  // Tahap 90 Pilar 4 & Tahap 109: When top pillarbox or YouTube landscape OSD with top pillarbox is confirmed, hasTruePortraitStatusBar must be false
  if (topPillarIsBlack || (hasYouTubeLandscapeOsd && topPillarIsBlack)) {
    hasTruePortraitStatusBar = false;
  }

  const topLeftAvgLuma = topLeftPillarCnt > 0 ? (topLeftPillarSum / topLeftPillarCnt) : 100;
  const topCenterAvgLuma = topCenterPillarCnt > 0 ? (topCenterPillarSum / topCenterPillarCnt) : 100;
  const topRightAvgLuma = topRightPillarCnt > 0 ? (topRightPillarSum / topRightPillarCnt) : 100;
  const topPillarAvgLuma = (topLeftAvgLuma + topCenterAvgLuma + topRightAvgLuma) / 3;

  // Tahap 89 Pilar 2: Bottom Pillarbox Dual Clean Corridors on y=244..252
  // Corridor Left: x=24..43 (between watermark "R" x=4..21 and AssistiveTouch x=45..74)
  let botLeftCorrSum = 0, botLeftCorrCnt = 0, botLeftCorrMax = 0, botLeftCorrBlack = 0;
  for (let y = 244; y <= 252; y++) {
    for (let x = 24; x <= 43; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      const l = p.luma;
      botLeftCorrSum += l; botLeftCorrCnt++;
      if (l <= 18 && sat <= 18) botLeftCorrBlack++;
      if (l > botLeftCorrMax) botLeftCorrMax = l;
    }
  }
  const botLeftCorrAvg = botLeftCorrCnt > 0 ? (botLeftCorrSum / botLeftCorrCnt) : 100;
  const botLeftCorrBlackRatio = botLeftCorrCnt > 0 ? (botLeftCorrBlack / botLeftCorrCnt) : 0;

  // Corridor Right: x=76..102 (between AssistiveTouch x=45..74 and corner controls x=104..118)
  let botRightCorrSum = 0, botRightCorrCnt = 0, botRightCorrMax = 0, botRightCorrBlack = 0;
  for (let y = 244; y <= 252; y++) {
    for (let x = 76; x <= 102; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      const l = p.luma;
      botRightCorrSum += l; botRightCorrCnt++;
      if (l <= 18 && sat <= 18) botRightCorrBlack++;
      if (l > botRightCorrMax) botRightCorrMax = l;
    }
  }
  const botRightCorrAvg = botRightCorrCnt > 0 ? (botRightCorrSum / botRightCorrCnt) : 100;
  const botRightCorrBlackRatio = botRightCorrCnt > 0 ? (botRightCorrBlack / botRightCorrCnt) : 0;

  // Compute min and sat for corridors
  let botLeftCorrMin = 255, botLeftCorrSatSum = 0;
  for (let y = 244; y <= 252; y++) {
    for (let x = 24; x <= 43; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      botLeftCorrSatSum += sat;
      if (p.luma < botLeftCorrMin) botLeftCorrMin = p.luma;
    }
  }
  const botLeftCorrSatAvg = botLeftCorrCnt > 0 ? (botLeftCorrSatSum / botLeftCorrCnt) : 0;

  let botRightCorrMin = 255, botRightCorrSatSum = 0;
  for (let y = 244; y <= 252; y++) {
    for (let x = 76; x <= 102; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      botRightCorrSatSum += sat;
      if (p.luma < botRightCorrMin) botRightCorrMin = p.luma;
    }
  }
  const botRightCorrSatAvg = botRightCorrCnt > 0 ? (botRightCorrSatSum / botRightCorrCnt) : 0;

  // Tahap 90 Pilar 2: Koridor Pillarbox Valid (Hitam Pekat ATAU Ambient Glow Halus)
  const isLeftCorrValid = Boolean(
    (botLeftCorrAvg <= 16 && botLeftCorrMax <= 30 && botLeftCorrBlackRatio >= 0.84) ||
    (botLeftCorrAvg <= 32 && (botLeftCorrMax - botLeftCorrMin) <= 20 && botLeftCorrSatAvg <= 25)
  );
  const isRightCorrValid = Boolean(
    (botRightCorrAvg <= 16 && botRightCorrMax <= 30 && botRightCorrBlackRatio >= 0.84) ||
    (botRightCorrAvg <= 32 && (botRightCorrMax - botRightCorrMin) <= 20 && botRightCorrSatAvg <= 25)
  );
  const hasCleanBlackBottomCorridors = Boolean(isLeftCorrValid && isRightCorrValid);

  // Bottom Pillarbox: y = 238..250 (Tahap 88 Pilar 4, 89 & 90)
  let botBlackPixels = 0, botTotalPixels = 0;
  let botPureBlackBlocks = 0;
  let botLeftPillarSum = 0, botLeftPillarCnt = 0, botLeftMaxLuma = 0;
  let botRightPillarSum = 0, botRightPillarCnt = 0, botRightMaxLuma = 0;
  let botWingsBrightPixels = 0;

  for (let bi = 0; bi < pillarBlocks.length; bi++) {
    const bx1 = pillarBlocks[bi][0], bx2 = pillarBlocks[bi][1];
    let bSum = 0, bCnt = 0, bMax = 0, bMin = 255;
    let bSatSum = 0;
    for (let y = 238; y <= 250; y++) {
      for (let x = bx1; x <= bx2; x++) {
        const p = getPixel(x, y);
        const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
        if (p.luma <= 18 && sat <= 18) botBlackPixels++;
        botTotalPixels++;
        const l = p.luma;
        bSum += l; bCnt++;
        bSatSum += sat;
        if (l > bMax) bMax = l;
        if (l < bMin) bMin = l;
        if (x <= 44) {
          botLeftPillarSum += l; botLeftPillarCnt++;
          if (l > botLeftMaxLuma) botLeftMaxLuma = l;
        } else if (x >= 76) {
          botRightPillarSum += l; botRightPillarCnt++;
          if (l > botRightMaxLuma) botRightMaxLuma = l;
        }
      }
    }
    const bAvg = bCnt > 0 ? (bSum / bCnt) : 100;
    const bSatAvg = bCnt > 0 ? (bSatSum / bCnt) : 0;
    if ((bAvg <= 18 && bMax <= 32) || (bAvg <= 34 && bMax <= 44 && (bMax - bMin) <= 20 && bSatAvg <= 25)) {
      botPureBlackBlocks++;
    }
  }
  const botPillarBlackRatio = botTotalPixels > 0 ? (botBlackPixels / botTotalPixels) : 0;
  const botPillarIsBlack = Boolean(
    hasCleanBlackBottomCorridors ||
    (botPureBlackBlocks >= 4 && botPillarBlackRatio >= 0.74)
  );

  const botLeftAvgLuma = botLeftPillarCnt > 0 ? (botLeftPillarSum / botLeftPillarCnt) : 100;
  const botRightAvgLuma = botRightPillarCnt > 0 ? (botRightPillarSum / botRightPillarCnt) : 100;
  const botPillarAvgLuma = (botLeftAvgLuma + botRightAvgLuma) / 2;

  // Tahap 83 Pilar 2: Instagram Reels Header Row ("Reels Friends" / song title at x=12..72, y=12..22)
  let reelsHeaderMinLuma = 255, reelsHeaderMaxLuma = 0;
  let reelsHeaderBrightPixels = 0;
  for (let y = 12; y <= 22; y += 2) {
    for (let x = 12; x <= 72; x += 2) {
      const p = getPixel(x, y);
      if (p.luma < reelsHeaderMinLuma) reelsHeaderMinLuma = p.luma;
      if (p.luma > reelsHeaderMaxLuma) reelsHeaderMaxLuma = p.luma;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 60 && sat <= 35) reelsHeaderBrightPixels++;
    }
  }

  // Tahap 83 Pilar 2: Instagram Reels Dark Bottom Navigation Icons (x=14..106, y=233..246)
  let botNavDarkBgMinLuma = 255, botNavIconMaxLuma = 0;
  let botNavBrightIconPixels = 0;
  for (let y = 233; y <= 246; y += 2) {
    for (let x = 14; x <= 106; x += 2) {
      const p = getPixel(x, y);
      if (p.luma < botNavDarkBgMinLuma) botNavDarkBgMinLuma = p.luma;
      if (p.luma > botNavIconMaxLuma) botNavIconMaxLuma = p.luma;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 52 && sat <= 35 && x >= 14 && x <= 90) {
        botNavBrightIconPixels++;
      }
    }
  }

  let hasInstagramReelsDarkChrome = Boolean(
    !(topPillarIsBlack && (hasCleanBlackBottomCorridors || botPillarIsBlack)) &&
    islandAvgLuma < 25 &&
    botNavDarkBgMinLuma <= 28 &&
    botNavIconMaxLuma >= 55 &&
    (botNavIconMaxLuma - botNavDarkBgMinLuma) >= 32 &&
    botNavBrightIconPixels >= 3 &&
    (
      (leftClockMaxLuma >= (bgLumaLeft + 22) && (leftClockMaxLuma - leftClockMinLuma) >= 24) ||
      (reelsHeaderMaxLuma >= 65 && (reelsHeaderMaxLuma - reelsHeaderMinLuma) >= 35 && reelsHeaderBrightPixels >= 3)
    )
  );

  // Tahap 108 Pilar 2: Instagram / Threads Dark Mode Chrome Detector (Feed & Reels on dark background)
  let botNavSum = 0, botNavSatSum = 0, botNavPixelCnt = 0;
  for (let y = 232; y <= 255; y += 2) {
    for (let x = 6; x <= 114; x += 4) {
      const p = getPixel(x, y);
      botNavSum += p.luma;
      botNavSatSum += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      botNavPixelCnt++;
    }
  }
  const botNavAvgLuma = botNavPixelCnt > 0 ? (botNavSum / botNavPixelCnt) : 100;
  const botNavAvgSat = botNavPixelCnt > 0 ? (botNavSatSum / botNavPixelCnt) : 100;

  // Check for bottom nav icons in y=235..246: distinct clusters of achromatic white pixels
  let botNavIconClusters = 0;
  for (const bx of [16, 38, 60, 82, 104]) {
    let hasIcon = false;
    for (let y = 235; y <= 246; y++) {
      for (let x = bx - 6; x <= bx + 6; x++) {
        const p = getPixel(x, y);
        const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
        if (p.luma >= 120 && sat <= 28) {
          hasIcon = true;
          break;
        }
      }
      if (hasIcon) break;
    }
    if (hasIcon) botNavIconClusters++;
  }

  // Header white glyph text in y=12..26, x=6..48 (e.g. logo "Instagram" / "Reels")
  let headerLogoWhiteGlyphs = 0;
  for (let y = 12; y <= 26; y++) {
    for (let x = 6; x <= 48; x++) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 120 && sat <= 28) {
        headerLogoWhiteGlyphs++;
      }
    }
  }

  // Tahap 82 Pilar 1: Bottom Navigation Bar Probes (TikTok [+] button & White Navigation Bar)
  let leftTabLumaSum = 0, leftTabCnt = 0;
  for (let y = 234; y <= 246; y += 2) {
    for (let x = 14; x <= 40; x += 4) {
      leftTabLumaSum += getLuma(x, y);
      leftTabCnt++;
    }
  }
  const leftTabAvgLuma = leftTabCnt > 0 ? (leftTabLumaSum / leftTabCnt) : 100;

  let rightTabLumaSum = 0, rightTabCnt = 0;
  for (let y = 234; y <= 246; y += 2) {
    for (let x = 80; x <= 106; x += 4) {
      rightTabLumaSum += getLuma(x, y);
      rightTabCnt++;
    }
  }
  const rightTabAvgLuma = rightTabCnt > 0 ? (rightTabLumaSum / rightTabCnt) : 100;

  // (Tahap 86: centerPlus & hasTikTokCenterPlusButton moved below hasDualBlackPillarbox)

  let botNavLeftSum = 0, botNavLeftSatSum = 0, botNavLeftCnt = 0;
  let botNavCenterSum = 0, botNavCenterSatSum = 0, botNavCenterCnt = 0;
  let botNavRightSum = 0, botNavRightSatSum = 0, botNavRightCnt = 0;

  for (let y = 230; y <= 248; y += 3) {
    for (let x = 10; x <= 38; x += 4) {
      const p = getPixel(x, y);
      botNavLeftSum += p.luma;
      botNavLeftSatSum += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      botNavLeftCnt++;
    }
    for (let x = 46; x <= 74; x += 4) {
      const p = getPixel(x, y);
      botNavCenterSum += p.luma;
      botNavCenterSatSum += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      botNavCenterCnt++;
    }
    for (let x = 82; x <= 110; x += 4) {
      const p = getPixel(x, y);
      botNavRightSum += p.luma;
      botNavRightSatSum += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      botNavRightCnt++;
    }
  }

  const botNavLeftAvgLuma = botNavLeftCnt > 0 ? (botNavLeftSum / botNavLeftCnt) : 0;
  const botNavLeftAvgSat = botNavLeftCnt > 0 ? (botNavLeftSatSum / botNavLeftCnt) : 100;
  const botNavCenterAvgLuma = botNavCenterCnt > 0 ? (botNavCenterSum / botNavCenterCnt) : 0;
  const botNavCenterAvgSat = botNavCenterCnt > 0 ? (botNavCenterSatSum / botNavCenterCnt) : 100;
  const botNavRightAvgLuma = botNavRightCnt > 0 ? (botNavRightSum / botNavRightCnt) : 0;
  const botNavRightAvgSat = botNavRightCnt > 0 ? (botNavRightSatSum / botNavRightCnt) : 100;

  const hasBottomWhiteNavBar = Boolean(
    botNavLeftAvgLuma >= 165 && botNavCenterAvgLuma >= 165 && botNavRightAvgLuma >= 165 &&
    botNavLeftAvgSat <= 32 && botNavCenterAvgSat <= 32 && botNavRightAvgSat <= 32
  );

  let centerVideoLumaSum = 0, centerVideoCnt = 0;
  let centerVideoMaxLuma = 0, centerVideoMinLuma = 255;
  for (let y = 35; y <= 220; y += 8) {
    for (let x = 20; x <= 100; x += 8) {
      const l = getLuma(x, y);
      centerVideoLumaSum += l;
      if (l > centerVideoMaxLuma) centerVideoMaxLuma = l;
      if (l < centerVideoMinLuma) centerVideoMinLuma = l;
      centerVideoCnt++;
    }
  }
  const activeVideoAvgLuma = centerVideoCnt > 0 ? (centerVideoLumaSum / centerVideoCnt) : centerVideoAvgLuma;

  // Tahap 89 Pilar 3: Fullscreen Active Video Body Probes (immune to AssistiveTouch at x=94..114)
  function checkZoneActive(yStart, yEnd) {
    let brightPix = 0, zSum = 0, zCnt = 0;
    let zMax = 0, zMin = 255;
    for (let y = yStart; y <= yEnd; y += 3) {
      for (let x = 12; x <= 85; x += 3) {
        const l = getLuma(x, y);
        zSum += l; zCnt++;
        if (l >= 40) brightPix++;
        if (l > zMax) zMax = l;
        if (l < zMin) zMin = l;
      }
    }
    const avgL = zCnt > 0 ? (zSum / zCnt) : 0;
    return Boolean(brightPix >= 8 && ((zMax - zMin) >= 30 || avgL >= 32));
  }
  const zoneTopActive = checkZoneActive(22, 88);
  const zoneMidActive = checkZoneActive(89, 162);
  const zoneBotActive = checkZoneActive(163, 230);
  const activeVideoZoneCount = (zoneTopActive ? 1 : 0) + (zoneMidActive ? 1 : 0) + (zoneBotActive ? 1 : 0);

  // Lower half video active y=135..236 (for presenter desk, slides, laptop)
  let lowerHalfBrightPix = 0, lowerHalfSum = 0, lowerHalfCnt = 0;
  let lowerHalfMax = 0, lowerHalfMin = 255;
  for (let y = 135; y <= 236; y += 3) {
    for (let x = 12; x <= 85; x += 3) {
      const l = getLuma(x, y);
      lowerHalfSum += l; lowerHalfCnt++;
      if (l >= 32) lowerHalfBrightPix++;
      if (l > lowerHalfMax) lowerHalfMax = l;
      if (l < lowerHalfMin) lowerHalfMin = l;
    }
  }
  const lowerHalfAvg = lowerHalfCnt > 0 ? (lowerHalfSum / lowerHalfCnt) : 0;
  const lowerHalfVideoActive = Boolean(lowerHalfBrightPix >= 10 && ((lowerHalfMax - lowerHalfMin) >= 25 || lowerHalfAvg >= 28));

  // Center studio video active (y=72..185, x=12..108)
  let zoneMidBrightPixels = 0;
  for (let y = 72; y <= 185; y += 3) {
    for (let x = 12; x <= 108; x += 3) {
      if (getLuma(x, y) >= 36) zoneMidBrightPixels++;
    }
  }

  // Mid flanks and natural wood/skin pixels
  let midLeftFlankSum = 0, midLeftFlankCnt = 0, midLeftFlankBlack = 0;
  for (let y = 75; y <= 180; y += 4) {
    for (let x = 4; x <= 30; x += 4) {
      const l = getLuma(x, y);
      midLeftFlankSum += l; midLeftFlankCnt++;
      if (l <= 18) midLeftFlankBlack++;
    }
  }
  const midLeftFlankAvgLuma = midLeftFlankCnt > 0 ? (midLeftFlankSum / midLeftFlankCnt) : 0;
  const midLeftFlankBlackRatio = midLeftFlankCnt > 0 ? (midLeftFlankBlack / midLeftFlankCnt) : 0;

  let midRightFlankSum = 0, midRightFlankCnt = 0, midRightFlankBlack = 0;
  for (let y = 75; y <= 180; y += 4) {
    for (let x = 90; x <= 116; x += 4) {
      const l = getLuma(x, y);
      midRightFlankSum += l; midRightFlankCnt++;
      if (l <= 18) midRightFlankBlack++;
    }
  }
  const midRightFlankAvgLuma = midRightFlankCnt > 0 ? (midRightFlankSum / midRightFlankCnt) : 0;
  const midRightFlankBlackRatio = midRightFlankCnt > 0 ? (midRightFlankBlack / midRightFlankCnt) : 0;

  // Natural wood and human skin tone detector (sat >= 20, p.g >= 35, p.r > p.b + 12, p.g >= p.b, p.luma >= 35)
  let midCenterWarmWoodOrSkinPixels = 0;
  for (let y = 75; y <= 180; y += 4) {
    for (let x = 20; x <= 100; x += 4) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 20 && p.g >= 35 && p.r > (p.b + 12) && p.g >= p.b && p.luma >= 35) {
        midCenterWarmWoodOrSkinPixels++;
      }
    }
  }

  const isCenterStudioVideoActive = Boolean(
    zoneMidBrightPixels >= 20 &&
    midRightFlankAvgLuma >= 22 &&
    midCenterWarmWoodOrSkinPixels >= 12
  );

  const hasActiveFullscreenVideoBody = Boolean(
    (zoneTopActive && zoneBotActive) ||
    isCenterStudioVideoActive ||
    (lowerHalfVideoActive && (hasPreliminarySideBar || isCurrentlyInLandscape)) ||
    (zoneMidActive && (zoneTopActive || zoneBotActive) && midCenterWarmWoodOrSkinPixels >= 14)
  );
  const hasThreeZoneVideoContent = hasActiveFullscreenVideoBody;
  const hasBothVideoHalvesActive = hasActiveFullscreenVideoBody;

  // Tahap 88 Pilar 3: YouTube Portrait Player Layout Detection
  let topPlayerSum = 0, topPlayerCnt = 0, topPlayerVividPixels = 0;
  for (let y = 16; y <= 80; y += 4) {
    for (let x = 8; x <= 112; x += 4) {
      const p = getPixel(x, y);
      topPlayerSum += p.luma; topPlayerCnt++;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 35 && p.luma >= 45) topPlayerVividPixels++;
    }
  }
  const topPlayerAvgLuma = topPlayerCnt > 0 ? (topPlayerSum / topPlayerCnt) : 0;

  let dividerLeftSum = 0, dividerLeftCnt = 0, dividerLeftDark = 0;
  for (let y = 88; y <= 118; y += 3) {
    for (let x = 8; x <= 54; x += 3) {
      const l = getLuma(x, y);
      dividerLeftSum += l; dividerLeftCnt++;
      if (l <= 34) dividerLeftDark++;
    }
  }
  const dividerLeftAvgLuma = dividerLeftCnt > 0 ? (dividerLeftSum / dividerLeftCnt) : 100;
  const dividerLeftDarkRatio = dividerLeftCnt > 0 ? (dividerLeftDark / dividerLeftCnt) : 0;

  let dividerRightSum = 0, dividerRightCnt = 0, dividerRightDark = 0;
  for (let y = 88; y <= 118; y += 3) {
    for (let x = 58; x <= 108; x += 3) {
      const l = getLuma(x, y);
      dividerRightSum += l; dividerRightCnt++;
      if (l <= 34) dividerRightDark++;
    }
  }
  const dividerRightAvgLuma = dividerRightCnt > 0 ? (dividerRightSum / dividerRightCnt) : 100;
  const dividerRightDarkRatio = dividerRightCnt > 0 ? (dividerRightDark / dividerRightCnt) : 0;

  let lowerTwoThirdsDarkCount = 0, lowerTwoThirdsSum = 0, lowerTwoThirdsCnt = 0;
  let lowerTwoThirdsSatSum = 0;
  for (let y = 94; y <= 230; y += 4) {
    for (let x = 8; x <= 85; x += 4) {
      const p = getPixel(x, y);
      lowerTwoThirdsSum += p.luma;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      lowerTwoThirdsSatSum += sat;
      lowerTwoThirdsCnt++;
      if (p.luma <= 38 && sat <= 18) lowerTwoThirdsDarkCount++;
    }
  }
  const lowerTwoThirdsAvgLuma = lowerTwoThirdsCnt > 0 ? (lowerTwoThirdsSum / lowerTwoThirdsCnt) : 100;
  const lowerTwoThirdsAvgSat = lowerTwoThirdsCnt > 0 ? (lowerTwoThirdsSatSum / lowerTwoThirdsCnt) : 100;
  const lowerTwoThirdsDarkRatio = lowerTwoThirdsCnt > 0 ? (lowerTwoThirdsDarkCount / lowerTwoThirdsCnt) : 0;

  let midRightWarmSkinPixels = 0;
  for (let y = 85; y <= 175; y += 4) {
    for (let x = 45; x <= 105; x += 4) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 20 && p.g >= 35 && p.r > (p.b + 12) && p.g >= p.b && p.luma >= 35) {
        midRightWarmSkinPixels++;
      }
    }
  }

  const isYouTubePortraitPlayerLayout = Boolean(
    !hasPreliminarySideBar &&
    !isCurrentlyInLandscape &&
    !(topPillarIsBlack && botPillarIsBlack && hasActiveFullscreenVideoBody) &&
    avgFlankSat <= 35 &&
    vividFlankPixelCount <= 8 &&
    (topPlayerAvgLuma >= 38 || topPlayerVividPixels >= 18) &&
    lowerTwoThirdsDarkRatio >= 0.60 &&
    lowerTwoThirdsAvgSat <= 18 &&
    midRightWarmSkinPixels <= 12 &&
    (topPlayerAvgLuma - lowerTwoThirdsAvgLuma) >= 20 &&
    (
      (dividerLeftAvgLuma <= 34 && dividerRightAvgLuma <= 34) ||
      (dividerLeftDarkRatio >= 0.40 && dividerRightDarkRatio >= 0.40) ||
      Math.abs(dividerRightAvgLuma - dividerLeftAvgLuma) <= 35
    )
  );

  // Tahap 88 Pilar 3: App Launch Splash Screen Detector (immune to dark studio presenter in YouTube Fullscreen)
  let upperSplashSum = 0, upperSplashCnt = 0;
  let upperSplashBrightPixels = 0;
  for (let y = 18; y <= 70; y += 4) {
    for (let x = 6; x <= 85; x += 4) {
      const l = getLuma(x, y);
      upperSplashSum += l; upperSplashCnt++;
      if (l > 38) upperSplashBrightPixels++;
    }
  }
  const upperSplashAvgLuma = upperSplashCnt > 0 ? (upperSplashSum / upperSplashCnt) : 0;

  let lowerSplashSum = 0, lowerSplashCnt = 0;
  let lowerSplashBrightPixels = 0;
  for (let y = 182; y <= 232; y += 4) {
    for (let x = 6; x <= 85; x += 4) {
      const l = getLuma(x, y);
      lowerSplashSum += l; lowerSplashCnt++;
      if (l > 38) lowerSplashBrightPixels++;
    }
  }
  const lowerSplashAvgLuma = lowerSplashCnt > 0 ? (lowerSplashSum / lowerSplashCnt) : 0;

  const isAppSplashScreen = Boolean(
    !hasPreliminarySideBar &&
    !isCurrentlyInLandscape &&
    upperSplashAvgLuma <= 14 &&
    lowerSplashAvgLuma <= 14 &&
    upperSplashBrightPixels <= 3 &&
    lowerSplashBrightPixels <= 3 &&
    midLeftFlankAvgLuma <= 14 &&
    midLeftFlankBlackRatio >= 0.85 &&
    midRightFlankBlackRatio >= 0.72 &&
    midCenterWarmWoodOrSkinPixels <= 6
  );

  // Tahap 88 Pilar 2: Pure Black Wings Probes on y=12..46 (Below Status Bar y=2..10 & Above Caption y=52)
  let leftWing12_46Sum = 0, leftWing12_46Cnt = 0, leftWing12_46Max = 0, leftWing12_46Black = 0;
  let rightWing12_46Sum = 0, rightWing12_46Cnt = 0, rightWing12_46Max = 0, rightWing12_46Black = 0;
  let centerThumb12_46Max = 0;

  for (let y = 12; y <= 46; y += 2) {
    for (let x = 2; x <= 32; x += 2) {
      const l = getLuma(x, y);
      leftWing12_46Sum += l; leftWing12_46Cnt++;
      if (l <= 14) leftWing12_46Black++;
      if (l > leftWing12_46Max) leftWing12_46Max = l;
    }
    for (let x = 88; x <= 118; x += 2) {
      const l = getLuma(x, y);
      rightWing12_46Sum += l; rightWing12_46Cnt++;
      if (l <= 14) rightWing12_46Black++;
      if (l > rightWing12_46Max) rightWing12_46Max = l;
    }
    for (let x = 37; x <= 83; x += 2) {
      const l = getLuma(x, y);
      if (l > centerThumb12_46Max) centerThumb12_46Max = l;
    }
  }

  const leftWing12_46Avg = leftWing12_46Cnt > 0 ? (leftWing12_46Sum / leftWing12_46Cnt) : 100;
  const leftWing12_46BlackRatio = leftWing12_46Cnt > 0 ? (leftWing12_46Black / leftWing12_46Cnt) : 0;
  const rightWing12_46Avg = rightWing12_46Cnt > 0 ? (rightWing12_46Sum / rightWing12_46Cnt) : 100;
  const rightWing12_46BlackRatio = rightWing12_46Cnt > 0 ? (rightWing12_46Black / rightWing12_46Cnt) : 0;

  const hasTopBlackWings12_46 = Boolean(
    leftWing12_46Avg <= 10 &&
    rightWing12_46Avg <= 10 &&
    leftWing12_46Max <= 24 &&
    rightWing12_46Max <= 24 &&
    leftWing12_46BlackRatio >= 0.90 &&
    rightWing12_46BlackRatio >= 0.90
  );

  // Caption & Header row (y=48..78, x=6..114): caption text on left, "komentar" header center, "X" button right
  let header48_78_MaxLuma = 0, header48_78_BrightPixels = 0;
  for (let y = 48; y <= 78; y += 2) {
    for (let x = 6; x <= 114; x += 2) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma > header48_78_MaxLuma) header48_78_MaxLuma = p.luma;
      if (p.luma >= 55 && sat <= 30) header48_78_BrightPixels++;
    }
  }

  // Comments OLED list (y=74..230, x=8..85): dark OLED black sheet & text rows
  let darkSheetBgCount = 0, commentTextPixelCount = 0, sheetSampleCnt = 0;
  for (let y = 74; y <= 230; y += 5) {
    for (let x = 8; x <= 85; x += 5) {
      const p = getPixel(x, y);
      sheetSampleCnt++;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 0 && p.luma <= 40 && sat <= 18) darkSheetBgCount++;
      if (p.luma >= 65 && sat <= 28) commentTextPixelCount++;
    }
  }
  const darkSheetBgRatio = sheetSampleCnt > 0 ? (darkSheetBgCount / sheetSampleCnt) : 0;

  // Right heart column (x=96..114, y=80..225)
  let rightHeartSum = 0, rightHeartCnt = 0;
  for (let y = 80; y <= 225; y += 5) {
    for (let x = 96; x <= 114; x += 3) {
      rightHeartSum += getLuma(x, y);
      rightHeartCnt++;
    }
  }
  const rightHeartColAvgLuma = rightHeartCnt > 0 ? (rightHeartSum / rightHeartCnt) : 0;

  // Video center warm pixels (x=22..88, y=74..225): detects human face/arms in YouTube Fullscreen video
  let videoCenterWarmPixels = 0;
  for (let y = 74; y <= 225; y += 4) {
    for (let x = 22; x <= 88; x += 4) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 25 && p.r > (p.b + 15) && p.luma >= 40) videoCenterWarmPixels++;
    }
  }

  // Tahap 108 Pilar 4: Precision TikTok Comment Sheet OLED dark background check (x=12..100, y=90..225)
  let commentAreaDarkCount = 0, commentAreaWarmCount = 0, commentAreaTotal = 0;
  for (let y = 90; y <= 225; y += 3) {
    for (let x = 12; x <= 100; x += 3) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      commentAreaTotal++;
      if (p.luma <= 28 && sat <= 20) {
        commentAreaDarkCount++;
      }
      if (p.luma >= 45 || (sat >= 25 && p.r > p.b)) {
        commentAreaWarmCount++;
      }
    }
  }
  const commentSheetDarkPixelRatio = commentAreaTotal > 0 ? (commentAreaDarkCount / commentAreaTotal) : 0;
  const commentAreaBrightOrWarmRatio = commentAreaTotal > 0 ? (commentAreaWarmCount / commentAreaTotal) : 0;

  const isPortraitCommentSheetScreen = Boolean(
    !hasPreliminarySideBar &&
    commentSheetDarkPixelRatio >= 0.65 &&
    commentAreaBrightOrWarmRatio <= 0.30 &&
    hasTopBlackWings12_46 &&
    (centerThumb12_46Max >= 16 || header48_78_MaxLuma >= 55) &&
    header48_78_BrightPixels >= 4 &&
    darkSheetBgRatio >= 0.48 &&
    commentTextPixelCount >= 6 &&
    rightHeartColAvgLuma <= 38 &&
    videoCenterWarmPixels <= 24
  );
  const hasTopMiniVideoThumbnail = isPortraitCommentSheetScreen;

  // Tahap 88 Pilar 3, Tahap 89, & Tahap 108: YouTube Portrait Dark Mode with Lower White Ad Card
  let lowerCardSum = 0, lowerCardSatSum = 0, lowerCardCnt = 0;
  let lowerCardWhitePixels = 0;
  for (let y = 138; y <= 215; y += 4) {
    for (let x = 10; x <= 110; x += 4) {
      const p = getPixel(x, y);
      lowerCardSum += p.luma;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      lowerCardSatSum += sat;
      lowerCardCnt++;
      if (p.luma >= 170 && sat <= 32) {
        lowerCardWhitePixels++;
      }
    }
  }
  const lowerCardAvgLuma = lowerCardCnt > 0 ? (lowerCardSum / lowerCardCnt) : 0;
  const lowerCardAvgSat = lowerCardCnt > 0 ? (lowerCardSatSum / lowerCardCnt) : 100;
  const lowerCardWhiteRatio = lowerCardCnt > 0 ? (lowerCardWhitePixels / lowerCardCnt) : 0;

  // Title / divider band (x=8..85, y=84..135): must be dark in YouTube Portrait, but is bright in Fullscreen Mac Mini / Web scenes
  let midDividerBandSum = 0, midDividerBandCnt = 0, midDividerBandDark = 0;
  for (let y = 84; y <= 135; y += 4) {
    for (let x = 8; x <= 85; x += 4) {
      const p = getPixel(x, y);
      midDividerBandSum += p.luma; midDividerBandCnt++;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma <= 38 && sat <= 18) midDividerBandDark++;
    }
  }
  const midDividerBandAvgLuma = midDividerBandCnt > 0 ? (midDividerBandSum / midDividerBandCnt) : 100;
  const midDividerBandDarkRatio = midDividerBandCnt > 0 ? (midDividerBandDark / midDividerBandCnt) : 0;

  // Tahap 108 Pilar 3: 4-Layer YouTube Portrait Watch Page Layout
  const hasYt4LayerPortraitWatchLayout = Boolean(
    !hasPreliminarySideBar &&
    topPlayerAvgLuma >= 38 &&
    midDividerBandAvgLuma <= 38 &&
    midDividerBandDarkRatio >= 0.65 &&
    lowerCardAvgLuma >= 125 &&
    (lowerCardWhiteRatio >= 0.45 || lowerCardAvgSat <= 32)
  );

  let hasYouTubeDarkLowerWhiteCard = Boolean(
    !hasPreliminarySideBar &&
    !isCurrentlyInLandscape &&
    topPlayerAvgLuma >= 38 &&
    !(topPillarIsBlack && (hasCleanBlackBottomCorridors || botPillarIsBlack) && topPlayerAvgLuma < 35) &&
    (
      hasYt4LayerPortraitWatchLayout ||
      (
        bgLumaLeft < 50 &&
        bgLumaRight < 50 &&
        midDividerBandAvgLuma <= 38 &&
        midDividerBandDarkRatio >= 0.65 &&
        lowerCardAvgLuma >= 125 &&
        lowerCardAvgSat <= 32
      )
    )
  );

  // Tahap 89 Pilar 4: 6-Block Pillarbox + Dual Corridors YouTube Fullscreen Detection
  const hasDualBlackPillarbox = Boolean(
    topPillarIsBlack &&
    botPillarIsBlack &&
    hasActiveFullscreenVideoBody &&
    !hasNotchColoredClockPillStatusBar &&
    !hasTruePortraitStatusBar &&
    !isPortraitCommentSheetScreen &&
    !isYouTubePortraitPlayerLayout &&
    !isAppSplashScreen &&
    !hasYouTubeDarkLowerWhiteCard
  );
  const hasActiveCenterVideo = Boolean(
    (activeVideoAvgLuma >= 18 && (activeVideoAvgLuma - topPillarAvgLuma) >= 12 && (activeVideoAvgLuma - botPillarAvgLuma) >= 12) ||
    ((centerVideoMaxLuma - centerVideoMinLuma) >= 55) ||
    hasActiveFullscreenVideoBody
  );
  const isTopBlackLetterbox = hasDualBlackPillarbox;

  let centerPlusMaxLuma = 0;
  let centerPlusBrightCount = 0;
  for (let y = 233; y <= 245; y += 2) {
    for (let x = 52; x <= 68; x += 2) {
      const l = getLuma(x, y);
      if (l > centerPlusMaxLuma) centerPlusMaxLuma = l;
      if (l >= 165 && (l - leftTabAvgLuma) >= 70) centerPlusBrightCount++;
    }
  }

  let hasTikTokCenterPlusButton = Boolean(
    !hasDualBlackPillarbox &&
    !(topPillarIsBlack && (hasCleanBlackBottomCorridors || botPillarIsBlack)) &&
    topBrightPixels_2_20 >= 5 &&
    leftTabAvgLuma <= 36 &&
    rightTabAvgLuma <= 36 &&
    centerPlusMaxLuma >= 180 &&
    centerPlusBrightCount >= 4
  );

  // Tahap 86 Pilar 4: Google Translate Dark Mode Layout (Watermark & AssistiveTouch immune)
  let translateInputBgCount = 0, midBoxSum = 0, midBoxSatSum = 0, midBoxCnt = 0;
  let midBoxWarmColorPixels = 0;
  for (let y = 46; y <= 152; y += 6) {
    for (let x = 8; x <= 85; x += 6) {
      const p = getPixel(x, y);
      midBoxSum += p.luma;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      midBoxSatSum += sat;
      midBoxCnt++;
      if (p.luma >= 14 && p.luma <= 48 && sat <= 20) translateInputBgCount++;
      if (sat >= 28 && p.r > (p.b + 15)) midBoxWarmColorPixels++;
    }
  }
  const translateInputBgRatio = midBoxCnt > 0 ? (translateInputBgCount / midBoxCnt) : 0;
  const midBoxAvgLuma = midBoxCnt > 0 ? (midBoxSum / midBoxCnt) : 100;
  const midBoxAvgSat = midBoxCnt > 0 ? (midBoxSatSum / midBoxCnt) : 100;

  let micBluePixelCount = 0, totalBlueBtnPixels = 0;
  for (let y = 188; y <= 244; y += 2) {
    for (let x = 12; x <= 108; x += 2) {
      const p = getPixel(x, y);
      const isBlue = (p.b >= 110 && p.g >= 65 && (p.b - p.r) >= 18 && p.luma >= 50);
      if (isBlue) {
        totalBlueBtnPixels++;
        if (x >= 42 && x <= 78) micBluePixelCount++;
      }
    }
  }

  let translateHeaderBrightPixels = 0;
  for (let y = 11; y <= 42; y += 3) {
    for (let x = 6; x <= 112; x += 4) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 70 && sat <= 30) translateHeaderBrightPixels++;
    }
  }

  const isGoogleTranslateDarkLayout = Boolean(
    !hasDualBlackPillarbox &&
    topBrightPixels_2_20 >= 4 &&
    translateInputBgRatio >= 0.65 &&
    midBoxAvgLuma >= 14 &&
    midBoxAvgLuma <= 46 &&
    midBoxAvgSat <= 18 &&
    midBoxWarmColorPixels <= 4 &&
    (micBluePixelCount >= 5 || totalBlueBtnPixels >= 10 || translateHeaderBrightPixels >= 6)
  );
  let hasDarkPortraitAppChrome = Boolean(
    isGoogleTranslateDarkLayout ||
    isPortraitCommentSheetScreen
  );

  // (Tahap 88: lowerCard & hasYouTubeDarkLowerWhiteCard defined above before hasDualBlackPillarbox)

  // (Tahap 86: hasDualBlackPillarbox defined above before hasTikTokCenterPlusButton & isGoogleTranslateDarkLayout)

  // 4. Portrait Bottom Home Bar (Tahap 84: Regular Bright Home Bar + Dimmed Dark-Mode Home Bar y: 245..256)
  let maxBottomRun = 0;
  for (let y = 248; y <= 256; y++) {
    let curRun = 0;
    for (let x = 40; x <= 80; x++) {
      const barLuma = getLuma(x, y);
      const bgLuma = getLuma(x, y - 5);
      if (barLuma > 140 && (barLuma - bgLuma) > 25) {
        curRun++;
        if (curRun > maxBottomRun) maxBottomRun = curRun;
      } else {
        curRun = 0;
      }
    }
  }

  let hasDimmedDarkHomeBar = false;
  let aboveCenterSum = 0, aboveCenterCnt = 0;
  for (let ay = 238; ay <= 241; ay++) {
    for (let ax = 52; ax <= 68; ax += 2) {
      aboveCenterSum += getLuma(ax, ay);
      aboveCenterCnt++;
    }
  }
  const aboveCenterLuma = aboveCenterCnt > 0 ? (aboveCenterSum / aboveCenterCnt) : 0;

  for (let y = 246; y <= 255; y++) {
    let rowLeftSum = 0, rowLeftCnt = 0;
    for (let x = 8; x <= 28; x += 4) {
      rowLeftSum += getLuma(x, y);
      rowLeftCnt++;
    }
    const rowLeftLuma = rowLeftCnt > 0 ? (rowLeftSum / rowLeftCnt) : 100;

    let rowRightSum = 0, rowRightCnt = 0;
    for (let x = 92; x <= 112; x += 4) {
      rowRightSum += getLuma(x, y);
      rowRightCnt++;
    }
    const rowRightLuma = rowRightCnt > 0 ? (rowRightSum / rowRightCnt) : 100;

    let segLeftSum = 0, segLeftCnt = 0;
    for (let x = 40; x <= 50; x += 2) {
      segLeftSum += getLuma(x, y);
      segLeftCnt++;
    }
    const segLeftLuma = segLeftCnt > 0 ? (segLeftSum / segLeftCnt) : 0;

    let segMidSum = 0, segMidCnt = 0;
    for (let x = 54; x <= 66; x += 2) {
      segMidSum += getLuma(x, y);
      segMidCnt++;
    }
    const segMidLuma = segMidCnt > 0 ? (segMidSum / segMidCnt) : 0;

    let segRightSum = 0, segRightCnt = 0;
    for (let x = 70; x <= 80; x += 2) {
      segRightSum += getLuma(x, y);
      segRightCnt++;
    }
    const segRightLuma = segRightCnt > 0 ? (segRightSum / segRightCnt) : 0;

    if (rowLeftLuma <= 28 && rowRightLuma <= 28 &&
        segLeftLuma >= 45 && segMidLuma >= 45 && segRightLuma >= 45 &&
        Math.abs(segLeftLuma - segMidLuma) <= 25 &&
        Math.abs(segRightLuma - segMidLuma) <= 25 &&
        (segMidLuma - rowLeftLuma) >= 20 &&
        (segMidLuma - rowRightLuma) >= 20 &&
        (segMidLuma - aboveCenterLuma) >= 18) {
      hasDimmedDarkHomeBar = true;
      break;
    }
  }

  // AssistiveTouch in center bottom pillarbox discrimination (Tahap 89 Pilar 3)
  if (topPillarIsBlack && (hasCleanBlackBottomCorridors || botPillarIsBlack)) {
    hasDimmedDarkHomeBar = false;
  }
  let hasPortraitBottomBar = Boolean(
    !(topPillarIsBlack && (hasCleanBlackBottomCorridors || botPillarIsBlack)) &&
    (maxBottomRun >= 22 || hasDimmedDarkHomeBar)
  );

  // 5. Tahap 83 & 109: Landscape Side Home Bar Detection on Left Edge ONLY (x=1..4, y=88..172)
  let leftEdgeStrict = false;
  if (!hasIosHomeScreenDock && !hasBottomWhiteNavBar && !hasTikTokCenterPlusButton && !hasInstagramReelsDarkChrome && !(botNavAvgLuma <= 45 && botNavAvgSat <= 18 && botNavIconClusters >= 3)) {
    let innerColSum = 0, innerColCnt = 0;
    for (let y = 95; y <= 165; y += 2) {
      for (let x = 10; x <= 18; x += 2) {
        innerColSum += getLuma(x, y);
        innerColCnt++;
      }
    }
    const innerColAvgLuma = innerColCnt > 0 ? (innerColSum / innerColCnt) : 0;

    let topCapEdgeSum = 0, topCapEdgeCnt = 0;
    for (let y = 62; y <= 80; y += 2) {
      for (let x = 1; x <= 3; x++) {
        topCapEdgeSum += getLuma(x, y);
        topCapEdgeCnt++;
      }
    }
    const topCapEdgeLuma = topCapEdgeCnt > 0 ? (topCapEdgeSum / topCapEdgeCnt) : 0;

    let botCapEdgeSum = 0, botCapEdgeCnt = 0;
    for (let y = 180; y <= 198; y += 2) {
      for (let x = 1; x <= 3; x++) {
        botCapEdgeSum += getLuma(x, y);
        botCapEdgeCnt++;
      }
    }
    const botCapEdgeLuma = botCapEdgeCnt > 0 ? (botCapEdgeSum / botCapEdgeCnt) : 0;

    let nearInnerColSum = 0, nearInnerColCnt = 0;
    for (let y = 105; y <= 155; y += 2) {
      for (let x = 5; x <= 7; x++) {
        nearInnerColSum += getLuma(x, y);
        nearInnerColCnt++;
      }
    }
    const nearInnerColLuma = nearInnerColCnt > 0 ? (nearInnerColSum / nearInnerColCnt) : 0;

    for (let xEdge = 1; xEdge <= 4; xEdge++) {
      let midSum = 0, satSum = 0;
      for (let y = 95; y <= 165; y++) {
        const p = getPixel(xEdge, y);
        midSum += p.luma;
        satSum += (Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b));
      }
      const barMidLuma = midSum / 71;
      const barAvgSat = satSum / 71;

      // Tahap 108 & 109 Pilar 3: Inner Wall Isolation on x=8..13, y=98..162 (Thin Home Bar vs wide wall)
      if ((barMidLuma - innerColAvgLuma98_162) < 32) continue;
      if ((barMidLuma - topCapEdgeLuma) < 24) continue;
      if ((barMidLuma - botCapEdgeLuma) < 24) continue;
      if ((barMidLuma - nearInnerColLuma) < 22) continue;

      let topCapSum = 0;
      for (let y = 68; y <= 78; y++) topCapSum += getLuma(xEdge, y);
      const topCapLuma = topCapSum / 11;

      let botCapSum = 0;
      for (let y = 182; y <= 192; y++) botCapSum += getLuma(xEdge, y);
      const botCapLuma = botCapSum / 11;

      // Condition: bar must be vertically isolated at both ends with low saturation
      // AND significantly brighter than inner column x=10..18 (anti-square-video wall)
      if ((barMidLuma - topCapLuma >= 25) && (barMidLuma - botCapLuma >= 25) && (barMidLuma - innerColAvgLuma >= 25) && barMidLuma >= 118 && barAvgSat <= 25) {
        let runLen = 0;
        let prevL = -1;
        let maxDev = 0;
        let darkContrastCount = 0;
        for (let y = 88; y <= 172; y++) {
          const barLuma = getLuma(xEdge, y);
          if (barLuma >= 118) {
            if (runLen === 0) {
              runLen = 1;
              prevL = barLuma;
              maxDev = 0;
              darkContrastCount = 0;
            } else {
              const dev = Math.abs(barLuma - prevL);
              if (dev > maxDev) maxDev = dev;
              prevL = barLuma;
              runLen++;
            }
            const innerLuma = getLuma(Math.min(119, xEdge + 6), y);
            if ((barLuma - innerLuma) >= 22) {
              darkContrastCount++;
            }
          } else {
            if (runLen >= 28 && maxDev < 10 && (darkContrastCount / runLen) >= 0.70) {
              leftEdgeStrict = true;
              break;
            }
            runLen = 0;
            prevL = -1;
          }
        }
        if (runLen >= 28 && maxDev < 10 && (darkContrastCount / runLen) >= 0.70) {
          leftEdgeStrict = true;
          break;
        }
      }
    }
  }

  let hasLandscapeSideBar = Boolean(leftEdgeStrict);
  const hasStrictThinEdgeHomeBar = hasLandscapeSideBar;

  const hasInstagramDarkModeChrome = Boolean(
    !hasLandscapeSideBar &&
    !hasYouTubeLandscapeOsd &&
    botNavAvgLuma <= 45 &&
    botNavAvgSat <= 18 &&
    (hasPortraitBottomBar || botNavIconClusters >= 3) &&
    (hasTruePortraitStatusBar || headerLogoWhiteGlyphs >= 8 || reelsHeaderBrightPixels >= 6)
  );

  // 6. Tahap 93 Pilar 3: Anti-False-Positive Guard for Home Screen "Social" / "Musik" Folder & Overlays
  let topBlurSum = 0, topBlurSatSum = 0, topBlurCnt = 0;
  let topBlurOuterVivid = 0;
  for (let y = 14; y <= 48; y += 4) {
    for (let x = 8; x <= 112; x += 4) {
      const p = getPixel(x, y);
      topBlurSum += p.luma;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      topBlurSatSum += sat;
      topBlurCnt++;
      if (sat >= 45 && p.luma >= 40) topBlurOuterVivid++;
    }
  }
  const topBlurAvgLuma = topBlurCnt > 0 ? (topBlurSum / topBlurCnt) : 0;
  const topBlurAvgSat = topBlurCnt > 0 ? (topBlurSatSum / topBlurCnt) : 100;
  const avgLumaTopZone = topBlurAvgLuma;

  let botBlurSum = 0, botBlurSatSum = 0, botBlurCnt = 0;
  let botBlurOuterVivid = 0;
  for (let y = 198; y <= 238; y += 4) {
    for (let x = 8; x <= 112; x += 4) {
      const p = getPixel(x, y);
      botBlurSum += p.luma;
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      botBlurSatSum += sat;
      botBlurCnt++;
      if (sat >= 45 && p.luma >= 40) botBlurOuterVivid++;
    }
  }
  const botBlurAvgLuma = botBlurCnt > 0 ? (botBlurSum / botBlurCnt) : 0;
  const botBlurAvgSat = botBlurCnt > 0 ? (botBlurSatSum / botBlurCnt) : 100;
  const avgLumaBottomZone = botBlurAvgLuma;
  const folderOuterVividPixels = topBlurOuterVivid + botBlurOuterVivid;

  let folderCenterMinLuma = 255, folderCenterMaxLuma = 0;
  for (let y = 70; y <= 180; y += 4) {
    for (let x = 20; x <= 100; x += 4) {
      const l = getLuma(x, y);
      if (l < folderCenterMinLuma) folderCenterMinLuma = l;
      if (l > folderCenterMaxLuma) folderCenterMaxLuma = l;
    }
  }

  // (minFlankLuma, maxFlankLuma, avgFlankSat computed earlier at top)

  // Tahap 84: Light portrait app chrome (Instagram, Threads, Safari, YouTube Portrait Dark Mode with Lower White Card)
  const hasTopWhiteChrome = Boolean(
    (bgLumaLeft >= 160 && bgLumaRight >= 160 && Math.abs(bgLumaLeft - bgLumaRight) <= 30 && avgFlankSat <= 35) ||
    (topWhitePixelCount >= 35 && avgFlankSat <= 35)
  );
  let hasLightPortraitAppChrome = Boolean(hasTopWhiteChrome || hasBottomWhiteNavBar || hasYouTubeDarkLowerWhiteCard);

  // Shifted status row in Control Center (Signal, Wi-Fi, Battery "85%" moved down to y=18..31, x=58..110)
  let shiftedStatusMin = 255;
  let shiftedStatusMax = 0;
  let shiftedStatusEdgeCount = 0;
  let shiftedStatusWhiteCount = 0;

  for (let y = 18; y <= 31; y++) {
    for (let x = 58; x <= 109; x++) {
      const p1 = getPixel(x, y);
      const p2 = getPixel(x + 1, y);
      if (p1.luma < shiftedStatusMin) shiftedStatusMin = p1.luma;
      if (p1.luma > shiftedStatusMax) shiftedStatusMax = p1.luma;
      if (Math.abs(p1.luma - p2.luma) > 40) shiftedStatusEdgeCount++;
      const sat = Math.max(p1.r, p1.g, p1.b) - Math.min(p1.r, p1.g, p1.b);
      if (p1.luma >= 175 && sat <= 30) shiftedStatusWhiteCount++;
    }
  }

  const hasShiftedStatusRow = Boolean(
    (shiftedStatusMax - shiftedStatusMin) >= 65 &&
    (shiftedStatusEdgeCount >= 5 || shiftedStatusWhiteCount >= 6) &&
    edgeCountRight <= 4
  );

  // Vertical Brightness & Volume sliders in x=62..108, y=76..134
  let sliderBrightCount = 0;
  let sliderDarkCount = 0;
  for (let y = 76; y <= 134; y += 4) {
    for (let x = 62; x <= 108; x += 4) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (p.luma >= 175 && sat <= 25) {
        sliderBrightCount++;
      } else if (p.luma < 75) {
        sliderDarkCount++;
      }
    }
  }
  const hasCcSliders = Boolean(sliderBrightCount >= 4 && sliderDarkCount >= 8);

  // Connectivity toggles (blue/green buttons) in x=12..56, y=34..88
  let toggleColorCount = 0;
  for (let y = 34; y <= 88; y += 4) {
    for (let x = 12; x <= 56; x += 4) {
      const p = getPixel(x, y);
      const isBlue = (p.b > 170 && (p.b - p.r) > 65);
      const isGreen = (p.g > 150 && (p.g - p.r) > 45);
      if (isBlue || isGreen) {
        toggleColorCount++;
      }
    }
  }
  const hasCcToggles = Boolean(toggleColorCount >= 2);

  // Expanded media platter check (02:15): smooth blur top & bottom
  let ccTopBlurSum = 0, ccTopBlurCnt = 0;
  for (let y = 10; y <= 28; y += 4) {
    for (let x = 10; x <= 110; x += 8) {
      ccTopBlurSum += getLuma(x, y);
      ccTopBlurCnt++;
    }
  }
  const ccTopBlurAvg = ccTopBlurCnt > 0 ? (ccTopBlurSum / ccTopBlurCnt) : 0;

  let ccBotBlurSum = 0, ccBotBlurCnt = 0;
  for (let y = 228; y <= 250; y += 4) {
    for (let x = 10; x <= 110; x += 8) {
      ccBotBlurSum += getLuma(x, y);
      ccBotBlurCnt++;
    }
  }
  const ccBotBlurAvg = ccBotBlurCnt > 0 ? (ccBotBlurSum / ccBotBlurCnt) : 0;

  // Golden Geometry: isLandscapeMenuLayout
  // Left edge strip x=1..8, y=20..240 (dark navy header of eFootball Landscape)
  let leftStripSum = 0, leftStripCnt = 0;
  for (let y = 20; y <= 240; y += 10) {
    for (let x = 1; x <= 8; x += 2) {
      leftStripSum += getLuma(x, y);
      leftStripCnt++;
    }
  }
  const leftEdgeStripAvgLuma = leftStripCnt > 0 ? (leftStripSum / leftStripCnt) : 100;

  // Right edge strip x=108..118, y=20..240 (dark navy header of eFootball Landscape)
  let rightStripSum = 0, rightStripCnt = 0;
  for (let y = 20; y <= 240; y += 10) {
    for (let x = 108; x <= 118; x += 2) {
      rightStripSum += getLuma(x, y);
      rightStripCnt++;
    }
  }
  const rightEdgeStripAvgLuma = rightStripCnt > 0 ? (rightStripSum / rightStripCnt) : 100;

  // Center body x=35..85, y=20..240
  let centerBodySum = 0, centerBodyCnt = 0;
  for (let y = 20; y <= 240; y += 10) {
    for (let x = 35; x <= 85; x += 10) {
      centerBodySum += getLuma(x, y);
      centerBodyCnt++;
    }
  }
  const centerBodyAvgLuma = centerBodyCnt > 0 ? (centerBodySum / centerBodyCnt) : 0;

  // (Tahap 88 & 109: pitchGreenCount computed early above for hasIosHomeScreenDock)

  // (blueBackBtnCount computed earlier at top)

  let botRightPanelSum = 0, botRightPanelCnt = 0;
  for (let y = 190; y <= 240; y += 6) {
    for (let x = 80; x <= 116; x += 4) {
      botRightPanelSum += getLuma(x, y);
      botRightPanelCnt++;
    }
  }
  const botRightPanelAvgLuma = botRightPanelCnt > 0 ? (botRightPanelSum / botRightPanelCnt) : 100;

  let isEfootballFormationScreen = Boolean(
    !hasYt4LayerPortraitWatchLayout &&
    !hasYouTubeDarkLowerWhiteCard &&
    islandAvgLuma < 28 &&
    blueBackBtnCount >= 6 &&
    belowClockBluePixels >= 4 &&
    pitchGreenCount >= 75 &&
    botRightPanelAvgLuma < 52 &&
    !hasTruePortraitStatusBar &&
    !hasNotchColoredClockPillStatusBar &&
    rightEdgeStripAvgLuma <= 38
  );

  // Tahap 92 Pilar 2: White portrait bar detection (placed early for isEfootballNeonPosterScreen)
  const hasWhitePortraitTopBar = Boolean(
    bgLumaLeft >= 170 &&
    bgLumaRight >= 170 &&
    Math.abs(bgLumaLeft - bgLumaRight) <= 22 &&
    avgFlankSat <= 28
  );

  const hasWhitePortraitBottomBar = Boolean(
    !topPillarIsBlack &&
    hasBottomWhiteNavBar &&
    botNavLeftAvgLuma >= 170 &&
    botNavCenterAvgLuma >= 170 &&
    botNavRightAvgLuma >= 170
  );

  // Tahap 93 Pilar 1: eFootball 2027 Lionel Messi & Lamine Yamal Neon Poster Screen (01:04 - 01:14)
  let upperPosterNeonPixels = 0;
  for (let y = 4; y <= 88; y += 2) {
    for (let x = 8; x <= 112; x += 2) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 60 && p.luma >= 38) {
        upperPosterNeonPixels++;
      }
    }
  }

  let lowerPosterNeonPixels = 0;
  let lowerPosterPinkMagentaPixels = 0;
  let lowerTruePinkPurplePixels = 0;
  for (let y = 150; y <= 246; y += 2) {
    for (let x = 8; x <= 112; x += 2) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 60 && p.luma >= 38) {
        lowerPosterNeonPixels++;
      }
      if (sat >= 55 && p.r >= 140 && (p.r - p.g) >= 35) {
        lowerPosterPinkMagentaPixels++;
      }
      if (sat >= 55 && p.r >= 130 && p.b >= 110 && (p.r - p.g) >= 35) {
        lowerTruePinkPurplePixels++;
      }
    }
  }

  let midBeltDarkPixels = 0, midBeltTotalPixels = 0;
  let navyBeltPixels = 0;
  for (let y = 92; y <= 145; y += 2) {
    for (let x = 10; x <= 110; x += 2) {
      const p = getPixel(x, y);
      midBeltTotalPixels++;
      if (p.luma <= 38) midBeltDarkPixels++;
      if (p.luma <= 60 && p.b > (p.g + 5) && p.b >= 24) {
        navyBeltPixels++;
      }
    }
  }
  const midBeltDarkRatio = midBeltTotalPixels > 0 ? (midBeltDarkPixels / midBeltTotalPixels) : 0;
  const navyBeltRatio = midBeltTotalPixels > 0 ? (navyBeltPixels / midBeltTotalPixels) : 0;

  let isEfootballNeonPosterScreen = Boolean(
    !hasYt4LayerPortraitWatchLayout &&
    !hasYouTubeDarkLowerWhiteCard &&
    !hasWhitePortraitTopBar &&
    !hasWhitePortraitBottomBar &&
    !isPortraitCommentSheetScreen &&
    leftFlankAvgSat >= 48 &&
    rightFlankAvgSat >= 45 &&
    avgFlankSat >= 52 &&
    vividFlankPixelCount >= 16 &&
    topFlankElectricBlueMagentaPixels >= 12 &&
    upperPosterNeonPixels >= 32 &&
    lowerPosterNeonPixels >= 32 &&
    lowerTruePinkPurplePixels >= 15 &&
    navyBeltRatio >= 0.28
  );

  // Tahap 93 Pilar 3: Definition of isPortraitOverlayOrFolder (iOS Frosted Blur Folder e.g. "Social")
  const isPortraitOverlayOrFolder = Boolean(
    !hasLandscapeSideBar &&
    !hasDualBlackPillarbox &&
    !isEfootballNeonPosterScreen &&
    !isEfootballFormationScreen &&
    leftFlankAvgSat <= 30 &&
    topBlurAvgSat <= 30 &&
    botBlurAvgSat <= 30 &&
    topBlurAvgLuma >= 38 &&
    botBlurAvgLuma >= 38 &&
    Math.abs(topBlurAvgLuma - botBlurAvgLuma) <= 26 &&
    folderOuterVividPixels <= 6 &&
    (folderCenterMaxLuma - folderCenterMinLuma) >= 55
  );

  const hasCcExpandedPlatter = Boolean(
    botPillarAvgLuma > 20 &&
    centerBodyAvgLuma > 40 &&
    avgFlankSat < 42 &&
    vividFlankPixelCount < 12 &&
    !hasLandscapeSideBar
  );

  const hasCcModules = Boolean(hasCcSliders || hasCcToggles || hasCcExpandedPlatter);

  // Tahap 81 Pilar 2: Control Center View (Used strictly as entry-blocker, NOT exit-trigger!)
  const isControlCenterView = Boolean(
    !hasLandscapeSideBar &&
    !hasDualBlackPillarbox &&
    avgFlankSat < 42 &&
    vividFlankPixelCount < 12 &&
    colorfulLeftZoneCount < 8 &&
    blueBackBtnCount < 3 &&
    !isEfootballFormationScreen &&
    (
      hasShiftedStatusRow ||
      hasCcExpandedPlatter ||
      (hasCcSliders && hasCcModules) ||
      (islandAvgLuma < 25 && edgeCountLeft <= 4 && edgeCountRight <= 4 && hasCcModules) ||
      (botPillarAvgLuma > 20 && (hasCcSliders || hasCcToggles || ccTopBlurAvg > 24))
    )
  );

  // Tahap 84: Combined portrait bottom bar and definite portrait app screen
  let hasTikTokOrPortraitBottomBar = Boolean(hasPortraitBottomBar || hasTikTokCenterPlusButton || hasInstagramReelsDarkChrome);

  // Tahap 90 Pilar 1: Dark heuristic entry blockers strictly active when in portrait (to prevent false entry to landscape)
  const isDarkHeuristicEntryBlocker = Boolean(
    isGoogleTranslateDarkLayout ||
    isPortraitCommentSheetScreen ||
    hasInstagramReelsDarkChrome ||
    hasTikTokCenterPlusButton ||
    hasDimmedDarkHomeBar ||
    isAppSplashScreen
  );

  let isDefinitePortraitAppScreen = Boolean(
    !isCurrentlyInLandscape ? (
      hasLightPortraitAppChrome ||
      hasDarkPortraitAppChrome ||
      hasInstagramDarkModeChrome ||
      hasIosHomeScreenDock ||
      hasTruePortraitStatusBar ||
      hasTikTokOrPortraitBottomBar ||
      isControlCenterView ||
      isPortraitOverlayOrFolder ||
      isAppSplashScreen ||
      isYouTubePortraitPlayerLayout ||
      hasYouTubeDarkLowerWhiteCard ||
      isDarkHeuristicEntryBlocker
    ) : (
      // When in landscape: ONLY explicit portrait chrome can trigger definite portrait
      hasLightPortraitAppChrome ||
      hasInstagramDarkModeChrome ||
      hasIosHomeScreenDock ||
      hasTruePortraitStatusBar ||
      isControlCenterView ||
      isPortraitOverlayOrFolder
    )
  );

  let isLandscapeMenuLayout = Boolean(
    !hasYt4LayerPortraitWatchLayout &&
    !hasYouTubeDarkLowerWhiteCard &&
    !hasIosHomeScreenDock &&
    !hasTruePortraitStatusBar &&
    (
      (
        rightEdgeStripAvgLuma < 28 &&
        leftEdgeStripAvgLuma < 42 &&
        centerBodyAvgLuma > 42 &&
        bgLumaRight < 32 &&
        (colorfulLeftZoneCount >= 8 || vividFlankPixelCount >= 10)
      ) ||
      isEfootballFormationScreen ||
      isEfootballNeonPosterScreen
    ) &&
    !isControlCenterView &&
    !isDefinitePortraitAppScreen &&
    !hasTruePortraitStatusBar &&
    !hasNotchColoredClockPillStatusBar &&
    (rightEdgeStripAvgLuma <= 34 || isEfootballNeonPosterScreen || isEfootballFormationScreen)
  );

  // Tahap 93 Pilar 2: Dynamic Island must have vivid game graphics saturation to trigger hasNakedDynamicIsland
  const hasGameFlankSaturation = Boolean(
    leftFlankAvgSat >= 38 &&
    (avgFlankSat >= 48 || vividFlankPixelCount >= 16) &&
    !isControlCenterView &&
    !isDefinitePortraitAppScreen
  );

  let hasNakedDynamicIsland = Boolean(
    !hasYt4LayerPortraitWatchLayout &&
    !hasYouTubeDarkLowerWhiteCard &&
    islandAvgLuma < 25 &&
    islandMaxLuma < 45 &&
    !hasTruePortraitStatusBar &&
    !hasPortraitBottomBar &&
    !isPortraitOverlayOrFolder &&
    !isControlCenterView &&
    !isDefinitePortraitAppScreen &&
    hasGameFlankSaturation &&
    (leftFlankAvgLuma > 20 || rightFlankAvgLuma > 20) &&
    (overallAvgLuma > 16 || maxFlankLuma > 25)
  );

  // Tahap 108 & 109: When opened iOS Folder, Instagram Dark Mode, YouTube Portrait Watch Page, or iOS Home Screen Dock is confirmed, cancel landscape menu
  if (isPortraitOverlayOrFolder || hasInstagramDarkModeChrome || hasYt4LayerPortraitWatchLayout || hasYouTubeDarkLowerWhiteCard || hasIosHomeScreenDock) {
    isLandscapeMenuLayout = false;
    hasNakedDynamicIsland = false;
  }

  // Tahap 92 Pilar 2: Absolute Priority of isEfootballNeonPosterScreen & hasNakedDynamicIsland
  if (isEfootballNeonPosterScreen || hasNakedDynamicIsland) {
    hasNotchColoredClockPillStatusBar = false;
    hasTruePortraitStatusBar = false;
    hasPortraitBottomBar = false;
    hasDimmedDarkHomeBar = false;
    hasTikTokCenterPlusButton = false;
    hasInstagramReelsDarkChrome = false;
    isDefinitePortraitAppScreen = false;
    isLandscapeMenuLayout = true;
  }

  // Tahap 81 Pilar 1 & Tahap 90 Pilar 3: YouTube Fullscreen Landscape video detection
  let isYouTubeFullscreenLandscape = Boolean(
    (
      (hasDualBlackPillarbox && hasActiveCenterVideo) ||
      hasYouTubeLandscapeOsd
    ) &&
    !hasPortraitBottomBar &&
    !hasTruePortraitStatusBar &&
    !isPortraitOverlayOrFolder &&
    !isControlCenterView &&
    !isDefinitePortraitAppScreen
  );

  // Tahap 89 & 90: Sticky Fullscreen Pillarbox Lock during active landscape playback
  const hasStickyFullscreenPillarboxLock = Boolean(
    isCurrentlyInLandscape &&
    topPillarIsBlack &&
    botPillarIsBlack &&
    !hasNotchColoredClockPillStatusBar &&
    !hasTruePortraitStatusBar &&
    !isPortraitCommentSheetScreen
  );

  // Tahap 87, 89 & 90: Absolute Priority of hasDualBlackPillarbox, Sticky Lock, & YouTube Landscape OSD
  if (hasDualBlackPillarbox || hasStickyFullscreenPillarboxLock || hasYouTubeLandscapeOsd) {
    hasLightPortraitAppChrome = false;
    hasDarkPortraitAppChrome = false;
    hasTruePortraitStatusBar = false;
    hasTikTokOrPortraitBottomBar = false;
    hasPortraitBottomBar = false;
    hasTikTokCenterPlusButton = false;
    isDefinitePortraitAppScreen = false;
    isYouTubeFullscreenLandscape = true;
    _portraitVoteCount = 0;
  }

  // Tahap 90 Pilar 1 & Tahap 92 Pilar 3: The 4 Explicit Portrait Exit Proofs (strictPortraitExitProof)
  // Bukti Eksplisit 1: exitByLightPortraitChrome
  const exitByLightPortraitChrome = Boolean(
    !hasLandscapeSideBar &&
    !hasYouTubeLandscapeOsd &&
    !isEfootballNeonPosterScreen &&
    (hasWhitePortraitTopBar || hasWhitePortraitBottomBar)
  );

  // Bukti Eksplisit 2: exitByYouTubePortraitWatchPage (Minimized from fullscreen to portrait watch page)
  let ytWatchDividerLeftSum = 0, ytWatchDividerLeftCnt = 0, ytWatchDividerLeftDark = 0;
  for (let y = 86; y <= 126; y += 2) {
    for (let x = 8; x <= 54; x += 2) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      ytWatchDividerLeftSum += p.luma;
      ytWatchDividerLeftCnt++;
      if (p.luma <= 38 && sat <= 20) ytWatchDividerLeftDark++;
    }
  }
  const ytWatchDividerLeftAvg = ytWatchDividerLeftCnt > 0 ? (ytWatchDividerLeftSum / ytWatchDividerLeftCnt) : 100;
  const ytWatchDividerLeftRatio = ytWatchDividerLeftCnt > 0 ? (ytWatchDividerLeftDark / ytWatchDividerLeftCnt) : 0;

  let ytWatchDividerRightSum = 0, ytWatchDividerRightCnt = 0, ytWatchDividerRightDark = 0;
  for (let y = 86; y <= 126; y += 2) {
    for (let x = 58; x <= 108; x += 2) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      ytWatchDividerRightSum += p.luma;
      ytWatchDividerRightCnt++;
      if (p.luma <= 38 && sat <= 20) ytWatchDividerRightDark++;
    }
  }
  const ytWatchDividerRightAvg = ytWatchDividerRightCnt > 0 ? (ytWatchDividerRightSum / ytWatchDividerRightCnt) : 100;
  const ytWatchDividerRightRatio = ytWatchDividerRightCnt > 0 ? (ytWatchDividerRightDark / ytWatchDividerRightCnt) : 0;
  const ytWatchDividerDarkRatio = (ytWatchDividerLeftCnt + ytWatchDividerRightCnt) > 0 ?
    ((ytWatchDividerLeftDark + ytWatchDividerRightDark) / (ytWatchDividerLeftCnt + ytWatchDividerRightCnt)) : 0;

  const hasYtWatchDarkDivider = Boolean(
    ((ytWatchDividerLeftAvg <= 48 && ytWatchDividerRightAvg <= 48) || ytWatchDividerDarkRatio >= 0.70) &&
    Math.abs(ytWatchDividerRightAvg - ytWatchDividerLeftAvg) <= 22 &&
    ytWatchDividerDarkRatio >= 0.65
  );

  let ytWatchLowerCardSum = 0, ytWatchLowerCardCnt = 0;
  for (let y = 135; y <= 235; y += 4) {
    for (let x = 12; x <= 108; x += 4) {
      ytWatchLowerCardSum += getLuma(x, y);
      ytWatchLowerCardCnt++;
    }
  }
  const ytWatchLowerCardAvg = ytWatchLowerCardCnt > 0 ? (ytWatchLowerCardSum / ytWatchLowerCardCnt) : 0;

  const hasTruePortraitStatusBarOrPill = Boolean(
    hasNotchColoredClockPillStatusBar ||
    (
      hasTruePortraitStatusBar &&
      (
        hasNotchColoredClockPillStatusBar ||
        hasPlainWhiteClockStatusBar ||
        (islandAvgLuma <= 22 && edgeCountLeft >= 2 && edgeCountRight >= 2) ||
        (bgLumaLeft < 65 && bgLumaRight < 65 && leftClockMaxLuma >= 85 && rightStatusMaxLuma >= 80)
      )
    )
  );

  const exitByYouTubePortraitWatchPage = Boolean(
    !hasLandscapeSideBar &&
    !hasYouTubeLandscapeOsd &&
    !hasNakedDynamicIsland &&
    !isEfootballNeonPosterScreen &&
    !isEfootballFormationScreen &&
    !isLandscapeMenuLayout &&
    hasTruePortraitStatusBarOrPill &&
    hasYtWatchDarkDivider &&
    ytWatchLowerCardAvg >= 25
  );

  // Bukti Eksplisit 3: exitByHomeScreenPortrait (Swipe up to Home Screen with colorful dock icons)
  let homeDockColorfulPixels = 0;
  let botDockZoneSum = 0, botDockZoneCnt = 0;
  for (let y = 175; y <= 248; y += 2) {
    for (let x = 12; x <= 108; x += 2) {
      const p = getPixel(x, y);
      const sat = Math.max(p.r, p.g, p.b) - Math.min(p.r, p.g, p.b);
      if (sat >= 45 && p.luma >= 45) {
        homeDockColorfulPixels++;
      }
      if (y >= 224) {
        botDockZoneSum += p.luma;
        botDockZoneCnt++;
      }
    }
  }
  const botDockZoneAvgLuma = botDockZoneCnt > 0 ? (botDockZoneSum / botDockZoneCnt) : 0;

  const exitByHomeScreenPortrait = Boolean(
    hasIosHomeScreenDock ||
    (
      !hasLandscapeSideBar &&
      !hasYouTubeLandscapeOsd &&
      !hasNakedDynamicIsland &&
      !isEfootballNeonPosterScreen &&
      !isEfootballFormationScreen &&
      !isLandscapeMenuLayout &&
      (hasNotchColoredClockPillStatusBar || hasTruePortraitStatusBar) &&
      homeDockColorfulPixels >= 14 &&
      botDockZoneAvgLuma >= 42
    )
  );

  // Bukti Eksplisit 4: exitByIosFolderPortrait (Tahap 93 Pilar 3: Opened iOS Folder e.g. "Social")
  const exitByIosFolderPortrait = Boolean(
    !hasLandscapeSideBar &&
    !hasYouTubeLandscapeOsd &&
    isPortraitOverlayOrFolder &&
    !topPillarIsBlack &&
    !botPillarIsBlack
  );

  // Bukti Eksplisit 5: exitByInstagramDarkMode (Tahap 108 Pilar 2: Instagram / Threads Dark Mode)
  const exitByInstagramDarkMode = Boolean(
    !hasLandscapeSideBar &&
    !hasYouTubeLandscapeOsd &&
    hasInstagramDarkModeChrome
  );

  let strictPortraitExitProof = Boolean(
    exitByLightPortraitChrome ||
    exitByYouTubePortraitWatchPage ||
    exitByHomeScreenPortrait ||
    exitByIosFolderPortrait ||
    exitByInstagramDarkMode
  );

  // Tahap 92 Pilar 3 & Tahap 108: Absolute Hard Horizontal Lock for Landscape Game & Menu
  if (isCurrentlyInLandscape && (hasNakedDynamicIsland || isEfootballNeonPosterScreen || isEfootballFormationScreen || isLandscapeMenuLayout)) {
    if (!exitByIosFolderPortrait && !exitByHomeScreenPortrait && !exitByInstagramDarkMode) {
      strictPortraitExitProof = false;
    }
  }

  // Tahap 82, 89 & 90: Absolute Portrait Override (Never kill landscape if dual black pillarbox, sticky lock, or landscape session active)
  if (isDefinitePortraitAppScreen && !hasDualBlackPillarbox && !hasStickyFullscreenPillarboxLock && !hasYouTubeLandscapeOsd && (!isCurrentlyInLandscape || strictPortraitExitProof)) {
    hasLandscapeSideBar = false;
    hasNakedDynamicIsland = false;
    isLandscapeMenuLayout = false;
    isYouTubeFullscreenLandscape = false;
  }

  // Tahap 109: Absolute Override when hasIosHomeScreenDock is true
  if (hasIosHomeScreenDock) {
    hasPreliminarySideBar = false;
    hasLandscapeSideBar = false;
    hasNakedDynamicIsland = false;
    isEfootballNeonPosterScreen = false;
    isEfootballFormationScreen = false;
    isLandscapeMenuLayout = false;
    hasYouTubeLandscapeOsd = false;
    isYouTubeFullscreenLandscape = false;
  }

  // 7. Overall Landscape Game / Video determination
  const isLandscapeGameFrame = Boolean(
    (
      hasLandscapeSideBar ||
      hasNakedDynamicIsland ||
      isYouTubeFullscreenLandscape ||
      hasYouTubeLandscapeOsd ||
      isLandscapeMenuLayout ||
      (isCurrentlyInLandscape && !strictPortraitExitProof)
    ) &&
    (!isCurrentlyInLandscape ? !isDefinitePortraitAppScreen : !strictPortraitExitProof)
  );
  const isFullscreenGame = isLandscapeGameFrame;

  // Bottom dock symmetry for Portrait screens
  let botLeftSum = 0, botLeftCnt = 0;
  for (let y = 218; y <= 248; y += 4) {
    for (let x = 8; x <= 32; x += 4) {
      botLeftSum += getLuma(x, y);
      botLeftCnt++;
    }
  }
  const botLeftMarginLuma = botLeftCnt > 0 ? (botLeftSum / botLeftCnt) : 0;

  let botRightSum = 0, botRightCnt = 0;
  for (let y = 218; y <= 248; y += 4) {
    for (let x = 88; x <= 112; x += 4) {
      botRightSum += getLuma(x, y);
      botRightCnt++;
    }
  }
  const botRightMarginLuma = botRightCnt > 0 ? (botRightSum / botRightCnt) : 0;

  const hasSymmetricPortraitBottomZone = Boolean(Math.abs(botLeftMarginLuma - botRightMarginLuma) <= 28);

  // Tahap 85, 89, 90 & 92: Hard Horizontal Lock with Asymmetric Exit Gate
  const isLandscapeActive = Boolean(
    !isCurrentlyInLandscape ? (
      !isDefinitePortraitAppScreen &&
      (
        hasLandscapeSideBar ||
        isYouTubeFullscreenLandscape ||
        hasYouTubeLandscapeOsd ||
        isLandscapeMenuLayout ||
        hasNakedDynamicIsland ||
        isEfootballFormationScreen ||
        isEfootballNeonPosterScreen ||
        overallAvgLuma < 24
      )
    ) : (
      !strictPortraitExitProof
    )
  );

  let canPassiveExitToPortrait = false;
  if (isCurrentlyInLandscape) {
    if (hasNakedDynamicIsland || isEfootballNeonPosterScreen || isEfootballFormationScreen || isLandscapeMenuLayout) {
      canPassiveExitToPortrait = false;
      _portraitVoteCount = 0;
    } else {
      canPassiveExitToPortrait = Boolean(strictPortraitExitProof);
    }
  } else {
    canPassiveExitToPortrait = Boolean(
      !hasYouTubeLandscapeOsd &&
      !hasDualBlackPillarbox &&
      !hasStickyFullscreenPillarboxLock &&
      !isEfootballNeonPosterScreen &&
      (
        isDefinitePortraitAppScreen ||
        hasTruePortraitStatusBar ||
        ((hasLightPortraitAppChrome || hasDarkPortraitAppChrome || isPortraitOverlayOrFolder) && overallAvgLuma >= 24)
      )
    );
  }
  const canExitLandscapeToPortrait = canPassiveExitToPortrait;

  return {
    isFullscreenGame,
    isLandscapeGameFrame,
    hasTruePortraitStatusBar,
    hasPortraitStatusBar: hasTruePortraitStatusBar,
    hasPortraitBottomBar,
    hasLandscapeSideBar,
    hasStrictThinEdgeHomeBar: hasLandscapeSideBar,
    hasNakedDynamicIsland,
    isPortraitOverlayOrFolder,
    isYouTubeFullscreenLandscape,
    hasLightPortraitAppChrome,
    hasTikTokCenterPlusButton,
    hasTikTokOrPortraitBottomBar,
    hasInstagramReelsDarkChrome,
    topPillarIsBlack,
    botPillarIsBlack,
    hasThreeZoneVideoContent,
    isYouTubePortraitPlayerLayout,
    hasDarkPortraitAppChrome,
    isAppSplashScreen,
    hasDimmedDarkHomeBar,
    isGoogleTranslateDarkLayout,
    hasYouTubeDarkLowerWhiteCard,
    isPortraitCommentSheetScreen,
    isDefinitePortraitAppScreen,
    isLandscapeMenuLayout,
    isEfootballFormationScreen,
    isEfootballNeonPosterScreen,
    hasDualBlackPillarbox,
    isControlCenterView,
    canPassiveExitToPortrait,
    canExitLandscapeToPortrait,
    hasNotchColoredClockPillStatusBar,
    hasPlainWhiteClockStatusBar,
    hasIosHomeScreenDock,
    hasInstagramDarkModeChrome,
    exitByInstagramDarkMode,
    hasYouTubeLandscapeOsd,
    strictPortraitExitProof,
    exitByLightPortraitChrome,
    exitByYouTubePortraitWatchPage,
    exitByHomeScreenPortrait,
    exitByIosFolderPortrait,
    isCenterStudioVideoActive,
    hasActiveFullscreenVideoBody,
    activeVideoZoneCount,
    hasCleanBlackBottomCorridors,
    hasStickyFullscreenPillarboxLock,
    lowerHalfVideoActive,
    achromaticClockGlyphCount,
    neonLeftCount,
    neonNearIslandCount,
    colorfulLeftZoneCount,
    avgFlankSat,
    vividFlankPixelCount,
    overallAvgLuma,
    islandAvgLuma,
    islandMaxLuma,
    maxFlankLuma,
    bgLumaLeft,
    bgLumaRight,
  };
}

function detectGameLandscapeFromFrame(sourceFrame) {
  const info = analyzeFrameOrientation(sourceFrame);
  if (!info) return null;
  if (info.isLandscapeGameFrame || info.isFullscreenGame) return true;
  if (info.hasTruePortraitStatusBar || info.hasPortraitBottomBar || info.isPortraitOverlayOrFolder || info.isDefinitePortraitAppScreen || info.hasInstagramReelsDarkChrome || info.hasInstagramDarkModeChrome || info.hasDarkPortraitAppChrome || info.isAppSplashScreen || info.isPortraitCommentSheetScreen || info.hasNotchColoredClockPillStatusBar || info.hasPlainWhiteClockStatusBar || info.hasIosHomeScreenDock) return false;
  return null;
}
window.detectGameLandscapeFromFrame = detectGameLandscapeFromFrame;
window.analyzeFrameOrientation = analyzeFrameOrientation;

function checkAutoRotateOrientation(sourceFrame, rawW, rawH) {
  if (!autoRotateGameEnabled) return;
  const now = performance.now();
  if (now - _lastAutoRotateSampleTs < 1000) return;
  _lastAutoRotateSampleTs = now;

  // Tahap 77 Pilar 3: User Manual Rotate Cooldown (30s)
  if (Date.now() < manualRotateCooldownUntil) {
    return;
  }

  // Tahap 78 Pilar 1: If user recently exited to Home via Home button, block re-entering landscape for 2.8s
  if (Date.now() < _homeExitGraceUntil) {
    _landscapeVoteCount = 0;
    return;
  }

  try {
    const info = analyzeFrameOrientation(sourceFrame);
    if (!info) return;

    const isCurrentlyLandscape = (currentRotation === 90 || currentRotation === 270);

    if (!isCurrentlyLandscape) {
      // IN PORTRAIT: Rotate to landscape if confirmed fullscreen game or video (Block splash screens)
      if (info.isAppSplashScreen) {
        _landscapeVoteCount = 0;
        return;
      }
      if (info.isLandscapeGameFrame || info.isFullscreenGame) {
        _landscapeVoteCount++;
        _portraitVoteCount = 0;
        const threshold = info.hasLandscapeSideBar ? 1 : 2;
        if (_landscapeVoteCount >= threshold) {
          _autoRotatedToLandscape = true;
          setRotation(270);
          try {
            window.parent.postMessage({ type: "auto-rotate-request", landscape: true, rotation: 270 }, "*");
          } catch (_) {}
        }
      } else {
        _landscapeVoteCount = 0;
      }
    } else {
      // IN LANDSCAPE: Return to portrait if canPassiveExitToPortrait
      // Tahap 90 Asymmetric Landscape Lock & Ambient Pillarbox
      if (
        info.hasStickyFullscreenPillarboxLock ||
        info.hasDualBlackPillarbox ||
        info.hasYouTubeLandscapeOsd ||
        !info.canPassiveExitToPortrait ||
        info.isLandscapeGameFrame ||
        info.isYouTubeFullscreenLandscape ||
        info.isLandscapeMenuLayout ||
        info.hasLandscapeSideBar
      ) {
        _portraitVoteCount = 0;
        return;
      }

      const hasRecentNav = (Date.now() - _lastHomeOrRecentNavTs < 5000);
      _portraitVoteCount++;
      _landscapeVoteCount = 0;
      // Tahap 90: Require 3 consecutive samples (~1.8s) for passive exit, or 1 if user just clicked Home/Recent
      const exitThreshold = hasRecentNav ? 1 : 3;
      if (_portraitVoteCount >= exitThreshold) {
        _autoRotatedToLandscape = false;
        setRotation(0);
        try {
          window.parent.postMessage({ type: "auto-rotate-request", landscape: false, rotation: 0 }, "*");
        } catch (_) {}
      }
    }
  } catch (_) {}
}

const MIRROR = "/mirror";
const canvas = document.getElementById("c");
const ctx = canvas.getContext("2d", { alpha: false, desynchronized: true }) || canvas.getContext("2d");
const statusEl = document.getElementById("mirror-status");

let visualRotation = 0;
let currentRotation = 0;
let zoom = 1;
const ZOOM_STEP = 1.2;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 3;

function setRotation(deg) {
  currentRotation = ((deg % 360) + 360) % 360;
  visualRotation = currentRotation;
  const isLandscape = (currentRotation === 90 || currentRotation === 270);
  _autoRotatedToLandscape = isLandscape;

  if (lastFrame) {
    const rawW = (lastFrame.codedWidth && lastFrame.codedHeight > lastFrame.displayHeight)
      ? lastFrame.codedWidth
      : (lastFrame.displayWidth || lastFrame.width || 1216);
    const rawH = (lastFrame.codedHeight && lastFrame.codedHeight > lastFrame.displayHeight)
      ? lastFrame.codedHeight
      : (lastFrame.displayHeight || lastFrame.height || 2656);
    if (isLandscape) {
      canvas.width = Math.max(rawW, rawH);
      canvas.height = Math.min(rawW, rawH);
    } else {
      canvas.width = Math.min(rawW, rawH);
      canvas.height = Math.max(rawW, rawH);
    }
  }

  fitCanvas();

  try {
    if (window.parent && window.parent !== window) {
      window.parent.postMessage({
        type: "rotation-changed",
        rotation: currentRotation,
        isLandscape: isLandscape,
      }, "*");
    }
  } catch (_) {}

  if (lastFrame) {
    try { drawFrame(lastFrame); } catch (_) {}
  }
}

function toggleLandscapeMode() {
  if (currentRotation === 0 || currentRotation === 180) {
    setRotation(270);
  } else {
    setRotation(0);
  }
}
window.setRotation = setRotation;
window.toggleLandscapeMode = toggleLandscapeMode;
window.setFitMode = function(m) {
  fitMode = (m === "contain" || m === "fill") ? m : "autofit";
  try { localStorage.setItem("pane-fit-mode", fitMode); } catch (_) {}
  fitCanvas();
};

let fitMode = localStorage.getItem("pane-fit-mode") || "autofit";
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
let hasRenderedFirstFrame = false;
let streamActive = false;
window._isDeviceLocked = false;

function setStatus(text, tone) {
  if (!statusEl) return;
  if (!text) {
    statusEl.textContent = "";
    statusEl.style.display = "none";
    statusEl.hidden = true;
    return;
  }
  // Sembunyikan pill jika device dalam mode Standby / Locked
  try {
    const parentNote = window.parent?.document?.getElementById("screen-note");
    if (parentNote && (parentNote.classList.contains("calm-standby") || (parentNote.textContent && parentNote.textContent.includes("Terkunci")))) {
      statusEl.textContent = "";
      statusEl.style.display = "none";
      statusEl.hidden = true;
      return;
    }
    if (window._isDeviceLocked || document.body.classList.contains("device-locked")) {
      statusEl.textContent = "";
      statusEl.style.display = "none";
      statusEl.hidden = true;
      return;
    }
  } catch (_) {}

  // Jangan pernah memunculkan 'Waiting for video…' atau 'Connecting to stream…' jika canvas sudah menampilkan frame
  if (hasRenderedFirstFrame && (text.includes("Waiting") || text.includes("Connecting"))) {
    statusEl.textContent = "";
    statusEl.style.display = "none";
    statusEl.hidden = true;
    return;
  }

  statusEl.textContent = text;
  statusEl.className = "mirror-status" + (tone ? ` ${tone}` : "");
  statusEl.style.display = "block";
  statusEl.hidden = false;
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

let cachedCanvasRect = null;
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

  document.body.classList.remove("fit-fill", "fit-contain", "fit-autofit");
  document.body.classList.add(`fit-${fitMode}`);

  canvas.style.position = "";
  canvas.style.top = "";
  canvas.style.left = "";
  if (fitMode === "autofit" || fitMode === "fill") {
    // Tahap 69 & 71: Auto-Fit & Fill ALWAYS fill 100vw and 100vh (0px bottom black bar)
    canvas.style.width = "100vw";
    canvas.style.height = "100vh";
    canvas.style.maxWidth = "100vw";
    canvas.style.maxHeight = "100vh";
    canvas.style.objectFit = "fill";
  } else {
    // fitMode === "contain" (Original Ratio)
    canvas.style.width = "100%";
    canvas.style.height = "100%";
    canvas.style.objectFit = "contain";
  }
  cachedCanvasRect = canvas.getBoundingClientRect();
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
  const rect = cachedCanvasRect || canvas.getBoundingClientRect();
  const nx = Math.max(0, Math.min(1, (e.clientX - rect.left) / (rect.width || 1)));
  const ny = Math.max(0, Math.min(1, (e.clientY - rect.top) / (rect.height || 1)));

  let iosX = nx;
  let iosY = ny;

  const rot = ((currentRotation % 360) + 360) % 360;
  if (rot === 270) {
    // 270° CW (= 90° CCW): Landscape standar game eFootball (Dynamic Island di kiri)
    iosX = 1.0 - ny;
    iosY = nx;
  } else if (rot === 90) {
    // 90° CW: Landscape terbalik (Dynamic Island di kanan)
    iosX = ny;
    iosY = 1.0 - nx;
  } else if (rot === 180) {
    // 180°: Portrait terbalik
    iosX = 1.0 - nx;
    iosY = 1.0 - ny;
  } else {
    // 0°: Portrait normal
    iosX = nx;
    iosY = ny;
  }

  // Tahap 72: Adjust touch coordinates if native H.265 padding trim is active
  if (window._lastCleanTransform && window._lastCleanTransform.rawW && window._lastCleanTransform.rawH) {
    const t = window._lastCleanTransform;
    iosX = (t.srcTrimLeft + iosX * t.cleanW) / t.rawW;
    iosY = (t.srcTrimTop + iosY * t.cleanH) / t.rawH;
    iosX = Math.max(0, Math.min(1, iosX));
    iosY = Math.max(0, Math.min(1, iosY));
  }

  return {
    x: Math.max(0, Math.min(65535, Math.round(iosX * 65535))),
    y: Math.max(0, Math.min(65535, Math.round(iosY * 65535))),
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

// Tahap 110: Smart Motion & Trackpad PLI Keyframe Settler
let _lastSmartPliTime = 0;
let _motionSettlePliTimer = null;

function requestSmartMotionPli(reason) {
  const now = performance.now();
  if (now - _lastSmartPliTime >= 550) {
    _lastSmartPliTime = now;
    fetch("/mirror/pli", { method: "POST" }).catch(() => {});
  }
  if (_motionSettlePliTimer) {
    clearTimeout(_motionSettlePliTimer);
  }
  _motionSettlePliTimer = setTimeout(() => {
    _motionSettlePliTimer = null;
    _lastSmartPliTime = performance.now();
    fetch("/mirror/pli", { method: "POST" }).catch(() => {});
  }, 180);
}
window.requestSmartMotionPli = requestSmartMotionPli;

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
  if (typeof window.clearOcrSessionMemory === "function") {
    window.clearOcrSessionMemory();
  }
  for (let i = 0; i < times; i++) {
    if (i) await new Promise((r) => setTimeout(r, 120));
    await postJson("/button", { name: "home", state: "press" });
  }
}

async function triggerBackGesture() {
  if (typeof window.clearOcrSessionMemory === "function") {
    window.clearOcrSessionMemory();
  }
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
  if (typeof endDragSelectionSnapshot === "function") endDragSelectionSnapshot();
  if (activePointer === null) return;
  activePointer = null;
  if (typeof requestSmartMotionPli === "function") {
    requestSmartMotionPli("pointerup");
  }
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

let _pointerDownTs = 0;
let _pointerDownPos = null;

// Listen on the whole document so a drag can start on the black margin just
// below the picture; coordinates clamp onto the screen edge.
document.addEventListener("pointerdown", (e) => {
  if (e.button === 3) {
    e.preventDefault();
    triggerBackGesture();
    return;
  }
  if (e.button !== 0) return;
  _pointerDownTs = Date.now();
  canvas.focus({ preventScroll: true });
  try {
    document.body.setPointerCapture(e.pointerId);
  } catch (_) {}
  activePointer = e.pointerId;
  lastCoords = snapToEdge(touchCoords(e));
  _pointerDownPos = lastCoords;
  // Tahap 34: Reset OCR memory when tapping Back button (< in Notes / Keep at X < 0.22 * W && Y < 0.15 * H)
  if (lastCoords.x < 14500 && lastCoords.y < 10000) {
    if (typeof window.clearOcrSessionMemory === "function") {
      window.clearOcrSessionMemory();
    }
  }
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
  if (e.pointerType !== "mouse" || (e.buttons & 1) === 1) {
    // Tahap 94: Only record selection snapshot if user held in place > 350ms before moving (text block selection)
    if (_pointerDownTs > 0 && (Date.now() - _pointerDownTs) > 350) {
      if (typeof sampleViewportIfChanged === "function") {
        sampleViewportIfChanged(false, 2500);
      }
    }
  }
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
  _pointerDownTs = 0;
  _pointerDownPos = null;
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

// Precision Touchpad (2-finger scroll / swipe) -> Instant Non-blocking & Leading-edge
let isWheelSending = false;
let pendingWheelY = 0;
let lastHorizontalSwipeTime = 0;
let accWheelX = 0;
let hSwipeTimer = null;
let trackpadSpeedMultiplier = 1.0;
try {
  const savedSpeed = parseInt(localStorage.getItem("pane-trackpad-speed"), 10);
  if (!isNaN(savedSpeed) && savedSpeed >= 30 && savedSpeed <= 200) {
    trackpadSpeedMultiplier = savedSpeed / 100;
  }
} catch (_) {}

async function performFastHorizontalSwipe(direction) {
  const yMid = 32768;
  const isLeft = direction === "left";
  const xStart = isLeft ? 55000 : 10535;
  const xEnd = isLeft ? 10535 : 55000;
  const steps = 7;
  const dt = 8;

  try {
    await postJson("/touch", { type: "contact", x: xStart, y: yMid });
    for (let i = 1; i <= steps; i++) {
      const t = i / steps;
      const curX = Math.round(xStart + (xEnd - xStart) * t);
      await new Promise((r) => setTimeout(r, dt));
      await postJson("/touch", { type: "contact", x: curX, y: yMid });
    }
    await postJson("/touch", { type: "release", x: xEnd, y: yMid });
  } catch (_) {}
}

async function pumpVerticalWheel() {
  if (isWheelSending) return;
  if (Math.abs(pendingWheelY) < 18) return;

  isWheelSending = true;
  try {
    const curDeltaY = pendingWheelY;
    pendingWheelY = 0;

    let startX = 32768;
    let startY = 32768;
    if (lastCoords && lastCoords.x > 5000 && lastCoords.y > 5000) {
      startX = Math.max(10000, Math.min(55535, lastCoords.x));
      startY = Math.max(16000, Math.min(49000, lastCoords.y));
    }

    const baseDistance = Math.max(6000, Math.min(22000, Math.round(Math.abs(curDeltaY) * 75)));
    const distance = Math.max(2000, Math.min(45000, Math.round(baseDistance * trackpadSpeedMultiplier)));
    const dy = curDeltaY > 0 ? -distance : distance;
    const endX = startX;
    const endY = Math.max(2500, Math.min(63000, startY + dy));

    const steps = 5;
    const dt = 8;

    await postJson("/touch", { type: "contact", x: startX, y: startY });
    for (let i = 1; i <= steps; i++) {
      const t = i / steps;
      const curY = Math.round(startY + (endY - startY) * t);
      await new Promise((r) => setTimeout(r, dt));
      await postJson("/touch", { type: "contact", x: endX, y: curY });
    }
    await postJson("/touch", { type: "release", x: endX, y: endY });
  } catch (_) {
  } finally {
    isWheelSending = false;
    if (Math.abs(pendingWheelY) >= 18) {
      pumpVerticalWheel();
    }
  }
}

canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    if (typeof requestSmartMotionPli === "function") {
      requestSmartMotionPli("wheel");
    }
    if (typeof recordScrollSnapshot === "function") {
      recordScrollSnapshot();
    }

    const absX = Math.abs(e.deltaX);
    const absY = Math.abs(e.deltaY);

    // 1. Fast Horizontal Swipe (Leading-Edge, no debounce delay)
    if (absX > absY * 1.3) {
      accWheelX += e.deltaX;
      window.clearTimeout(hSwipeTimer);
      hSwipeTimer = window.setTimeout(() => {
        accWheelX = 0;
      }, 200);

      const now = performance.now();
      if (Math.abs(accWheelX) >= 28 && now - lastHorizontalSwipeTime > 260) {
        const dir = accWheelX > 0 ? "left" : "right";
        accWheelX = 0;
        lastHorizontalSwipeTime = now;
        performFastHorizontalSwipe(dir);
      }
      return;
    }

    // 2. Coalesced Non-Blocking Vertical Scroll
    pendingWheelY += e.deltaY;
    if (!isWheelSending && Math.abs(pendingWheelY) >= 18) {
      pumpVerticalWheel();
    }
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
      return;
    }
    if ((e.ctrlKey || e.metaKey) && (e.key === "a" || e.code === "KeyA")) {
      if (typeof recordSelectionSnapshot === "function") {
        recordSelectionSnapshot(true);
      }
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
    fitMode = (d.mode === "contain" || d.mode === "fill") ? d.mode : "autofit";
    try { localStorage.setItem("pane-fit-mode", fitMode); } catch (_) {}
    fitCanvas();
    if (lastFrame) {
      try { drawFrame(lastFrame); } catch (_) {}
    }
    return;
  }
  if (d.type === "set-crop" || d.type === "crop-change" || d.type === "update-crop") {
    applyCropStyles(d.crop);
    if (lastFrame) {
      try { drawFrame(lastFrame); } catch (_) {}
    }
    return;
  }
  if (d.type === "clear-viewport-history") {
    if (typeof window.clearViewportHistory === "function") {
      window.clearViewportHistory();
    } else {
      window._ViewportHistory = [];
      if (window._PinnedTopFrames) window._PinnedTopFrames = [];
    }
    return;
  }
  if (d.type === "set-trackpad-speed") {
    const s = parseInt(d.speed, 10);
    if (!isNaN(s) && s >= 30 && s <= 200) {
      trackpadSpeedMultiplier = s / 100;
      try { localStorage.setItem("pane-trackpad-speed", String(s)); } catch (_) {}
    }
    return;
  }
  if (d.type === "set-keyboard-settings") {
    if (d.settings) {
      window.HandsetKeyboardSettings = { ...window.HandsetKeyboardSettings, ...d.settings };
    }
    return;
  }
  if (d.type === "set-orientation") {
    const targetDeg = d.landscape ? (d.rotation !== undefined ? d.rotation : 270) : 0;
    setRotation(targetDeg);
    if (d.manualOverride) {
      _autoRotatedToLandscape = Boolean(d.landscape);
      _landscapeVoteCount = 0;
      _portraitVoteCount = 0;
      manualRotateCooldownUntil = Date.now() + 30000;
    }
    return;
  }
  if (d.type === "user-nav-home-clicked") {
    _lastHomeOrRecentNavTs = Date.now();
    manualRotateCooldownUntil = 0;
    _homeExitGraceUntil = Date.now() + 4000;
    _landscapeVoteCount = 0;
    _portraitVoteCount = 0;
    if (d.autoRotate || autoRotateEnabled) {
      _autoRotatedToLandscape = false;
      setRotation(0);
      try {
        window.parent.postMessage({ type: "auto-rotate-request", landscape: false, rotation: 0 }, "*");
      } catch (_) {}
    }
    return;
  }
  if (d.type === "user-nav-recent-clicked" || d.type === "user-nav-home-or-recent") {
    _lastHomeOrRecentNavTs = Date.now();
    manualRotateCooldownUntil = 0;
    _homeExitGraceUntil = Date.now() + 4000;
    _landscapeVoteCount = 0;
    _portraitVoteCount = 0;
    if (d.autoRotate || autoRotateEnabled) {
      _autoRotatedToLandscape = false;
      setRotation(0);
      try {
        window.parent.postMessage({ type: "auto-rotate-request", landscape: false, rotation: 0 }, "*");
      } catch (_) {}
    }
    return;
  }
  if (d.type === "rotate-landscape") {
    setRotation(d.landscape ? 270 : 0);
    return;
  }
  if (d.type === "set-rotation") {
    setRotation(d.rotation !== undefined ? d.rotation : 0);
    return;
  }
  if (d.type === "rotate-step") {
    setRotation(currentRotation + (d.delta || 0));
    return;
  }
  if (d.type === "toggle-landscape") {
    toggleLandscapeMode();
    return;
  }

  if (e.origin !== window.location.origin) return;
  if (d.source !== "handset") return;
  if (d.cmd === "back") {
    triggerBackGesture();
  } else if (d.cmd === "rotate") {
    setRotation(currentRotation + (d.deg || 0));
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
      if (info.codec === "jpeg_stream" || info.codec === "mjpeg" || info.mode === "image") {
        return {
          codec: (info.codec || "jpeg_stream").trim(),
          description: null,
          isImageStream: true,
          width: info.width || 1179,
          height: info.height || 2556,
        };
      }
      if (!info.codec || !info.description) {
        lastErr = "waiting for video parameters";
        continue;
      }
      const description = Uint8Array.from(atob(info.description), (c) => c.charCodeAt(0));
      return { codec: info.codec.trim(), description, isImageStream: false };
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
  if (!f) return;
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
      : (sourceFrame.displayWidth || sourceFrame.codedWidth || sourceFrame.width || 1216);
    const rawH = (sourceFrame.codedHeight && sourceFrame.codedHeight > sourceFrame.displayHeight)
      ? sourceFrame.codedHeight
      : (sourceFrame.displayHeight || sourceFrame.codedHeight || sourceFrame.height || 2656);

    const sideways = Math.abs(currentRotation % 180) === 90;
    const viewW = sideways ? rawH : rawW;
    const viewH = sideways ? rawW : rawH;

    const dimsChanged = viewW !== canvas.width || viewH !== canvas.height;
    if (dimsChanged) {
      canvas.width = viewW;
      canvas.height = viewH;
      fitCanvas();
    }

    if (!hasRenderedFirstFrame) {
      hasRenderedFirstFrame = true;
      setStatus("");
      try {
        window.parent.postMessage({ type: "video-dimensions", width: rawW, height: rawH }, "*");
        window.parent.postMessage({ type: "mirror-ready" }, "*");
      } catch (_) {}
    }

    const cw = canvas.width;
    const ch = canvas.height;

    // Tahap 72: Native H.265 Macroblock Padding Trim on raw sourceFrame coordinates
    // rawW = 1216, rawH = 2656 on iPhone 17 Pro
    const scaleH = rawH / 2656;
    const scaleW = rawW / 1216;

    // Tahap 74: Set nativeTrimTop = 0 & nativeTrimLeft = 0 agar 0px terpotong di atas Dynamic Island & jam status bar
    const nativeTrimTop = 0;
    const nativeTrimBot = (fitMode === "contain") ? 0 : Math.round(58 * scaleH);
    const nativeTrimLeft = 0;
    const nativeTrimRight = (fitMode === "contain") ? 0 : Math.round(14 * scaleW);

    // Translate manual UI crop sliders (relative to user view) to sourceFrame axes
    const pxScale = Math.max(3, Math.round(rawW / 346));
    const uTop = Math.max(0, (Number(currentCrop.top) || 0) * pxScale);
    const uBot = Math.max(0, (Number(currentCrop.bottom) || 0) * pxScale);
    const uLeft = Math.max(0, (Number(currentCrop.left) || 0) * pxScale);
    const uRight = Math.max(0, (Number(currentCrop.right) || 0) * pxScale);

    let manualTrimTop = uTop, manualTrimBot = uBot, manualTrimLeft = uLeft, manualTrimRight = uRight;
    const rotNorm = ((currentRotation % 360) + 360) % 360;
    if (rotNorm === 270) {
      manualTrimTop = uLeft;
      manualTrimBot = uRight;
      manualTrimLeft = uBot;
      manualTrimRight = uTop;
    } else if (rotNorm === 90) {
      manualTrimTop = uRight;
      manualTrimBot = uLeft;
      manualTrimLeft = uTop;
      manualTrimRight = uBot;
    } else if (rotNorm === 180) {
      manualTrimTop = uBot;
      manualTrimBot = uTop;
      manualTrimLeft = uRight;
      manualTrimRight = uLeft;
    }

    const srcTrimTop = nativeTrimTop + manualTrimTop;
    const srcTrimBot = nativeTrimBot + manualTrimBot;
    const srcTrimLeft = nativeTrimLeft + manualTrimLeft;
    const srcTrimRight = nativeTrimRight + manualTrimRight;

    const cleanW = Math.max(64, rawW - srcTrimLeft - srcTrimRight);
    const cleanH = Math.max(64, rawH - srcTrimTop - srcTrimBot);
    const cleanCenterX = srcTrimLeft + (cleanW / 2);
    const cleanCenterY = srcTrimTop + (cleanH / 2);

    window._lastCleanTransform = { cleanW, cleanH, cleanCenterX, cleanCenterY, srcTrimLeft, srcTrimTop, rawW, rawH };

    const isRot90or270 = (rotNorm === 90 || rotNorm === 270);
    const effW = isRot90or270 ? cleanH : cleanW;
    const effH = isRot90or270 ? cleanW : cleanH;
    const scaleX = cw / effW;
    const scaleY = ch / effH;

    ctx.save();
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.translate(cw / 2, ch / 2);
    ctx.scale(scaleX, scaleY);
    if (rotNorm !== 0) {
      ctx.rotate((rotNorm * Math.PI) / 180);
    }
    // Draw sourceFrame so that (cleanCenterX, cleanCenterY) aligns precisely at (0, 0)
    ctx.drawImage(sourceFrame, -cleanCenterX, -cleanCenterY, rawW, rawH);
    ctx.restore();

    if (autoRotateGameEnabled) {
      checkAutoRotateOrientation(sourceFrame, rawW, rawH);
    }

    if (frameCount === 1 || frameCount === 15 || frameCount === 60) {
      sendMirrorMetrics(sourceFrame, frameCount);
    }
  } finally {
    if (clonedFrame) {
      try {
        clonedFrame.close();
      } catch (_) {}
    }
    if (!_isFlushingPendingFrame && sourceFrame && typeof sourceFrame.close === "function") {
      try {
        sourceFrame.close();
      } catch (_) {}
    }
  }
}
window.drawFrame = drawFrame;

window.captureFullResFrameDataUrl = function () {
  try {
    if (canvas && ((canvas.width >= 600 && canvas.height >= 1000) || (canvas.width >= 1000 && canvas.height >= 600))) {
      return canvas.toDataURL("image/png");
    }
    if (lastFrame) {
      const rawW = (lastFrame.codedWidth && lastFrame.codedHeight > lastFrame.displayHeight)
        ? lastFrame.codedWidth
        : (lastFrame.displayWidth || lastFrame.codedWidth || 1216);
      const rawH = (lastFrame.codedHeight && lastFrame.codedHeight > lastFrame.displayHeight)
        ? lastFrame.codedHeight
        : (lastFrame.displayHeight || lastFrame.codedHeight || 2656);
      const off = document.createElement("canvas");
      off.width = rawW;
      off.height = rawH;
      const offCtx = off.getContext("2d");
      offCtx.drawImage(lastFrame, 0, 0, rawW, rawH);
      return off.toDataURL("image/png");
    }
    if (canvas && canvas.width > 0 && canvas.height > 0) {
      return canvas.toDataURL("image/png");
    }
  } catch (err) {
    console.warn("captureFullResFrameDataUrl error:", err);
  }
  return null;
};

let _historyOffscreenCanvas = null;
let _historyOffscreenCtx = null;

// Tahap 56 & 94: 1/4 size (304x664) JPEG encoder (0.85) for history snapshots (16x RAM & CPU reduction)
window.captureHistoryFrameDataUrl = function () {
  try {
    if (canvas && canvas.width > 0 && canvas.height > 0) {
      if (!_historyOffscreenCanvas) {
        _historyOffscreenCanvas = document.createElement("canvas");
        _historyOffscreenCanvas.width = 304;
        _historyOffscreenCanvas.height = 664;
        _historyOffscreenCtx = _historyOffscreenCanvas.getContext("2d", { willReadFrequently: true });
      }
      _historyOffscreenCtx.drawImage(canvas, 0, 0, 304, 664);
      return _historyOffscreenCanvas.toDataURL("image/jpeg", 0.85);
    }
    return null;
  } catch (err) {
    console.warn("captureHistoryFrameDataUrl error:", err);
    return null;
  }
};

// --- Event-Driven Visual-Change Viewport History for Long Note Stitching (Tahap 30 & Tahap 56) ---
const MAX_VIEWPORT_HISTORY = 4;
window.MAX_VIEWPORT_HISTORY = MAX_VIEWPORT_HISTORY;
window._ViewportHistory = []; // Capacity 4 snapshots (Tahap 94), expires 45s

window.clearViewportHistory = function() {
  window._ViewportHistory = [];
  if (window._PinnedTopFrames) window._PinnedTopFrames = [];
  fetch("/api/ocr/clear-chunks", { method: "POST" }).catch(() => {});
};
window.clearOcrSessionMemory = window.clearViewportHistory;
let _lastEventSampleTs = 0;

function computeViewportSignature() {
  try {
    if (!canvas || canvas.width <= 0 || canvas.height <= 0) return null;
    const W = canvas.width;
    const H = canvas.height;
    const sig = [];
    const yFractions = [0.25, 0.40, 0.55, 0.70];
    const xFractions = [0.25, 0.40, 0.55, 0.70];

    for (let r = 0; r < 4; r++) {
      const py = Math.floor(H * yFractions[r]);
      for (let c = 0; c < 4; c++) {
        const px = Math.floor(W * xFractions[c]);
        const pix = ctx.getImageData(px, py, 1, 1).data;
        const lum = 0.299 * pix[0] + 0.587 * pix[1] + 0.114 * pix[2];
        sig.push(lum);
      }
    }
    return sig;
  } catch (err) {
    return null;
  }
}

function getSignatureDelta(sig1, sig2) {
  if (!sig1 || !sig2 || sig1.length !== sig2.length || sig1.length === 0) return 999;
  let totalDiff = 0;
  for (let i = 0; i < sig1.length; i++) {
    totalDiff += Math.abs(sig1[i] - sig2[i]);
  }
  return totalDiff / sig1.length;
}

function sampleViewportIfChanged(force = false, minInterval = 0) {
  try {
    const now = Date.now();
    const effectiveInterval = force ? 0 : Math.max(minInterval || 0, 2500);
    if (!force && effectiveInterval > 0 && (now - _lastEventSampleTs) < effectiveInterval) return;
    _lastEventSampleTs = now;

    if (frameCount === 0) return;
    const captureHistoryFn = typeof window.captureHistoryFrameDataUrl === "function"
      ? window.captureHistoryFrameDataUrl
      : window.captureFullResFrameDataUrl;
    if (typeof captureHistoryFn !== "function") return;

    // Prune history older than 45s (TTL 45 seconds)
    if (window._ViewportHistory.length > 0) {
      window._ViewportHistory = window._ViewportHistory.filter(item => (now - item.ts) <= 45000);
    }

    const sig = computeViewportSignature();
    if (!sig) return;

    if (window._ViewportHistory.length === 0) {
      const dataUrl = captureHistoryFn();
      if (dataUrl) {
        window._ViewportHistory.push({ ts: now, sig, dataUrl });
      }
      return;
    }

    // Global comparison against ALL items in _ViewportHistory
    let minDiff = 999999;
    let closestIndex = -1;
    for (let i = 0; i < window._ViewportHistory.length; i++) {
      const d = getSignatureDelta(sig, window._ViewportHistory[i].sig);
      if (d < minDiff) {
        minDiff = d;
        closestIndex = i;
      }
    }

    // If minDiff < 7.5 (already recorded viewport position), refresh slot timestamp & dataUrl without pushing a new slot!
    if (minDiff < 7.5 && closestIndex >= 0) {
      const dataUrl = captureHistoryFn();
      if (dataUrl) {
        window._ViewportHistory[closestIndex].ts = now;
        window._ViewportHistory[closestIndex].sig = sig;
        window._ViewportHistory[closestIndex].dataUrl = dataUrl;
      }
    } else {
      // Truly new scroll position: push new slot (capped strictly at MAX_VIEWPORT_HISTORY = 6)
      const dataUrl = captureHistoryFn();
      if (dataUrl) {
        window._ViewportHistory.push({ ts: now, sig, dataUrl });
        while (window._ViewportHistory.length > MAX_VIEWPORT_HISTORY) {
          window._ViewportHistory.shift();
        }
      }
    }
  } catch (err) {
    console.warn("sampleViewportIfChanged error:", err);
  }
}
window.sampleViewportIfChanged = sampleViewportIfChanged;

// Tahap 56 Zero-Stutter Audio & GPU Fix:
// Passive 900ms setInterval REMOVED so watching TikTok/video never triggers background canvas.toDataURL() stalls!
// Snapshots are captured strictly on user interaction (wheel, pointerdown, active drag, Ctrl+A, Alt+C).

// Event hooks (anti-reset: never wipes previous history on pointerdown; throttled 700ms during scroll/drag)
window.recordScrollSnapshot = function () {
  // Tahap 94: No-op during normal wheel scroll to eliminate synchronous canvas.toDataURL() stalls!
};

window.startDragSelectionSnapshot = function () {
  sampleViewportIfChanged(false, 0);
};

window.updateDragSelectionSnapshot = function () {
  sampleViewportIfChanged(false, 700);
};

window.endDragSelectionSnapshot = function () {};

window.recordSelectionSnapshot = function (force = false) {
  sampleViewportIfChanged(force, force ? 0 : 700);
};

window.getRecentSelectionFrames = function () {
  try {
    const now = Date.now();
    // Guarantee latest viewport is checked on explicit copy request
    sampleViewportIfChanged(true, 0);
    // Current live frame is ALWAYS captured as 100% lossless PNG for maximum OCR sharpness
    const liveLosslessPng = typeof window.captureFullResFrameDataUrl === "function"
      ? window.captureFullResFrameDataUrl()
      : null;

    const validHistory = (window._ViewportHistory || []).filter(item => (now - item.ts) <= 45000);
    if (validHistory.length === 0) {
      return liveLosslessPng ? [liveLosslessPng] : [];
    }

    if (validHistory.length <= 4) {
      const frames = validHistory.map(item => item.dataUrl);
      if (liveLosslessPng && frames.length > 0) {
        frames[frames.length - 1] = liveLosslessPng;
      }
      return frames;
    }

    // Select up to 4 frames (max 3 history + 1 current), ordered chronologically (top -> mid -> bottom)
    const n = validHistory.length;
    const candidateIndices = Array.from(new Set([
      0,
      Math.floor(n * 0.33),
      Math.floor(n * 0.67),
      n - 1
    ])).sort((a, b) => a - b);

    const chosen = [];
    for (const idx of candidateIndices) {
      const item = validHistory[idx];
      if (!item || !item.dataUrl) continue;
      if (chosen.length === 0 || getSignatureDelta(item.sig, chosen[chosen.length - 1].sig) >= 3.0) {
        chosen.push(item);
      }
    }

    // Ensure current (last) frame is ALWAYS included
    const lastItem = validHistory[n - 1];
    if (lastItem && !chosen.includes(lastItem)) {
      if (chosen.length >= 4) chosen.pop();
      chosen.push(lastItem);
    }

    // If fewer than 4 frames due to high similarity threshold, backfill chronological frames
    if (chosen.length < Math.min(4, validHistory.length)) {
      for (let i = 0; i < validHistory.length; i++) {
        const item = validHistory[i];
        if (!chosen.includes(item)) {
          chosen.push(item);
          if (chosen.length >= 4) break;
        }
      }
      chosen.sort((a, b) => a.ts - b.ts);
    }

    const resultFrames = chosen.map(item => item.dataUrl);
    if (liveLosslessPng && resultFrames.length > 0) {
      resultFrames[resultFrames.length - 1] = liveLosslessPng;
    }
    return resultFrames;
  } catch (err) {
    console.warn("getRecentSelectionFrames error:", err);
    return [];
  }
};

let _isFlushingPendingFrame = false;

function drawPending() {
  rafScheduled = false;
  const f = pendingFrame;
  pendingFrame = null;
  if (!f) return;

  const rawW = (f.codedWidth && f.codedHeight > f.displayHeight) ? f.codedWidth : (f.displayWidth || f.width || 1179);
  const rawH = (f.codedHeight && f.codedHeight > f.displayHeight) ? f.codedHeight : (f.displayHeight || f.height || 2556);
  const landscape = rawW > rawH;

  if (lastFrameLandscape !== null && landscape !== lastFrameLandscape && visualRotation !== 0) {
    visualRotation = 0;
  }
  lastFrameLandscape = landscape;

  _isFlushingPendingFrame = true;
  try {
    drawFrame(f);
    if (!hasRenderedFirstFrame) {
      hasRenderedFirstFrame = true;
      setStatus("");
      fitCanvas();
      setTimeout(fitCanvas, 50);
      setTimeout(fitCanvas, 200);
    }
  } catch (err) {
    console.error("drawFrame error:", err);
  } finally {
    _isFlushingPendingFrame = false;
    try {
      if (typeof f.close === "function") {
        f.close();
      }
    } catch (_) {}
  }
}
window.drawPending = drawPending;

async function runImageStream(generation, codecInfo) {
  setStatus("Connecting to image stream…");
  let resp = null;
  const streamDelays = [200, 400, 800, 1500, 2500, 4000];
  for (let a = 0; a <= streamDelays.length; a++) {
    if (generation !== runGeneration) return;
    if (a > 0) {
      await new Promise((r) => setTimeout(r, streamDelays[a - 1]));
    }
    try {
      const r = await fetch(`${MIRROR}/live`, { cache: "no-store" });
      if (r.ok) {
        resp = r;
        break;
      }
    } catch (_) {}
  }
  if (!resp || generation !== runGeneration) {
    setStatus("Cannot reach the image stream. Press Restart screen.", "warn");
    return;
  }

  const reader = resp.body.getReader();
  abortStream = () => reader.cancel().catch(() => {});
  let buf = new Uint8Array(0);

  try {
    while (generation === runGeneration) {
      const { value, done } = await reader.read();
      if (done) break;
      const merged = new Uint8Array(buf.length + value.length);
      merged.set(buf);
      merged.set(value, buf.length);
      buf = merged;

      while (buf.length >= 4) {
        const len = (buf[0] << 24) | (buf[1] << 16) | (buf[2] << 8) | buf[3];
        if (len <= 0 || len > 10000000) {
          buf = new Uint8Array(0);
          break;
        }
        if (buf.length < 4 + len) break;
        const imgBytes = buf.slice(4, 4 + len);
        buf = buf.subarray(4 + len);

        try {
          const blob = new Blob([imgBytes], { type: "image/png" });
          const bmp = await createImageBitmap(blob);
          if (generation !== runGeneration) {
            bmp.close();
            return;
          }
          if (pendingFrame) {
            try { pendingFrame.close(); } catch (_) {}
          }
          pendingFrame = bmp;
          if (!rafScheduled) {
            rafScheduled = true;
            requestAnimationFrame(drawPending);
          }
          frameCount += 1;
        } catch (decodeErr) {
          console.warn("createImageBitmap error:", decodeErr);
        }
      }
    }
  } catch (err) {
    console.warn("runImageStream loop ended:", err);
  } finally {
    try { reader.cancel(); } catch (_) {}
  }
  if (generation === runGeneration) {
    setStatus("Stream ended — press Restart screen", "warn");
  }
}

async function run(generation) {
  if (!hasRenderedFirstFrame) {
    setStatus("Connecting to stream…");
  } else {
    setStatus("");
  }
  frameCount = 0;
  const startedAt = performance.now();
  const watchdog = window.setInterval(() => {
    if (generation !== runGeneration) {
      window.clearInterval(watchdog);
      return;
    }
    if (frameCount > 0 || hasRenderedFirstFrame) {
      window.clearInterval(watchdog);
      return;
    }
    if (performance.now() - startedAt > 12000) {
      window.clearInterval(watchdog);
      if (!hasRenderedFirstFrame) {
        setStatus("No picture yet — press Restart screen on the right", "warn");
      }
    }
  }, 1000);

  const codecInfo = await fetchCodecWithRetry();
  if (generation !== runGeneration) return;

  if (codecInfo.isImageStream) {
    window.clearInterval(watchdog);
    await runImageStream(generation, codecInfo);
    return;
  }

  const { codec, description } = codecInfo;

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
    hardwareAcceleration: "prefer-hardware",
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
  if (!hasRenderedFirstFrame) {
    setStatus("Waiting for video…");
  } else {
    setStatus("");
  }

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
  streamActive = true;
  let buf = new Uint8Array(0);
  let timestamp = 0;
  let gotKey = false;

  try {
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
  } finally {
    streamActive = false;
    try { reader.cancel(); } catch (_) {}
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

window.addEventListener("message", (e) => {
  if (!e.data || typeof e.data !== "object") return;
  if (e.data.type === "device-locked") {
    window._isDeviceLocked = true;
    setStatus("");
  } else if (e.data.type === "wake-stream-pli") {
    window._isDeviceLocked = false;
    setStatus("");
    // Request instant keyframe from iOS encoder without breaking stream
    fetch(`${MIRROR}/pli`, { method: "POST", cache: "no-store" }).catch(() => {});
    setTimeout(() => {
      fetch(`${MIRROR}/pli`, { method: "POST", cache: "no-store" }).catch(() => {});
    }, 300);
    if (!streamActive) {
      beginRun();
    }
  } else if (e.data.type === "set-auto-rotate") {
    autoRotateGameEnabled = Boolean(e.data.enabled);
    _lastAutoRotateSampleTs = 0;
    _landscapeVoteCount = 0;
    _portraitVoteCount = 0;
    if (autoRotateGameEnabled && lastFrame) {
      setTimeout(() => {
        try {
          checkAutoRotateOrientation(lastFrame, lastFrame.codedWidth || 1216, lastFrame.codedHeight || 2656);
        } catch (_) {}
      }, 50);
    }
  } else if (e.data.type === "refresh-stream") {
    if (streamActive && hasRenderedFirstFrame) {
      setStatus("");
      fetch(`${MIRROR}/pli`, { method: "POST", cache: "no-store" }).catch(() => {});
      setTimeout(() => {
        fetch(`${MIRROR}/pli`, { method: "POST", cache: "no-store" }).catch(() => {});
      }, 300);
    } else {
      beginRun();
    }
  }
});

// Tahap 70: Emit mirror-ready on load
try {
  if (window.parent && window.parent !== window) {
    window.parent.postMessage({ type: "mirror-ready" }, "*");
  }
} catch (_) {}

// Tahap 77 & 78: Expose cooldown, grace and nav getters for testing
window.manualRotateCooldownUntil = () => manualRotateCooldownUntil;
window._getManualRotateCooldownUntil = () => manualRotateCooldownUntil;
window._getLastHomeOrRecentNavTs = () => _lastHomeOrRecentNavTs;
window._getHomeExitGraceUntil = () => _homeExitGraceUntil;
window._triggerRecentGrace = () => {
  _lastHomeOrRecentNavTs = Date.now();
  _homeExitGraceUntil = Date.now() + 4000;
  _landscapeVoteCount = 0;
  _portraitVoteCount = 0;
};
