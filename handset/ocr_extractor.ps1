param(
    [Parameter(Mandatory=$true)]
    [string]$ImagePath
)

$ErrorActionPreference = "SilentlyContinue"

try {
    Add-Type -AssemblyName 'System.Runtime.WindowsRuntime'
    Add-Type -AssemblyName 'System.Drawing'
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
    $W = $srcBmp.Width
    $H = $srcBmp.Height

    # A. Strict Content Zone Crop: ignore status bar clock & header buttons (Y < 15%) and toolbars/home bar (Y > 86%)
    $topY = [int]($H * 0.15)
    $bottomY = [int]($H * 0.86)

    # B. Scan for highlight bounding box in content area
    $minX = 999999
    $maxX = 0
    $minY = 999999
    $maxY = 0
    $foundHighlight = $false

    for ($y = $topY; $y -le $bottomY; $y += 2) {
        for ($x = 0; $x -lt $W; $x += 3) {
            $p = $srcBmp.GetPixel($x, $y)
            $isYellow = ($p.R -gt 195) -and ($p.G -gt 165) -and (($p.R - $p.B) -ge 22)
            $isBlue = ($p.B -gt 130) -and (($p.B - $p.R) -ge 22)
            if ($isYellow -or $isBlue) {
                $foundHighlight = $true
                if ($x -lt $minX) { $minX = $x }
                if ($x -gt $maxX) { $maxX = $x }
                if ($y -lt $minY) { $minY = $y }
                if ($y -gt $maxY) { $maxY = $y }
            }
        }
    }

    if ($foundHighlight -and (($maxX - $minX) -ge 20)) {
        $cropX = [Math]::Max(0, $minX - 16)
        $cropY = [Math]::Max($topY, $minY - 10)
        $cropW = [Math]::Min($W - $cropX, ($maxX - $minX) + 32)
        $cropH = [Math]::Min($bottomY - $cropY, ($maxY - $minY) + 20)
    } else {
        $cropX = 0
        $cropY = $topY
        $cropW = $W
        $cropH = $bottomY - $topY
    }

    # C. Color Neutralization / Binarization:
    # Vaporize yellow highlight, handles, and bright background to pure white. Dark text becomes crisp black.
    $margin = 24
    $cleanW = $cropW + ($margin * 2)
    $cleanH = $cropH + ($margin * 2)
    $cleanBmp = New-Object System.Drawing.Bitmap($cleanW, $cleanH)
    $gClean = [System.Drawing.Graphics]::FromImage($cleanBmp)
    $gClean.Clear([System.Drawing.Color]::White)
    $gClean.Dispose()

    for ($cy = 0; $cy -lt $cropH; $cy++) {
        for ($cx = 0; $cx -lt $cropW; $cx++) {
            $p = $srcBmp.GetPixel($cropX + $cx, $cropY + $cy)
            $lum = [int](0.299 * $p.R + 0.587 * $p.G + 0.114 * $p.B)
            $isYellow = (($p.R - $p.B) -ge 18) -and ($p.R -gt 140)
            $isBlue = (($p.B - $p.R) -ge 18) -and ($p.B -gt 120)
            $isBright = $lum -gt 155
            if ($isYellow -or $isBlue -or $isBright) {
                $cleanBmp.SetPixel($cx + $margin, $cy + $margin, [System.Drawing.Color]::White)
            } else {
                $cleanBmp.SetPixel($cx + $margin, $cy + $margin, [System.Drawing.Color]::Black)
            }
        }
    }

    # D. Upscale 2.5x with HighQualityBicubic for maximum OCR sharpness
    $scale = 2.5
    $upW = [int]($cleanW * $scale)
    $upH = [int]($cleanH * $scale)
    $upBmp = New-Object System.Drawing.Bitmap($upW, $upH)
    $gUp = [System.Drawing.Graphics]::FromImage($upBmp)
    $gUp.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $gUp.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $gUp.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $gUp.Clear([System.Drawing.Color]::White)
    $gUp.DrawImage($cleanBmp, 0, 0, $upW, $upH)
    $gUp.Dispose()

    $tempProcessed = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), 'ocr_preproc_' + [System.Guid]::NewGuid().ToString() + '.png')
    $upBmp.Save($tempProcessed, [System.Drawing.Imaging.ImageFormat]::Png)

    $cleanBmp.Dispose()
    $upBmp.Dispose()
    $srcBmp.Dispose()

    # E. WinRT OCR Execution
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

    # F. Post-Processing & Filtering
    $dateRegex = '^\d{1,2}\s+[A-Za-z]+\s+\d{4}\s+(at|pukul)\s+\d{1,2}[.:]\d{2}'
    $timeRegex = '^\d{1,2}[.:]\d{2}$'
    $lines = @()

    foreach ($l in $ocrResult.Lines) {
        $t = $l.Text.Trim()
        if (!$t) { continue }
        if ($t -match $dateRegex -or $t -match $timeRegex) { continue }
        if ($t -match '^[<✓>]+$' -or $t -eq 'Notes' -or $t -eq 'Catatan' -or $t -eq 'Done' -or $t -eq 'Selesai') { continue }
        # Artifact cleanups
        $t = $t -replace '\brnessi\b', 'messi'
        $t = $t -replace '([A-Za-z0-9])\.$', '$1'
        $lines += $t
    }

    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    if ($lines.Count -gt 0) {
        $resultText = $lines -join "`r`n"
        [Console]::WriteLine($resultText)
    }
} catch {
    # Fail silently without breaking server
}
