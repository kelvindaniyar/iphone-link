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
        out Bitmap cand1Natural,
        out Bitmap cand2RedFiltered,
        out bool foundHighlight,
        out int cropX,
        out int cropY,
        out int cropH
    ) {
        int W = src.Width;
        int H = src.Height;

        BitmapData srcData = src.LockBits(new Rectangle(0, 0, W, H), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        int stride = srcData.Stride;
        byte[] raw = new byte[Math.Abs(stride) * H];
        Marshal.Copy(srcData.Scan0, raw, 0, raw.Length);
        src.UnlockBits(srcData);

        // 1. Detect and Mask AssistiveTouch floating button in Left Zone (X < 0.20 * W) or Right Zone (X > 0.80 * W)
        // Neutral gray: |R - G| <= 14 && |G - B| <= 14 && |R - B| <= 14 && R >= 48 && R <= 195
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

        // 2. Scan highlights (Ignore top toolbar buttons and bottom floating pill toolbar)
        int scanTopY = (int)(H * 0.145);
        int scanBottomY = (int)(H * 0.785);
        int leftX = (int)(W * 0.04);
        int rightX = (int)(W * 0.96);

        bool[] rowHasHighlight = new bool[H];
        foundHighlight = false;

        if (!isFullViewport) {
            for (int y = scanTopY; y <= scanBottomY; y += 2) {
                // Rule 1A: For Y < 0.170 * H, exclude left zone (X < 0.22 * W) and right zone (X > 0.76 * W)
                // where '<' back button and yellow '✓' Done button live!
                int rowLeftX = (y < (int)(H * 0.170)) ? (int)(W * 0.22) : leftX;
                int rowRightX = (y < (int)(H * 0.170)) ? (int)(W * 0.76) : rightX;

                int rowOffset = y * stride;
                int rowHighlightCount = 0;

                for (int x = rowLeftX; x <= rowRightX; x += 3) {
                    int idx = rowOffset + (x * 4);
                    byte b = raw[idx];
                    byte g = raw[idx + 1];
                    byte r = raw[idx + 2];

                    // Notes pale yellow highlight: R > 220, G > 205, B < 238, R - B >= 12
                    bool isPaleYellow = (r > 220) && (g > 205) && (b < 238) && ((r - b) >= 12);
                    // Notes lollipop handle yellow-orange: R > 200, G > 140, R - B >= 40
                    bool isLollipop = (r > 200) && (g > 140) && ((r - b) >= 40);
                    // Standard iOS blue selection: B > 140, B - R >= 22
                    bool isBlue = (b > 140) && ((b - r) >= 22);

                    if (isPaleYellow || isLollipop || isBlue) {
                        rowHighlightCount++;
                    }
                }

                if (rowHighlightCount >= 3) {
                    rowHasHighlight[y] = true;
                    if (y + 1 < H) rowHasHighlight[y + 1] = true;
                }
            }
        }

        // Rule 1B: Contiguous vertical clustering: tolerance gap <= 0.028 * H
        int maxLineGap = (int)(H * 0.028);
        int bestClusterStart = -1;
        int bestClusterEnd = -1;
        int bestClusterHeight = 0;

        int curStart = -1;
        int lastRow = -1;

        for (int y = scanTopY; y <= scanBottomY; y++) {
            if (rowHasHighlight[y]) {
                if (curStart == -1) {
                    curStart = y;
                } else if (lastRow != -1 && (y - lastRow) > maxLineGap) {
                    int h = lastRow - curStart;
                    if (h >= (int)(H * 0.010) && h > bestClusterHeight) {
                        bestClusterHeight = h;
                        bestClusterStart = curStart;
                        bestClusterEnd = lastRow;
                    }
                    curStart = y;
                }
                lastRow = y;
            }
        }

        if (curStart != -1 && lastRow != -1) {
            int h = lastRow - curStart;
            if (h >= (int)(H * 0.010) && h > bestClusterHeight) {
                bestClusterHeight = h;
                bestClusterStart = curStart;
                bestClusterEnd = lastRow;
            }
        }

        int minY = 9999999;
        int maxY = 0;
        if (bestClusterStart != -1 && bestClusterEnd != -1) {
            foundHighlight = true;
            minY = bestClusterStart;
            maxY = bestClusterEnd;
        }

        // FULL-WIDTH HORIZONTAL STRIP: 0.03*W to 0.97*W
        cropX = Math.Max(0, (int)(W * 0.03));
        int cropW = Math.Min(W - cropX, (int)(W * 0.94));

        if (!isFullViewport && foundHighlight) {
            // Strictly follow actual highlight vertical bounds!
            cropY = Math.Max((int)(H * 0.115), minY - (int)(H * 0.015));
            int bottomLimit = Math.Min((int)(H * 0.88), maxY + (int)(H * 0.030));
            cropH = Math.Max(20, bottomLimit - cropY);
        } else {
            // Full-Viewport Content Crop (for history frames or unhighlighted notes)
            // Exclude top banner / date (< 0.145 * H) and bottom floating toolbar (> 0.768 * H)
            cropY = (int)(H * 0.145);
            int cropBottom = (int)(H * 0.768);
            cropH = Math.Max(20, cropBottom - cropY);
        }

        // Candidate 1: Natural crop without aggressive pixel removal
        Bitmap stripNatural = new Bitmap(cropW, cropH, PixelFormat.Format32bppArgb);
        BitmapData natData = stripNatural.LockBits(new Rectangle(0, 0, cropW, cropH), ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
        byte[] natPixels = new byte[Math.Abs(natData.Stride) * cropH];

        // Candidate 2: Red-channel filtered crop
        Bitmap stripRed = new Bitmap(cropW, cropH, PixelFormat.Format32bppArgb);
        BitmapData redData = stripRed.LockBits(new Rectangle(0, 0, cropW, cropH), ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
        byte[] redPixels = new byte[Math.Abs(redData.Stride) * cropH];

        for (int cy = 0; cy < cropH; cy++) {
            int srcRow = (cropY + cy) * stride;
            int natRow = cy * natData.Stride;
            int redRow = cy * redData.Stride;
            for (int cx = 0; cx < cropW; cx++) {
                int sIdx = srcRow + ((cropX + cx) * 4);
                int nIdx = natRow + (cx * 4);
                int rIdx = redRow + (cx * 4);

                byte b = raw[sIdx];
                byte g = raw[sIdx + 1];
                byte r = raw[sIdx + 2];

                // Selection handle filter (Tahap 30):
                // Yellow-orange selection handle lollipop: R > 180 && G > 120 && (R - B) > 40
                // Dark text edge guard: R < 115 && G < 115 && B < 115
                bool isHandle = (r > 180) && (g > 120) && ((r - b) > 40);
                bool isDarkText = (r < 115) && (g < 115) && (b < 115);

                // Natural: direct copy, but neutralize handle to white without erasing dark text
                if (isHandle && !isDarkText) {
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

                // Red-filtered:
                bool isColorNoise = (r > 175) && (((r - b) > 35) || ((r - g) > 30));
                byte v;
                if ((isHandle && !isDarkText) || r >= 210 || isColorNoise) {
                    v = 255;
                } else {
                    int scaled = (int)((r - 25) * 255.0 / (210 - 25));
                    if (scaled < 0) scaled = 0;
                    if (scaled > 255) scaled = 255;
                    v = (byte)scaled;
                }
                redPixels[rIdx] = v;
                redPixels[rIdx + 1] = v;
                redPixels[rIdx + 2] = v;
                redPixels[rIdx + 3] = 255;
            }
        }

        Marshal.Copy(natPixels, 0, natData.Scan0, natPixels.Length);
        stripNatural.UnlockBits(natData);

        Marshal.Copy(redPixels, 0, redData.Scan0, redPixels.Length);
        stripRed.UnlockBits(redData);

        // Upscale 2.5x with 60px margin
        float scale = 2.5f;
        int margin = 60;
        int upW = (int)(cropW * scale);
        int upH = (int)(cropH * scale);
        int finalW = upW + (margin * 2);
        int finalH = upH + (margin * 2);

        cand1Natural = new Bitmap(finalW, finalH, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(cand1Natural)) {
            g.Clear(Color.White);
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
            g.SmoothingMode = SmoothingMode.HighQuality;
            g.DrawImage(stripNatural, new Rectangle(margin, margin, upW, upH), new Rectangle(0, 0, cropW, cropH), GraphicsUnit.Pixel);
        }
        stripNatural.Dispose();

        cand2RedFiltered = new Bitmap(finalW, finalH, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(cand2RedFiltered)) {
            g.Clear(Color.White);
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
            g.SmoothingMode = SmoothingMode.HighQuality;
            g.DrawImage(stripRed, new Rectangle(margin, margin, upW, upH), new Rectangle(0, 0, cropW, cropH), GraphicsUnit.Pixel);
        }
        stripRed.Dispose();
    }

    public static bool VerifyLineHighlight(
        Bitmap origBmp,
        int cropX, int cropY,
        float scale, int margin,
        double lineX, double lineY, double lineW, double lineH
    ) {
        int W = origBmp.Width;
        int H = origBmp.Height;

        int ox = cropX + (int)((lineX - margin) / scale);
        int oy = cropY + (int)((lineY - margin) / scale);
        int ow = (int)(lineW / scale);
        int oh = (int)(lineH / scale);

        oy = Math.Max(0, oy - 4);
        oh = Math.Min(H - oy, oh + 8);

        if (ox < 0) { ow += ox; ox = 0; }
        if (ox + ow > W) ow = W - ox;
        if (ow <= 10 || oh <= 4) return true;

        BitmapData data = origBmp.LockBits(new Rectangle(ox, oy, ow, oh), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        int stride = data.Stride;
        byte[] pixels = new byte[Math.Abs(stride) * oh];
        Marshal.Copy(data.Scan0, pixels, 0, pixels.Length);
        origBmp.UnlockBits(data);

        int highlightCount = 0;
        int totalSampled = 0;

        for (int y = 0; y < oh; y += 2) {
            int row = y * stride;
            for (int x = 0; x < ow; x += 3) {
                int idx = row + (x * 4);
                byte b = pixels[idx];
                byte g = pixels[idx + 1];
                byte r = pixels[idx + 2];
                totalSampled++;

                bool isYellow = (r > 200) && (g > 175) && ((r - b) >= 10);
                bool isHandle = (r > 190) && ((r - b) >= 35);
                bool isBlue = (b > 130) && ((b - r) >= 15);

                if (isYellow || isHandle || isBlue) {
                    highlightCount++;
                }
            }
        }

        if (totalSampled == 0) return true;
        float ratio = (float)highlightCount / (float)totalSampled;
        return ratio >= 0.05f;
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

    function Run-Ocr([System.Drawing.Bitmap]$bmp, [System.Drawing.Bitmap]$origBmp = $null, [int]$cropX = 0, [int]$cropY = 0, [bool]$checkHighlight = $false) {
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

        $dateRegex = '(?i)^\d{1,2}\s+(January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4}(\s+(at|pukul)\s+\d{1,2}[.:]\d{2})?'
        $timeRegex = '^[lu0-9OIl]{1,2}[.:][0-9OIl]{2}\b'
        $calloutRegex = '^(Cut|Copy|Paste|Delete|Select|Select All|Look Up|Translate|Share\.\.\.|No Replacements Found|Device Hub|This device was viewed remotely|Co|py|Pas|te|>\s*|\.\.\.)$'

        $parsedLines = @()
        foreach ($l in $ocrResult.Lines) {
            $t = $l.Text.Trim()
            if (!$t) { continue }
            if ($t -match $dateRegex -or $t -match $timeRegex) { continue }
            if ($t -match $calloutRegex) { continue }
            if ($t -match '(?i)(jendela-kecil|Device Hub|This device was viewed remotely|remotely from Mac)') { continue }
            if ($t -match '\b(v3one|oil13i1|111e113n|me1a1ui|miliknva|aO/O3124an)\b') { continue }
            if ($t -match '^[<✓>]+$' -or $t -eq 'Notes' -or $t -eq 'Catatan' -or $t -eq 'Done' -or $t -eq 'Selesai') { continue }

            # Filter iOS callout menu balloon lines (e.g. "Cut Co", "Paste Delete", "Cut Copy Paste")
            if ($t -match '\b(Cut|Copy|Paste|Delete|Select|Look Up|Translate|Share|Replacements)\b') {
                $testClean = $t -replace '\b(Cut|Copy|Paste|Delete|Select|Select All|Look Up|Translate|Share|No Replacements Found)\b', ''
                $testClean = $testClean -replace '\b(Co|py|te|Cu|Pa|De)\b', ''
                $testClean = $testClean.Trim()
                if ($testClean.Length -le 2) {
                    continue
                }
            }

            # Filter 1-2 character leftover fragments
            if ($t.Length -le 2 -and ($t -match '^(Co|py|te|Cu|Pa|De|[<>/\\|_\-\.])$')) { continue }

            # Remove stray embedded callout words if any
            $t = $t -replace '\b(Cut|Copy|Paste|Delete|Select|Select All)\b', ''
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
            $lineW = [Math]::Max(10.0, ($maxX - $minX))
            $lineH = [Math]::Max(10.0, ($maxY - $minY))

            # Rule 1C: If image has a highlight selection, verify that this line actually has highlight behind it!
            if ($checkHighlight -and $origBmp -ne $null) {
                $hasHl = [DualOcrPreprocessor]::VerifyLineHighlight($origBmp, $cropX, $cropY, 2.5, 60, $minX, $minY, $lineW, $lineH)
                if (-not $hasHl) {
                    continue
                }
            }

            $parsedLines += [PSCustomObject]@{
                Text = $t
                MinY = $minY
                MaxY = $maxY
                Height = $lineH
            }
        }

        if ($parsedLines.Count -eq 0) {
            return @()
        }

        # Sort lines by MinY (explicitly wrap with @() for single line items)
        $sortedLines = @($parsedLines | Sort-Object MinY)

        # Anti-Garbage Filter for partially clipped top line at scroll edge (e.g. "1.1n1 j un 11" or "u 11 1")
        if ($sortedLines.Count -ge 2) {
            $topText = $sortedLines[0].Text.Trim()
            $topTokens = $topText.Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
            $longWords = @($topTokens | Where-Object { $_ -match '^[A-Za-z]{4,}$' })
            $digitCount = ($topText -replace '[^0-9]', '').Length

            $nextText = $sortedLines[1].Text.Trim()
            $nextTokens = $nextText.Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
            $nextLongWords = @($nextTokens | Where-Object { $_ -match '^[A-Za-z]{4,}$' })

            # If top line has NO 4+ letter words AND (has digits OR next line is a normal sentence with 4+ letter words)
            $isGarbageTop = ($longWords.Count -eq 0) -and ($digitCount -ge 2 -or $nextLongWords.Count -ge 2 -or $topText.Length -lt 15)
            if ($isGarbageTop) {
                $sortedLines = @($sortedLines[1..($sortedLines.Count - 1)])
            }
        }

        $sumH = 0
        foreach ($item in $sortedLines) {
            $sumH += $item.Height
        }
        $avgH = $sumH / $sortedLines.Count

        $lines = @()
        for ($i = 0; $i -lt $sortedLines.Count; $i++) {
            $curr = $sortedLines[$i]
            if ($i -gt 0) {
                $prev = $sortedLines[$i - 1]
                $gap = $curr.MinY - $prev.MaxY
                if ($gap -gt ($avgH * 0.32)) {
                    $lines += ""
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

    function Process-SingleFrame([string]$imageFilePath, [bool]$isLast, [bool]$isFullViewport) {
        if (!(Test-Path $imageFilePath)) {
            return @()
        }
        $fullPath = (Resolve-Path $imageFilePath).Path
        $srcBmp = [System.Drawing.Bitmap]::FromFile($fullPath)

        $candNat = $null
        $candRed = $null
        $foundHighlight = $false
        $cropX = 0
        $cropY = 0
        $cropH = 0

        [DualOcrPreprocessor]::PrepareCandidates($srcBmp, $isFullViewport, [ref]$candNat, [ref]$candRed, [ref]$foundHighlight, [ref]$cropX, [ref]$cropY, [ref]$cropH)

        $doVerify = ($foundHighlight -and -not $isFullViewport)

        # Fast candidate execution: Red-filtered first (best for Notes Light Mode & spellcheck removal)
        $bestLines = @(Run-Ocr $candRed $srcBmp $cropX $cropY $doVerify)
        $bestBmp = $candRed
        if ($bestLines.Count -eq 0 -and $candNat -ne $null) {
            # Fallback to Natural candidate if Red-filtered gave nothing (e.g. Dark Mode)
            $linesNat = @(Run-Ocr $candNat $srcBmp $cropX $cropY $doVerify)
            if ($linesNat.Count -gt 0) {
                $bestLines = $linesNat
                $bestBmp = $candNat
            }
        }

        # If it's the last (current) frame, save debug_ocr_cleaned.png
        if ($isLast -and $bestBmp -ne $null) {
            $debugCleanedPath = [System.IO.Path]::Combine($projectRoot, 'debug_ocr_cleaned.png')
            $bestBmp.Save($debugCleanedPath, [System.Drawing.Imaging.ImageFormat]::Png)
        }

        $srcBmp.Dispose()
        if ($candNat -ne $null) { $candNat.Dispose() }
        if ($candRed -ne $null) { $candRed.Dispose() }

        return $bestLines
    }

    $lastIdx = $pathList.Count - 1
    # Process current frame (at $lastIdx) first:
    $currentLines = @(Process-SingleFrame $pathList[$lastIdx].Trim() $true $false)
    $nonEmptyCurrent = @($currentLines | Where-Object { $_.Trim().Length -gt 0 })
    $charCountCurrent = ($currentLines -join "").Length

    # If only 1 frame, OR if current frame is short (< 4 non-empty lines and < 150 chars):
    # Output immediately and exit! (Ultra-fast single pass ~0.4s)
    if ($pathList.Count -eq 1 -or ($nonEmptyCurrent.Count -lt 4 -and $charCountCurrent -lt 150)) {
        if ($currentLines.Count -gt 0) {
            [Console]::WriteLine($currentLines -join "`r`n")
        }
        exit 0
    }

    # Otherwise, it's long text (>= 4 lines or >= 150 chars) and multiple frames are provided.
    # Process history frames 0 .. ($lastIdx - 1) in the exact same process
    $allOutputs = @()
    for ($i = 0; $i -lt $lastIdx; $i++) {
        $histLines = @(Process-SingleFrame $pathList[$i].Trim() $false $true)
        if ($histLines.Count -gt 0) {
            $allOutputs += ($histLines -join "`r`n")
        }
    }

    # Append current frame text
    if ($currentLines.Count -gt 0) {
        $allOutputs += ($currentLines -join "`r`n")
    }

    # Output frames separated by ---FRAME_SPLIT---
    [Console]::WriteLine($allOutputs -join "`r`n---FRAME_SPLIT---`r`n")
} catch {
    # Fail silently without breaking server
}

