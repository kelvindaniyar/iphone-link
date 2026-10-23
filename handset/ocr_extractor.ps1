param(
    [Parameter(Mandatory=$true)]
    [string]$ImagePath
)

$ErrorActionPreference = "SilentlyContinue"

try {
    Add-Type -AssemblyName 'System.Runtime.WindowsRuntime'
    Add-Type -AssemblyName 'System.Drawing'
    Add-Type -ReferencedAssemblies 'System.Drawing' -TypeDefinition '
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public class DualOcrPreprocessor {
    public static void PrepareCandidates(Bitmap src, out Bitmap cand1Natural, out Bitmap cand2RedFiltered, out bool foundHighlight, out int cropY, out int cropH) {
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
            int zoneXStart = (side == 0) ? 0 : (int)(W * 0.80);
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
                        if (cW >= 30 && cH >= 35 && clusterRowCount >= 25) {
                            int pad = 8;
                            int mX0 = Math.Max(0, clusterMinX - pad);
                            int mX1 = Math.Min(W - 1, clusterMaxX + pad);
                            int mY0 = Math.Max(0, clusterMinY - pad);
                            int mY1 = Math.Min(H - 1, clusterMaxY + pad);
                            for (int my = mY0; my <= mY1; my++) {
                                int mOffset = my * stride;
                                for (int mx = mX0; mx <= mX1; mx++) {
                                    int mIdx = mOffset + (mx * 4);
                                    raw[mIdx] = 255;
                                    raw[mIdx + 1] = 255;
                                    raw[mIdx + 2] = 255;
                                    raw[mIdx + 3] = 255;
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
                if (cW >= 30 && cH >= 35 && clusterRowCount >= 25) {
                    int pad = 8;
                    int mX0 = Math.Max(0, clusterMinX - pad);
                    int mX1 = Math.Min(W - 1, clusterMaxX + pad);
                    int mY0 = Math.Max(0, clusterMinY - pad);
                    int mY1 = Math.Min(H - 1, clusterMaxY + pad);
                    for (int my = mY0; my <= mY1; my++) {
                        int mOffset = my * stride;
                        for (int mx = mX0; mx <= mX1; mx++) {
                            int mIdx = mOffset + (mx * 4);
                            raw[mIdx] = 255;
                            raw[mIdx + 1] = 255;
                            raw[mIdx + 2] = 255;
                            raw[mIdx + 3] = 255;
                        }
                    }
                }
            }
        }

        // 2. Scan highlights starting at Y >= 0.125 * H to avoid top toolbar yellow icons
        int scanTopY = (int)(H * 0.125);
        int scanBottomY = (int)(H * 0.88);
        int leftX = (int)(W * 0.04);
        int rightX = (int)(W * 0.96);

        int minY = 9999999;
        int maxY = 0;
        foundHighlight = false;

        for (int y = scanTopY; y <= scanBottomY; y += 2) {
            int rowOffset = y * stride;
            for (int x = leftX; x <= rightX; x += 3) {
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
                    foundHighlight = true;
                    if (y < minY) minY = y;
                    if (y > maxY) maxY = y;
                }
            }
        }

        // FULL-WIDTH HORIZONTAL STRIP: 0.03*W to 0.97*W
        int cropX = Math.Max(0, (int)(W * 0.03));
        int cropW = Math.Min(W - cropX, (int)(W * 0.94));

        if (foundHighlight) {
            if (minY < (int)(H * 0.35) || ((maxY - minY) > (int)(H * 0.10))) {
                cropY = (int)(H * 0.105);
            } else {
                cropY = Math.Max((int)(H * 0.105), minY - (int)(H * 0.035));
            }
            int bottomLimit = Math.Min((int)(H * 0.88), maxY + (int)(H * 0.04));
            cropH = Math.Max(20, bottomLimit - cropY);
        } else {
            cropY = (int)(H * 0.105);
            cropH = (int)(H * 0.775);
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

                // Natural: direct copy
                natPixels[nIdx] = b;
                natPixels[nIdx + 1] = g;
                natPixels[nIdx + 2] = r;
                natPixels[nIdx + 3] = 255;

                // Red-filtered
                bool isColorNoise = (r > 175) && (((r - b) > 35) || ((r - g) > 30));
                byte v;
                if (r >= 210 || isColorNoise) {
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
}
'

    [void][Windows.Media.Ocr.OcrEngine, Windows.Foundation.Metadata, ContentType = WindowsRuntime]
    [void][Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation.Metadata, ContentType = WindowsRuntime]
    [void][Windows.Storage.StorageFile, Windows.Foundation.Metadata, ContentType = WindowsRuntime]

    $asTaskGeneric = [System.WindowsRuntimeSystemExtensions].GetMethods() | ? { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.IsGenericMethod } | Select-Object -First 1

    function Await-Op($op, [type]$t) {
        $netTask = $asTaskGeneric.MakeGenericMethod($t).Invoke($null, @($op))
        [void]$netTask.Wait(-1)
        return $netTask.Result
    }

    function Run-Ocr([System.Drawing.Bitmap]$bmp) {
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

        $dateRegex = '^\d{1,2}\s+[A-Za-z]+\s+\d{4}\s+(at|pukul)\s+\d{1,2}[.:]\d{2}'
        $timeRegex = '^[lu0-9OIl]{1,2}[.:][0-9OIl]{2}\b'
        $calloutRegex = '^(Cut|Copy|Paste|Delete|Select|Select All|Look Up|Translate|Share\.\.\.|No Replacements Found|Device Hub|This device was viewed remotely|Co|py|Pas|te|>\s*|\.\.\.)$'

        $parsedLines = @()
        foreach ($l in $ocrResult.Lines) {
            $t = $l.Text.Trim()
            if (!$t) { continue }
            if ($t -match $dateRegex -or $t -match $timeRegex) { continue }
            if ($t -match $calloutRegex) { continue }
            if ($t -match 'This device was viewed remotely') { continue }
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

            $minY = 9999999
            $maxY = 0
            foreach ($w in $l.Words) {
                $r = $w.BoundingRect
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

        if ($parsedLines.Count -eq 0) {
            return @()
        }

        # Sort lines by MinY
        $sortedLines = $parsedLines | Sort-Object MinY

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
                if ($gap -gt ($avgH * 0.75)) {
                    $lines += ""
                }
            }
            $lines += $curr.Text
        }
        return $lines
    }

    if (!(Test-Path $ImagePath)) {
        exit 0
    }

    $fullPath = (Resolve-Path $ImagePath).Path
    $srcBmp = [System.Drawing.Bitmap]::FromFile($fullPath)

    $candNat = $null
    $candRed = $null
    $foundHighlight = $false
    $cropY = 0
    $cropH = 0
    [DualOcrPreprocessor]::PrepareCandidates($srcBmp, [ref]$candNat, [ref]$candRed, [ref]$foundHighlight, [ref]$cropY, [ref]$cropH)
    $srcBmp.Dispose()

    # Run OCR on Candidate 1 (Natural) and Candidate 2 (Red-Filtered)
    $lines1 = Run-Ocr $candNat
    $lines2 = Run-Ocr $candRed

    $len1 = ($lines1 -join ' ').Trim().Length
    $len2 = ($lines2 -join ' ').Trim().Length

    # Select the candidate with highest textual yield / quality
    $bestBmp = $candNat
    $bestLines = $lines1

    if ($len2 -ge $len1 -and $len2 -gt 0) {
        $bestBmp = $candRed
        $bestLines = $lines2
    } elseif ($len1 -gt 0) {
        $bestBmp = $candNat
        $bestLines = $lines1
    }

    # Save best cleaned debug image to project root
    $projectRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..'))
    $debugCleanedPath = [System.IO.Path]::Combine($projectRoot, 'debug_ocr_cleaned.png')
    $bestBmp.Save($debugCleanedPath, [System.Drawing.Imaging.ImageFormat]::Png)

    $candNat.Dispose()
    $candRed.Dispose()

    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    if ($bestLines.Count -gt 0) {
        $resultText = $bestLines -join "`r`n"
        [Console]::WriteLine($resultText)
    }
} catch {
    # Fail silently without breaking server
}
