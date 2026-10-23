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

public class OcrPreprocessor {
    public static Bitmap PrepareStrip(Bitmap src, out bool foundHighlight, out int cropY, out int cropH) {
        int W = src.Width;
        int H = src.Height;
        
        int topY = (int)(H * 0.16);
        int bottomY = (int)(H * 0.82);
        int leftX = (int)(W * 0.04);
        int rightX = (int)(W * 0.96);

        BitmapData srcData = src.LockBits(new Rectangle(0, 0, W, H), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        int stride = srcData.Stride;
        byte[] raw = new byte[Math.Abs(stride) * H];
        Marshal.Copy(srcData.Scan0, raw, 0, raw.Length);
        src.UnlockBits(srcData);

        int minY = 9999999;
        int maxY = 0;
        foundHighlight = false;

        for (int y = topY; y <= bottomY; y += 2) {
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
            cropY = Math.Max((int)(H * 0.15), minY - (int)(H * 0.035));
            int rawH = (maxY - cropY) + (int)(H * 0.035);
            cropH = Math.Min((int)(H * 0.84) - cropY, rawH);
            if (cropH < 20) cropH = 20;
        } else {
            cropY = (int)(H * 0.16);
            cropH = (int)(H * 0.66);
        }

        Bitmap stripBmp = new Bitmap(cropW, cropH, PixelFormat.Format32bppArgb);
        BitmapData stripData = stripBmp.LockBits(new Rectangle(0, 0, cropW, cropH), ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
        byte[] stripPixels = new byte[Math.Abs(stripData.Stride) * cropH];

        for (int cy = 0; cy < cropH; cy++) {
            int srcRow = (cropY + cy) * stride;
            int dstRow = cy * stripData.Stride;
            for (int cx = 0; cx < cropW; cx++) {
                int sIdx = srcRow + ((cropX + cx) * 4);
                int dIdx = dstRow + (cx * 4);

                byte b = raw[sIdx];
                byte g = raw[sIdx + 1];
                byte r = raw[sIdx + 2];

                // Red-Channel extraction & neutralization:
                // Yellow lollipop / yellow highlight / red spellcheck dots:
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

                stripPixels[dIdx] = v;
                stripPixels[dIdx + 1] = v;
                stripPixels[dIdx + 2] = v;
                stripPixels[dIdx + 3] = 255;
            }
        }
        Marshal.Copy(stripPixels, 0, stripData.Scan0, stripPixels.Length);
        stripBmp.UnlockBits(stripData);

        // 2.5x upscale with Bicubic and 60px white margin
        float scale = 2.5f;
        int margin = 60;
        int upW = (int)(cropW * scale);
        int upH = (int)(cropH * scale);
        int finalW = upW + (margin * 2);
        int finalH = upH + (margin * 2);

        Bitmap finalBmp = new Bitmap(finalW, finalH, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(finalBmp)) {
            g.Clear(Color.White);
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
            g.SmoothingMode = SmoothingMode.HighQuality;
            g.DrawImage(stripBmp, new Rectangle(margin, margin, upW, upH), new Rectangle(0, 0, cropW, cropH), GraphicsUnit.Pixel);
        }
        stripBmp.Dispose();

        return finalBmp;
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

    if (!(Test-Path $ImagePath)) {
        exit 0
    }

    $fullPath = (Resolve-Path $ImagePath).Path
    $srcBmp = [System.Drawing.Bitmap]::FromFile($fullPath)

    $foundHighlight = $false
    $cropY = 0
    $cropH = 0
    $finalBmp = [OcrPreprocessor]::PrepareStrip($srcBmp, [ref]$foundHighlight, [ref]$cropY, [ref]$cropH)

    $srcBmp.Dispose()

    # Save cleaned debug image to project root
    $projectRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..'))
    $debugCleanedPath = [System.IO.Path]::Combine($projectRoot, 'debug_ocr_cleaned.png')
    $finalBmp.Save($debugCleanedPath, [System.Drawing.Imaging.ImageFormat]::Png)

    $tempProcessed = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), 'ocr_proc_' + [System.Guid]::NewGuid().ToString() + '.png')
    $finalBmp.Save($tempProcessed, [System.Drawing.Imaging.ImageFormat]::Png)
    $finalBmp.Dispose()

    # WinRT OCR Execution
    $storageFile = Await-Op ([Windows.Storage.StorageFile]::GetFileFromPathAsync($tempProcessed)) ([Windows.Storage.StorageFile])
    $stream = Await-Op ($storageFile.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
    $decoder = Await-Op ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
    $softwareBitmap = Await-Op ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

    $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
    if ($null -eq $engine) {
        $stream.Dispose()
        Remove-Item $tempProcessed -Force -ErrorAction SilentlyContinue
        exit 0
    }

    $ocrResult = Await-Op ($engine.RecognizeAsync($softwareBitmap)) ([Windows.Media.Ocr.OcrResult])

    $stream.Dispose()
    Remove-Item $tempProcessed -Force -ErrorAction SilentlyContinue

    # Filtering & Cleaning
    $dateRegex = '^\d{1,2}\s+[A-Za-z]+\s+\d{4}\s+(at|pukul)\s+\d{1,2}[.:]\d{2}'
    $timeRegex = '^[lu0-9OIl]{1,2}[.:][0-9OIl]{2}\b'
    $calloutRegex = '^(Cut|Copy|Paste|Delete|Select|Select All|Look Up|Translate|Share\.\.\.|No Replacements Found|Device Hub|This device was viewed remotely|Co|py|Pas|te|>\s*|\.\.\.)$'

    $lines = @()
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
        if ($t) { $lines += $t }
    }

    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    if ($lines.Count -gt 0) {
        $resultText = $lines -join "`r`n"
        [Console]::WriteLine($resultText)
    }
} catch {
    # Fail silently without breaking server
}
