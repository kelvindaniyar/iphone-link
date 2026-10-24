param(
    [string]$ImagePath = "",
    [string]$ImagePaths = ""
)

$ErrorActionPreference = "SilentlyContinue"

try {
    Add-Type -AssemblyName 'System.Runtime.WindowsRuntime'
    Add-Type -AssemblyName 'System.Drawing'
    Add-Type -ReferencedAssemblies 'System.Drawing' -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public class DualOcrPreprocessor {
    public static void PrepareCandidates(
        Bitmap src,
        bool isFullViewport,
        bool onlyCandidate1,
        out Bitmap cand1Natural,
        out Bitmap cand2RedFiltered,
        out bool foundHighlight,
        out int cropX,
        out int cropY,
        out int cropH,
        out int selMinY,
        out int selMaxY,
        out string selectionColor
    ) {
        int W = src.Width;
        int H = src.Height;
        selMinY = 0;
        selMaxY = 0;
        selectionColor = "none";
        int yellowHlCount = 0;
        int blueHlCount = 0;

        BitmapData srcData = src.LockBits(new Rectangle(0, 0, W, H), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        int stride = srcData.Stride;
        byte[] raw = new byte[Math.Abs(stride) * H];
        Marshal.Copy(srcData.Scan0, raw, 0, raw.Length);
        src.UnlockBits(srcData);

        // 1. Detect and Mask AssistiveTouch floating button in Left Zone (X < 0.20 * W) or Right Zone (X > 0.74 * W)
        for (int side = 0; side < 2; side++) {
            int zoneXStart = (side == 0) ? 0 : (int)(W * 0.74);
            int zoneXEnd = (side == 0) ? (int)(W * 0.20) : W;

            int clusterMinY = -1;
            int clusterMaxY = -1;
            int clusterMinX = 999999;
            int clusterMaxX = 0;
            int clusterRowCount = 0;

            for (int y = (int)(H * 0.10); y <= (int)(H * 0.85); y++) {
                int rowOffset = y * stride;
                int curRun = 0;
                int maxRun = 0;
                int runStartX = 0;
                int bestStartX = 0;

                for (int x = zoneXStart; x < zoneXEnd; x++) {
                    int idx = rowOffset + (x * 4);
                    byte b = raw[idx];
                    byte g = raw[idx + 1];
                    byte r = raw[idx + 2];

                    bool isNeutralGray = (Math.Abs(r - g) <= 14) && (Math.Abs(g - b) <= 14) && (Math.Abs(r - b) <= 14) && (r >= 48 && r <= 195);
                    if (isNeutralGray) {
                        if (curRun == 0) runStartX = x;
                        curRun++;
                        if (curRun > maxRun) {
                            maxRun = curRun;
                            bestStartX = runStartX;
                        }
                    } else {
                        curRun = 0;
                    }
                }

                if (maxRun >= 25) {
                    if (clusterMinY == -1 || y <= (clusterMaxY + 2)) {
                        if (clusterMinY == -1) clusterMinY = y;
                        clusterMaxY = y;
                        clusterRowCount++;
                        if (bestStartX < clusterMinX) clusterMinX = bestStartX;
                        if ((bestStartX + maxRun) > clusterMaxX) clusterMaxX = bestStartX + maxRun;
                    } else {
                        int cW = clusterMaxX - clusterMinX;
                        int cH = clusterMaxY - clusterMinY;
                        float aspect = (cH > 0) ? ((float)cW / (float)cH) : 0f;
                        if (cW >= 55 && cH >= 55 && clusterRowCount >= 40 && aspect >= 0.75f && aspect <= 1.35f) {
                            int pad = 10;
                            int mX0 = Math.Max(0, clusterMinX - pad);
                            int mX1 = Math.Min(W - 1, clusterMaxX + pad);
                            int mY0 = Math.Max(0, clusterMinY - pad);
                            int mY1 = Math.Min(H - 1, clusterMaxY + pad);
                            int cCenterX = (clusterMinX + clusterMaxX) / 2;
                            int cCenterY = (clusterMinY + clusterMaxY) / 2;
                            int cRadius = Math.Max(cW, cH) / 2;
                            int coreRadiusSq = (int)((cRadius * 0.55) * (cRadius * 0.55));
                            for (int my = mY0; my <= mY1; my++) {
                                int mOffset = my * stride;
                                int dy = my - cCenterY;
                                for (int mx = mX0; mx <= mX1; mx++) {
                                    int mIdx = mOffset + (mx * 4);
                                    byte b = raw[mIdx];
                                    byte g = raw[mIdx + 1];
                                    byte r = raw[mIdx + 2];
                                    int dx = mx - cCenterX;
                                    bool isCore = (dx * dx + dy * dy) <= coreRadiusSq;
                                    bool isDarkText = (r < 75 && g < 75 && b < 75);
                                    if (isCore || (!isDarkText && (r >= 70 || g >= 70 || b >= 70))) {
                                        raw[mIdx] = 255;
                                        raw[mIdx + 1] = 255;
                                        raw[mIdx + 2] = 255;
                                        raw[mIdx + 3] = 255;
                                    }
                                }
                            }
                        }
                        clusterMinY = y;
                        clusterMaxY = y;
                        clusterRowCount = 1;
                        clusterMinX = bestStartX;
                        clusterMaxX = bestStartX + maxRun;
                    }
                }
            }

            if (clusterMinY != -1) {
                int cW = clusterMaxX - clusterMinX;
                int cH = clusterMaxY - clusterMinY;
                float aspect = (cH > 0) ? ((float)cW / (float)cH) : 0f;
                if (cW >= 55 && cH >= 55 && clusterRowCount >= 40 && aspect >= 0.75f && aspect <= 1.35f) {
                    int pad = 10;
                    int mX0 = Math.Max(0, clusterMinX - pad);
                    int mX1 = Math.Min(W - 1, clusterMaxX + pad);
                    int mY0 = Math.Max(0, clusterMinY - pad);
                    int mY1 = Math.Min(H - 1, clusterMaxY + pad);
                    int cCenterX = (clusterMinX + clusterMaxX) / 2;
                    int cCenterY = (clusterMinY + clusterMaxY) / 2;
                    int cRadius = Math.Max(cW, cH) / 2;
                    int coreRadiusSq = (int)((cRadius * 0.55) * (cRadius * 0.55));
                    for (int my = mY0; my <= mY1; my++) {
                        int mOffset = my * stride;
                        int dy = my - cCenterY;
                        for (int mx = mX0; mx <= mX1; mx++) {
                            int mIdx = mOffset + (mx * 4);
                            byte b = raw[mIdx];
                            byte g = raw[mIdx + 1];
                            byte r = raw[mIdx + 2];
                            int dx = mx - cCenterX;
                            bool isCore = (dx * dx + dy * dy) <= coreRadiusSq;
                            bool isDarkText = (r < 75 && g < 75 && b < 75);
                            if (isCore || (!isDarkText && (r >= 70 || g >= 70 || b >= 70))) {
                                raw[mIdx] = 255;
                                raw[mIdx + 1] = 255;
                                raw[mIdx + 2] = 255;
                                raw[mIdx + 3] = 255;
                            }
                        }
                    }
                }
            }
        }

        // 2. Detect iOS On-Screen Virtual Keyboard in lower screen (Y = 0.56 * H to 0.84 * H)
        bool hasKeyboard = false;
        int kbCheckYStart = (int)(H * 0.56);
        int kbCheckYEnd = (int)(H * 0.84);
        int kbCheckXStart = (int)(W * 0.10);
        int kbCheckXEnd = (int)(W * 0.90);
        int kbGrayCount = 0;
        int totalSampled = 0;

        for (int y = kbCheckYStart; y <= kbCheckYEnd; y += 4) {
            int rOff = y * stride;
            for (int x = kbCheckXStart; x <= kbCheckXEnd; x += 4) {
                int idx = rOff + (x * 4);
                byte b = raw[idx];
                byte g = raw[idx + 1];
                byte r = raw[idx + 2];
                totalSampled++;
                if (r >= 180 && r <= 228 && Math.Abs(r - g) <= 10 && Math.Abs(g - b) <= 10 && Math.Abs(r - b) <= 10) {
                    kbGrayCount++;
                }
            }
        }
        if (totalSampled > 0 && ((float)kbGrayCount / totalSampled) >= 0.12f) {
            hasKeyboard = true;
        }

        // Tahap 40: When no keyboard, extend bottom scan and crop to 0.892 * H
        int maxContentBottom = hasKeyboard ? (int)(H * 0.495) : (int)(H * 0.892);

        // 3. Scan Highlights (Tahap 36/37/38/40: Yellow & Blue Highlights, threshold >= 8 pixels per row)
        int scanTopY = (int)(H * 0.102);
        int scanBottomY = hasKeyboard ? (int)(H * 0.495) : (int)(H * 0.885);
        int leftX = (int)(W * 0.04);
        int rightX = (int)(W * 0.96);

        bool[] rowHasHighlight = new bool[H];
        foundHighlight = false;

        if (!isFullViewport) {
            for (int y = scanTopY; y <= scanBottomY; y++) {
                // Ignore top toolbar zone buttons (< and checkmark): scan X between 0.18*W and 0.66*W if Y < 0.140*H
                int rowLeftX = (y < (int)(H * 0.140)) ? (int)(W * 0.18) : leftX;
                int rowRightX = (y < (int)(H * 0.140)) ? (int)(W * 0.66) : rightX;

                int rowOffset = y * stride;
                int hlCount = 0;

                for (int x = rowLeftX; x <= rowRightX; x++) {
                    int idx = rowOffset + (x * 4);
                    byte b = raw[idx];
                    byte g = raw[idx + 1];
                    byte r = raw[idx + 2];

                    // Apple Notes Yellow Highlight (including pale cream):
                    bool isYellowHl = (r > 215 && g > 195 && b < 240 && (r - b) >= 14);
                    bool isYellowHandle = (r > 195 && g > 125 && b < 100 && (r - b) >= 80);
                    // Google Keep / iOS Blue Selection (Tahap 37):
                    bool isBlueHl = ((b >= 200 && g >= 185 && (b - r) >= 10) || (b >= 215 && g >= 195 && (b - r) >= 12));
                    bool isBlueHandle = (b > 155 && (b - r) >= 55 && (b - g) >= 25);

                    if (isYellowHl || isYellowHandle) {
                        yellowHlCount++;
                        hlCount++;
                    } else if (isBlueHl || isBlueHandle) {
                        blueHlCount++;
                        hlCount++;
                    }
                }

                // Tahap 36/37/40: Threshold >= 8 horizontal pixels per row
                if (hlCount >= 8) {
                    rowHasHighlight[y] = true;
                }
            }
        }

        // Group rows with vertical gap tolerance 0.060 * H
        int maxLineGap = (int)(H * 0.060);
        int firstClusterStart = -1;
        int lastClusterEnd = -1;
        int curStart = -1;
        int lastRow = -1;

        for (int y = scanTopY; y <= scanBottomY; y++) {
            if (rowHasHighlight[y]) {
                if (curStart == -1) {
                    curStart = y;
                } else if (lastRow != -1 && (y - lastRow) > maxLineGap) {
                    int h = lastRow - curStart;
                    if (h >= 12) {
                        if (firstClusterStart == -1) firstClusterStart = curStart;
                        lastClusterEnd = lastRow;
                    }
                    curStart = y;
                }
                lastRow = y;
            }
        }

        if (curStart != -1 && lastRow != -1) {
            int h = lastRow - curStart;
            if (h >= 12) {
                if (firstClusterStart == -1) firstClusterStart = curStart;
                lastClusterEnd = lastRow;
            }
        }

        if (firstClusterStart != -1 && lastClusterEnd != -1 && (lastClusterEnd - firstClusterStart) >= 12) {
            foundHighlight = true;
            selMinY = firstClusterStart;
            selMaxY = lastClusterEnd;
        }

        if (blueHlCount > yellowHlCount && blueHlCount >= 20) {
            selectionColor = "blue";
        } else if (yellowHlCount >= 20) {
            selectionColor = "yellow";
        } else {
            selectionColor = "none";
        }

        // FULL-WIDTH HORIZONTAL STRIP: 0.03*W to 0.97*W
        cropX = Math.Max(0, (int)(W * 0.03));
        int cropW = Math.Min(W - cropX, (int)(W * 0.94));

        if (foundHighlight && !isFullViewport) {
            // Tahap 40: scan down to 0.098*H and add +0.012*H bottom padding so tail descenders (j, g, y) are never cut
            cropY = Math.Max((int)(H * 0.098), selMinY - (int)(H * 0.008));
            int cropBottom = Math.Min(maxContentBottom, selMaxY + (int)(H * 0.012));
            cropH = Math.Max(20, cropBottom - cropY);
        } else {
            // Full-Viewport Content Crop (Tahap 39/40: 0.102 * H to Math.Min(maxContentBottom, 0.810 * H))
            cropY = (int)(H * 0.102);
            int cropBottom = Math.Min(maxContentBottom, (int)(H * 0.810));
            cropH = Math.Max(20, cropBottom - cropY);
        }

        // Candidate 1: Clean Natural RGB (only neutralizes handles and red spellcheck dots)
        Bitmap stripNatural = new Bitmap(cropW, cropH, PixelFormat.Format32bppArgb);
        BitmapData natData = stripNatural.LockBits(new Rectangle(0, 0, cropW, cropH), ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
        byte[] natPixels = new byte[Math.Abs(natData.Stride) * cropH];

        // Candidate 2: Red/Luma Contrast Enhanced (optional for history frames to maximize speed)
        Bitmap stripRed = null;
        BitmapData redData = null;
        byte[] redPixels = null;
        if (!onlyCandidate1) {
            stripRed = new Bitmap(cropW, cropH, PixelFormat.Format32bppArgb);
            redData = stripRed.LockBits(new Rectangle(0, 0, cropW, cropH), ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
            redPixels = new byte[Math.Abs(redData.Stride) * cropH];
        }

        for (int cy = 0; cy < cropH; cy++) {
            int srcRow = (cropY + cy) * stride;
            int natRow = cy * natData.Stride;
            int redRow = (redData != null) ? cy * redData.Stride : 0;
            for (int cx = 0; cx < cropW; cx++) {
                int sIdx = srcRow + ((cropX + cx) * 4);
                int nIdx = natRow + (cx * 4);

                byte b = raw[sIdx];
                byte g = raw[sIdx + 1];
                byte r = raw[sIdx + 2];

                // Tahap 36/37/38/40: Targeted Unwanted Objects Neutralization
                int absY = cropY + cy;
                int absX = cropX + cx;
                bool isTopNavButton = (absY < (int)(H * 0.140)) && (absX < (int)(W * 0.18) || absX > (int)(W * 0.66));
                bool isYellowHandle = (r > 195 && g > 125 && b < 100 && (r - b) >= 80 && !(r < 130 && g < 130 && b < 130));
                bool isBlueHandle = (b > 155 && (b - r) >= 55 && (b - g) >= 25 && !(r < 110 && g < 110 && b < 110));
                bool isRedSpellcheck = (r > 180 && g < 140 && b < 140 && (r - g) > 52 && (r - b) > 52);

                // Kandidat 1 (Natural RGB):
                if (isTopNavButton || isYellowHandle || isBlueHandle || isRedSpellcheck) {
                    natPixels[nIdx] = 255;
                    natPixels[nIdx + 1] = 255;
                    natPixels[nIdx + 2] = 255;
                    natPixels[nIdx + 3] = 255;
                } else {
                    natPixels[nIdx] = b;
                    natPixels[nIdx + 1] = g;
                    natPixels[nIdx + 2] = r;
                    natPixels[nIdx + 3] = 255;
                }

                // Kandidat 2 (Red/Luma Contrast Enhanced):
                if (!onlyCandidate1 && redPixels != null) {
                    int rIdx = redRow + (cx * 4);
                    if (isTopNavButton || isYellowHandle || isBlueHandle || isRedSpellcheck) {
                        redPixels[rIdx] = 255;
                        redPixels[rIdx + 1] = 255;
                        redPixels[rIdx + 2] = 255;
                        redPixels[rIdx + 3] = 255;
                    } else {
                        int lum;
                        int threshold;
                        if (b > (r + 10)) {
                            // Blue selection (Google Keep)
                            lum = (r + g) / 2;
                            threshold = 212;
                        } else {
                            // Yellow highlight or white background
                            lum = r;
                            threshold = 228;
                        }

                        byte v;
                        if (lum >= threshold) {
                            v = 255;
                        } else if (lum < 160) {
                            v = (byte)(lum * 0.55);
                        } else {
                            int mapped = 90 + (int)((lum - 160) * (250 - 90) / (threshold - 160));
                            if (mapped < 0) mapped = 0;
                            if (mapped > 255) mapped = 255;
                            v = (byte)mapped;
                        }
                        redPixels[rIdx] = v;
                        redPixels[rIdx + 1] = v;
                        redPixels[rIdx + 2] = v;
                        redPixels[rIdx + 3] = 255;
                    }
                }
            }
        }

        Marshal.Copy(natPixels, 0, natData.Scan0, natPixels.Length);
        stripNatural.UnlockBits(natData);

        if (!onlyCandidate1 && stripRed != null && redData != null && redPixels != null) {
            Marshal.Copy(redPixels, 0, redData.Scan0, redPixels.Length);
            stripRed.UnlockBits(redData);
        }

        // Dynamic Safe Upscaling (Tahap 37): Max 3500px, NEVER exceed 4096px
        double scale = (cropH <= 650) ? 2.2 : 1.35;
        int margin = 48;
        double maxAllowedDim = 3400.0;
        if (cropW * scale > maxAllowedDim) scale = maxAllowedDim / cropW;
        if (cropH * scale > maxAllowedDim) scale = maxAllowedDim / cropH;
        int targetW = Math.Max(1, (int)Math.Round(cropW * scale));
        int targetH = Math.Max(1, (int)Math.Round(cropH * scale));
        int finalW = targetW + (margin * 2);
        int finalH = targetH + (margin * 2);

        cand1Natural = new Bitmap(finalW, finalH, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(cand1Natural)) {
            g.Clear(Color.White);
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
            g.SmoothingMode = SmoothingMode.HighQuality;
            g.DrawImage(stripNatural, new Rectangle(margin, margin, targetW, targetH), new Rectangle(0, 0, cropW, cropH), GraphicsUnit.Pixel);
        }
        stripNatural.Dispose();

        if (!onlyCandidate1 && stripRed != null) {
            cand2RedFiltered = new Bitmap(finalW, finalH, PixelFormat.Format32bppArgb);
            using (Graphics g = Graphics.FromImage(cand2RedFiltered)) {
                g.Clear(Color.White);
                g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                g.SmoothingMode = SmoothingMode.HighQuality;
                g.DrawImage(stripRed, new Rectangle(margin, margin, targetW, targetH), new Rectangle(0, 0, cropW, cropH), GraphicsUnit.Pixel);
            }
            stripRed.Dispose();
        } else {
            cand2RedFiltered = null;
        }
    }
}
'@

    [void][Windows.Media.Ocr.OcrEngine, Windows.Foundation.Metadata, ContentType = WindowsRuntime]
    [void][Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation.Metadata, ContentType = WindowsRuntime]
    [void][Windows.Storage.StorageFile, Windows.Foundation.Metadata, ContentType = WindowsRuntime]

    $asTaskGeneric = [System.WindowsRuntimeSystemExtensions].GetMethods() | ? { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.IsGenericMethod } | Select-Object -First 1

    function Await-Op($op, [type]$t) {
        $netTask = $asTaskGeneric.MakeGenericMethod($t).Invoke($null, @($op))
        [void]$netTask.Wait(-1)
        return $netTask.Result
    }

    function Get-OcrParsedLines([System.Drawing.Bitmap]$bmp) {
        $tempFile = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), 'ocr_run_' + [System.Guid]::NewGuid().ToString() + '.png')
        $bmp.Save($tempFile, [System.Drawing.Imaging.ImageFormat]::Png)

        $storageFile = Await-Op ([Windows.Storage.StorageFile]::GetFileFromPathAsync($tempFile)) ([Windows.Storage.StorageFile])
        $stream = Await-Op ($storageFile.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
        $decoder = Await-Op ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
        $softwareBitmap = Await-Op ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

        $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
        if ($null -eq $engine) {
            $stream.Dispose()
            Remove-Item $tempFile -Force -ErrorAction SilentlyContinue
            return @()
        }

        $ocrResult = Await-Op ($engine.RecognizeAsync($softwareBitmap)) ([Windows.Media.Ocr.OcrResult])

        $stream.Dispose()
        Remove-Item $tempFile -Force -ErrorAction SilentlyContinue

        $dateRegex = '(?i)^\d{1,2}\s+(January|February|March|April|May|June|July|August|September|October|November|December|Oktober|Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4}(\s+(at|pukul)\s+\d{1,2}[.:]\d{2})?'
        $timeRegex = '^[lu0-9OIl]{1,2}[.:][0-9OIl]{2}\b'
        $calloutRegex = '^(Cut|Copy|Paste|Delete|Select|Select All|Look Up|Translate|Share\.\.\.|No Replacements Found|Device Hub|This device was viewed remotely|Co|py|Pas|te|>\s*|\.\.\.)$'

        # LAPIS 1 (Tahap 33): Detect Notes Folder List View across all raw lines
        $folderListHeaderRegex = '(?i)^(Today|Yesterday|Previous\s+\d+\s+Days|Hari Ini|Kemarin|\d+\s+Hari Sebelumnya|Notes|Catatan|\d+\s+Notes|\d+\s+Catatan|iCloud|January|February|March|April|May|June|July|August|September|October|November|December)$'
        $folderListPreviewRegex = '(?i)^\d{1,2}[.:]\d{2}\s+\S+'

        $ellipsisCount = 0
        $isNotesListView = $false
        foreach ($l in $ocrResult.Lines) {
            $rawT = $l.Text.Trim()
            if (!$rawT) { continue }
            if ($rawT -match $folderListHeaderRegex) {
                $isNotesListView = $true
                break
            }
            if ($rawT -match $folderListPreviewRegex) {
                $isNotesListView = $true
                break
            }
            if ($rawT -match '(\.\.\.|…)$') {
                $ellipsisCount++
                if ($ellipsisCount -ge 2) {
                    $isNotesListView = $true
                    break
                }
            }
        }

        if ($isNotesListView) {
            return @([PSCustomObject]@{ Text = "<<<NOTES_LIST_VIEW>>>"; MinY = 0; MaxY = 0; Height = 0 })
        }

        $parsedLines = @()
        $hasDateHeader = $false
        $dateMinY = -1.0
        foreach ($l in $ocrResult.Lines) {
            $t = $l.Text.Trim()
            if (!$t) { continue }
            if ($t -match $dateRegex) {
                $hasDateHeader = $true
                $minY_d = 9999999
                foreach ($w in $l.Words) {
                    if ($w.BoundingRect.Y -lt $minY_d) { $minY_d = $w.BoundingRect.Y }
                }
                if ($dateMinY -lt 0 -or $minY_d -lt $dateMinY) {
                    $dateMinY = $minY_d
                }
                continue
            }
            if ($t -match $timeRegex) { continue }
            if ($t -match $calloutRegex) { continue }
            if ($t -match '(?i)(jendela-kecil|Device Hub|This device was viewed remotely|remotely from Mac)') { continue }
            if ($t -match '\b(v3one|oil13i1|111e113n|me1a1ui|miliknva|aO/O3124an)\b') { continue }
            if ($t -match '^[<✓>]+$' -or $t -eq 'Notes' -or $t -eq 'Catatan' -or $t -eq 'Done' -or $t -eq 'Selesai') { continue }

            # Tahap 40 (Bug 1): iOS Callout Menu Balloon Filter.
            # DILARANG KERAS menghapus kata Cut, Copy, Paste, Delete dari dalam kalimat berita!
            # Baris HANYA boleh dibuang jika SELURUH baris murni terdiri dari menu iOS atau menu fragments.
            $pureMenuLineRegex = '^[\s|<>[\]()\\/_\-\.]*(?:Cut|Copy|Paste|Delete|Select(?:\s+All)?|AutoFill|Look\s+Up|Translate|Share(?:\.\.\.)?|Proofread|Rewrite|No\s+Replacements\s+Found)(?:[\s|<>[\]()\\/_\-\.]+(?:Cut|Copy|Paste|Delete|Select(?:\s+All)?|AutoFill|Look\s+Up|Translate|Share(?:\.\.\.)?|Proofread|Rewrite|No\s+Replacements\s+Found))*[\s|<>[\]()\\/_\-\.]*$'
            $isCutOffNews = ($t -match '(?i)\b(cut-off|cutoff)\b')

            if (-not $isCutOffNews) {
                if ($t -match $pureMenuLineRegex) { continue }

                # Secondary check for pure menu balloon fragments (e.g. "Cut Co", "Paste Delete")
                if ($t -match '\b(Cut|Copy|Paste|Delete|Select|Look Up|Translate|Share|Replacements)\b') {
                    $testClean = $t -replace '\b(Cut|Copy|Paste|Delete|Select|Select All|Look Up|Translate|Share|No Replacements Found)\b', ''
                    $testClean = $testClean -replace '\b(Co|py|te|Cu|Pa|De)\b', ''
                    $testClean = $testClean.Trim()
                    if ($testClean.Length -le 2) {
                        continue
                    }
                }
            }

            # Filter 1-3 character leftover fragments and popup balloon border debris (e.g. "r I", "I I", "r")
            if ($t.Length -le 3 -and ($t -match '^(Co|py|te|Cu|Pa|De|[<>/\\|_\-\.])$' -or $t -match '^[rIl1|/\[\]\(\)\-\s]+$')) { continue }

            $t = $t.Trim()
            if (!$t) { continue }

            $minX = 9999999
            $minY = 9999999
            $maxX = 0
            $maxY = 0
            foreach ($w in $l.Words) {
                $r = $w.BoundingRect
                if ($r.X -lt $minX) { $minX = $r.X }
                $rx = $r.X + $r.Width
                if ($rx -gt $maxX) { $maxX = $rx }
                if ($r.Y -lt $minY) { $minY = $r.Y }
                $b = $r.Y + $r.Height
                if ($b -gt $maxY) { $maxY = $b }
            }
            $lineH = [Math]::Max(10.0, ($maxY - $minY))

            $parsedLines += [PSCustomObject]@{
                Text = $t
                MinY = $minY
                MaxY = $maxY
                Height = $lineH
            }
        }

        # Top-of-Note Lock: if a date header was detected on this frame, discard any lines above it!
        if ($dateMinY -ge 0) {
            $parsedLines = @($parsedLines | Where-Object { $_.MinY -gt $dateMinY })
        }

        if ($parsedLines.Count -eq 0) {
            return @()
        }

        # Sort lines by MinY
        $sorted = @($parsedLines | Sort-Object MinY)

        # Tahap 36: Discard separator line --- and any hashtag lines appearing before opening news paragraph
        while ($sorted.Count -gt 0) {
            $firstText = $sorted[0].Text.Trim()
            if ($firstText -match '^[-—–_=]{2,}$') {
                $sorted = @($sorted[1..($sorted.Count - 1)])
                continue
            }
            if ($firstText.StartsWith('#') -and ($sorted.Count -ge 2)) {
                $hasBodyLater = $false
                for ($k = 1; $k -lt $sorted.Count; $k++) {
                    $tLater = $sorted[$k].Text.Trim()
                    if ($tLater.Length -ge 20 -and -not $tLater.StartsWith('#') -and -not ($tLater -match '^(Source|Sumber):')) {
                        $hasBodyLater = $true
                        break
                    }
                }
                if ($hasBodyLater) {
                    $sorted = @($sorted[1..($sorted.Count - 1)])
                    continue
                }
            }
            break
        }

        return $sorted
    }

    # Tahap 35 & 36: Dual-Candidate Line-by-Line Fusion per vertical Y coordinate
    function Merge-LineLists($lines1, $lines2) {
        if (!$lines1 -or $lines1.Count -eq 0) { return $lines2 }
        if (!$lines2 -or $lines2.Count -eq 0) { return $lines1 }

        # Check for NOTES_LIST_VIEW
        if ($lines1.Count -eq 1 -and $lines1[0].Text -eq "<<<NOTES_LIST_VIEW>>>") { return $lines1 }
        if ($lines2.Count -eq 1 -and $lines2[0].Text -eq "<<<NOTES_LIST_VIEW>>>") { return $lines2 }

        $allLines = @($lines1) + @($lines2)
        $sumH = 0
        foreach ($l in $allLines) { $sumH += $l.Height }
        $avgH = if ($allLines.Count -gt 0) { $sumH / $allLines.Count } else { 30.0 }

        $used2 = @{}
        $fused = @()

        foreach ($l1 in $lines1) {
            $cy1 = ($l1.MinY + $l1.MaxY) / 2.0
            $bestMatch2 = $null
            $bestIdx2 = -1
            $bestDist = 999999.0

            for ($j = 0; $j -lt $lines2.Count; $j++) {
                if ($used2.ContainsKey($j)) { continue }
                $l2 = $lines2[$j]
                $cy2 = ($l2.MinY + $l2.MaxY) / 2.0
                $dist = [Math]::Abs($cy1 - $cy2)
                $isOverlap = ($l1.MinY -le $l2.MaxY) -and ($l1.MaxY -ge $l2.MinY)

                if ($isOverlap -or $dist -le (0.65 * $avgH)) {
                    if ($dist -lt $bestDist) {
                        $bestDist = $dist
                        $bestMatch2 = $l2
                        $bestIdx2 = $j
                    }
                }
            }

            if ($bestMatch2 -ne $null) {
                $used2[$bestIdx2] = $true
                $t1 = $l1.Text.Trim()
                $t2 = $bestMatch2.Text.Trim()

                # If t2 is missing or > 25% shorter than t1: take t1 (Natural)!
                # If t2 has defect (< at end) and t1 doesn't: take t1!
                # If t1 has defect (< at end) and t2 doesn't: take bestMatch2!
                $chosen = $bestMatch2
                if ($t2.Length -lt (0.75 * $t1.Length)) {
                    $chosen = $l1
                } elseif ($t2 -match '<\s*$' -and -not ($t1 -match '<\s*$')) {
                    $chosen = $l1
                } elseif ($t1 -match '<\s*$' -and -not ($t2 -match '<\s*$')) {
                    $chosen = $bestMatch2
                } elseif ($t1.Length -gt $t2.Length) {
                    $chosen = $l1
                }

                $fused += $chosen
            } else {
                # In Cand 1 only: keep it!
                $fused += $l1
            }
        }

        # Add remaining unmatched lines from Cand 2
        for ($j = 0; $j -lt $lines2.Count; $j++) {
            if (-not $used2.ContainsKey($j)) {
                $fused += $lines2[$j]
            }
        }

        $fusedSorted = @($fused | Sort-Object MinY)
        return $fusedSorted
    }

    function Format-ParsedLinesToText($sortedLines, [bool]$hasDateHeader) {
        if (!$sortedLines -or $sortedLines.Count -eq 0) {
            return @()
        }

        if ($sortedLines.Count -eq 1 -and $sortedLines[0].Text -eq "<<<NOTES_LIST_VIEW>>>") {
            return @("<<<NOTES_LIST_VIEW>>>")
        }

        # Anti-Garbage Filter for partially clipped top line at scroll edge
        if ($sortedLines.Count -ge 2) {
            $topText = $sortedLines[0].Text.Trim()
            $topTokens = $topText.Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
            $longWords = @($topTokens | Where-Object { $_ -match '^[A-Za-z]{4,}$' })
            $digitCount = ($topText -replace '[^0-9]', '').Length

            $nextText = $sortedLines[1].Text.Trim()
            $nextTokens = $nextText.Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
            $nextLongWords = @($nextTokens | Where-Object { $_ -match '^[A-Za-z]{4,}$' })

            $isGarbageTop = ($longWords.Count -eq 0) -and ($digitCount -ge 2 -or $nextLongWords.Count -ge 2 -or $topText.Length -lt 15)
            if ($isGarbageTop) {
                $sortedLines = @($sortedLines[1..($sortedLines.Count - 1)])
            }
        }

        $sumH = 0
        foreach ($item in $sortedLines) {
            $sumH += $item.Height
        }
        $avgH = if ($sortedLines.Count -gt 0) { $sumH / $sortedLines.Count } else { 30.0 }

        # Anti-Garbage Filter for partially clipped bottom line at scroll/crop edge or floating toolbar (Tahap 39/40)
        if ($sortedLines.Count -ge 3) {
            $lastIdx = $sortedLines.Count - 1
            $lastItem = $sortedLines[$lastIdx]
            $lastText = $lastItem.Text.Trim()
            $isHashtag = ($lastText -match '#')
            $isDamagedNotesTail = ($lastText -match '(?i)\b(ccrta|dilanorkan|hcrhacil|kcrhacil)\b')
            $isClippedBottom = (-not $isHashtag) -and (($lastItem.Height -lt ($avgH * 0.55)) -or ($lastText -match '^[^a-zA-Z0-9#@]+$') -or $isDamagedNotesTail)
            if ($isClippedBottom) {
                $sortedLines = @($sortedLines[0..($lastIdx - 1)])
            }
        }

        $lines = @()
        if ($hasDateHeader) {
            $lines += "<<<DOC_START>>>"
        }
        for ($i = 0; $i -lt $sortedLines.Count; $i++) {
            $curr = $sortedLines[$i]
            if ($i -gt 0) {
                $prev = $sortedLines[$i - 1]
                $gap = $curr.MinY - $prev.MaxY
                $prevCenterY = ($prev.MinY + $prev.MaxY) / 2.0
                $currCenterY = ($curr.MinY + $curr.MaxY) / 2.0
                $centerStep = $currCenterY - $prevCenterY

                if ($sortedLines.Count -le 7) {
                    if (($gap -gt ($avgH * 0.25)) -or ($centerStep -gt ($avgH * 1.52))) {
                        $lines += ""
                    }
                } else {
                    # Tahap 40: 0.58 * avgH or 1.68 * centerStep allows genuine empty paragraph lines
                    if (($gap -gt ($avgH * 0.58)) -or ($centerStep -gt ($avgH * 1.68))) {
                        $lines += ""
                    }
                }
            }
            $lines += $curr.Text
        }
        return $lines
    }

    $pathList = @()
    if ($ImagePaths) {
        $pathList = $ImagePaths.Split('|', [StringSplitOptions]::RemoveEmptyEntries)
    } elseif ($ImagePath -and $ImagePath.Contains('|')) {
        $pathList = $ImagePath.Split('|', [StringSplitOptions]::RemoveEmptyEntries)
    } elseif ($ImagePath) {
        $pathList = @($ImagePath)
    }

    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $projectRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..'))

    if ($pathList.Count -eq 0) {
        exit 0
    }

    function Process-SingleFrame([string]$imageFilePath, [bool]$isLast, [bool]$isFullViewport, [bool]$onlyCand1) {
        if (!(Test-Path $imageFilePath)) {
            return @{ Lines = @(); FoundHighlight = $false; SelMinY = 0; SelMaxY = 0; Height = 0 }
        }
        $fullPath = (Resolve-Path $imageFilePath).Path
        $srcBmp = [System.Drawing.Bitmap]::FromFile($fullPath)

        $candNat = $null
        $candRed = $null
        $foundHighlight = $false
        $cropX = 0
        $cropY = 0
        $cropH = 0
        $selMinY = 0
        $selMaxY = 0
        $selColor = "none"

        [DualOcrPreprocessor]::PrepareCandidates($srcBmp, $isFullViewport, $onlyCand1, [ref]$candNat, [ref]$candRed, [ref]$foundHighlight, [ref]$cropX, [ref]$cropY, [ref]$cropH, [ref]$selMinY, [ref]$selMaxY, [ref]$selColor)

        # Tahap 37: OCR on Candidate 1 (always) and Candidate 2 (if present)
        $parsedNat = @(Get-OcrParsedLines $candNat)
        $parsedRed = @()
        if ($candRed -ne $null) {
            $parsedRed = @(Get-OcrParsedLines $candRed)
        }

        $isNotesList = $false
        if ($parsedNat.Count -eq 1 -and $parsedNat[0].Text -eq "<<<NOTES_LIST_VIEW>>>") { $isNotesList = $true }
        if ($parsedRed.Count -eq 1 -and $parsedRed[0].Text -eq "<<<NOTES_LIST_VIEW>>>") { $isNotesList = $true }
        if ($isNotesList) {
            $srcH = $srcBmp.Height
            $srcBmp.Dispose()
            if ($candNat -ne $null) { $candNat.Dispose() }
            if ($candRed -ne $null) { $candRed.Dispose() }
            return @{ Lines = @("<<<NOTES_LIST_VIEW>>>"); FoundHighlight = $false; SelMinY = 0; SelMaxY = 0; Height = $srcH; SelectionColor = "none" }
        }

        # Merge Candidate 1 (Natural) & Candidate 2 (Channel-Filtered if available)
        $mergedParsed = if ($parsedRed.Count -gt 0) {
            @(Merge-LineLists $parsedNat $parsedRed)
        } else {
            $parsedNat
        }

        $hasDate = ($parsedNat -contains "<<<DOC_START>>>" -or $parsedRed -contains "<<<DOC_START>>>")
        $finalLines = @(Format-ParsedLinesToText $mergedParsed $hasDate)

        # If it's the last (current) frame, save debug_ocr_cleaned.png
        if ($isLast) {
            $saveBmp = if ($candRed -ne $null) { $candRed } else { $candNat }
            if ($saveBmp -ne $null) {
                $debugCleanedPath = [System.IO.Path]::Combine($projectRoot, 'debug_ocr_cleaned.png')
                $saveBmp.Save($debugCleanedPath, [System.Drawing.Imaging.ImageFormat]::Png)
            }
        }

        $srcH = $srcBmp.Height
        $srcBmp.Dispose()
        if ($candNat -ne $null) { $candNat.Dispose() }
        if ($candRed -ne $null) { $candRed.Dispose() }

        return @{
            Lines = $finalLines
            FoundHighlight = $foundHighlight
            SelMinY = $selMinY
            SelMaxY = $selMaxY
            Height = $srcH
            SelectionColor = $selColor
        }
    }

    # Tahap 37: Fast Current-Frame-First Execution & Early-Exit (< 0.7s)
    $lastIdx = $pathList.Count - 1
    $currentPath = $pathList[$lastIdx].Trim()

    # Process CURRENT frame first with dual-candidate high quality
    $resCurrent = Process-SingleFrame $currentPath $true $false $false

    # Evaluate non-empty lines of current frame
    $cleanLines = @($resCurrent.Lines | Where-Object { 
        $_ -and $_.Trim() -ne "" -and $_ -ne "<<<DOC_START>>>" -and $_ -ne "<<<NOTES_LIST_VIEW>>>" 
    })

    $totalChars = ($cleanLines -join '').Length
    $isShortSnippet = ($cleanLines.Count -ge 1 -and $cleanLines.Count -le 5 -and $totalChars -lt 200)
    $isNotesList = ($resCurrent.Lines -contains "<<<NOTES_LIST_VIEW>>>")

    # Complete 1-Screen Caption Check (Tahap 39/40):
    # >= 6 non-empty lines, starts with capital/quote opener at selMinY >= 0.130*H, ends with #hashtag
    $hasHashtagEnd = ($cleanLines.Count -ge 2 -and ($cleanLines[-1].Trim().StartsWith("#") -or $cleanLines[-2].Trim().StartsWith("#") -or $cleanLines[-1].Trim() -match '^#[a-zA-Z0-9_]+' -or $cleanLines[-2].Trim() -match '^#[a-zA-Z0-9_]+'))
    $hasValidStart = ($cleanLines.Count -ge 1 -and $cleanLines[0].Trim() -match '^[A-Z"“]' -and -not $cleanLines[0].Trim().EndsWith(','))
    $isCompleteSingleScreen = ($cleanLines.Count -ge 6 -and $hasHashtagEnd -and $hasValidStart -and $resCurrent.SelMinY -ge [int]($resCurrent.Height * 0.130))

    if ($pathList.Count -eq 1 -or $isShortSnippet -or $isNotesList -or $isCompleteSingleScreen) {
        # FAST EARLY EXIT: current frame is 100% complete, skip history frames!
        $outText = $resCurrent.Lines -join "`r`n"
        if ($resCurrent.SelectionColor) {
            $outText = "<<<SEL_COLOR:$($resCurrent.SelectionColor)>>>`r`n" + $outText
        }
        [Console]::WriteLine($outText)
        exit 0
    }

    # If multi-screen caption needed (e.g. Apple Notes scrolled to bottom or scrolled to top),
    # process at most 3 representative history frames with fast single candidate
    $histPaths = @($pathList[0..($lastIdx - 1)])
    if ($histPaths.Count -gt 3) {
        $histPaths = @($histPaths[($histPaths.Count - 3)..($histPaths.Count - 1)])
    }

    $allOutputs = @()
    foreach ($hPath in $histPaths) {
        $hTrimmed = $hPath.Trim()
        if (-not (Test-Path $hTrimmed)) { continue }

        # History frame: use fast single candidate (Candidate 1 Clean Natural RGB)
        $hRes = Process-SingleFrame $hTrimmed $false $false $true

        # If highlight missing or span < 0.15*H, use Full-Viewport Content Crop (Tahap 39)
        $hSpan = $hRes.SelMaxY - $hRes.SelMinY
        $hMinSpan = [int]($hRes.Height * 0.15)
        if (-not $hRes.FoundHighlight -or $hSpan -lt $hMinSpan) {
            $hRes = Process-SingleFrame $hTrimmed $false $true $true
        }

        $hLines = @($hRes.Lines)
        if ($hLines.Count -gt 0) {
            if ($hLines -contains "<<<NOTES_LIST_VIEW>>>") {
                $allOutputs += "<<<NOTES_LIST_VIEW>>>"
            } else {
                $allOutputs += ($hLines -join "`r`n")
            }
        }
    }

    # Add CURRENT frame as the last frame
    $currOut = ($resCurrent.Lines -join "`r`n")
    if ($resCurrent.SelectionColor) {
        $currOut = "<<<SEL_COLOR:$($resCurrent.SelectionColor)>>>`r`n" + $currOut
    }
    $allOutputs += $currOut

    # Output frames cleanly separated by ---FRAME_SPLIT---
    [Console]::WriteLine($allOutputs -join "`r`n---FRAME_SPLIT---`r`n")
} catch {
    # Fail cleanly
}
