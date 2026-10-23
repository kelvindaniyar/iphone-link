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

    # A. Strict Content Zone Crop: ignore status bar clock & top buttons (Y < 16%) and bottom toolbars (Y > 84%)
    $topY = [int]($H * 0.16)
    $bottomY = [int]($H * 0.84)

    # B. Scan for highlight bounding box (Notes Yellow or Standard Blue)
    $minX = 999999
    $maxX = 0
    $minY = 999999
    $maxY = 0
    $foundHighlight = $false

    $stepY = [Math]::Max(2, [int]($H / 600))
    $stepX = [Math]::Max(2, [int]($W / 400))

    for ($y = $topY; $y -le $bottomY; $y += $stepY) {
        for ($x = 0; $x -lt $W; $x += $stepX) {
            $p = $srcBmp.GetPixel($x, $y)
            $isYellow = ($p.R -gt 200) -and ($p.G -gt 170) -and (($p.R - $p.B) -ge 25)
            $isBlue = ($p.B -gt 140) -and (($p.B - $p.R) -ge 25)
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
        # Tight crop: padY is only 4px so popup balloon below is NOT included
        $padY = [Math]::Max(2, [int]($H * 0.003))
        $padX = [Math]::Max(4, [int]($W * 0.01))
        $cropX = [Math]::Max(0, $minX - $padX)
        $cropY = [Math]::Max($topY, $minY - $padY)
        $cropW = [Math]::Min($W - $cropX, ($maxX - $minX) + ($padX * 2))
        $cropH = [Math]::Min($bottomY - $cropY, ($maxY - $minY) + ($padY * 2))
    } else {
        $cropX = 0
        $cropY = $topY
        $cropW = $W
        $cropH = $bottomY - $topY
    }

    # C. Soft Grayscale Contrast Stretch & Red Underline / Yellow Neutralization (NO HARSH BINARIZATION)
    $margin = [Math]::Max(16, [int]($cropH * 0.15))
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
            
            # 1. Red / Orange Spellcheck Underline
            $isRedSpellcheck = ($p.R -gt 150) -and (($p.R - $p.G) -gt 28) -and (($p.R - $p.B) -gt 20)
            
            # 2. Yellow Highlight or Lollipop handle
            $isYellowBg = (($p.R - $p.B) -ge 18) -and ($p.R -gt 170) -and ($p.G -gt 140)
            $isBlueBg = (($p.B - $p.R) -ge 18) -and ($p.B -gt 120)
            $isBright = $lum -gt 195

            if ($isRedSpellcheck -or $isYellowBg -or $isBlueBg -or $isBright) {
                $cleanBmp.SetPixel($cx + $margin, $cy + $margin, [System.Drawing.Color]::White)
            } else {
                # 3. Text pixels: Preserve anti-aliased font strokes with soft contrast stretch
                $gray = [Math]::Max(0, [Math]::Min(255, [int](($lum - 40) * 255.0 / (200 - 40))))
                $cleanBmp.SetPixel($cx + $margin, $cy + $margin, [System.Drawing.Color]::FromArgb($gray, $gray, $gray))
            }
        }
    }

    $tempProcessed = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), 'ocr_proc_' + [System.Guid]::NewGuid().ToString() + '.png')
    $cleanBmp.Save($tempProcessed, [System.Drawing.Imaging.ImageFormat]::Png)

    $cleanBmp.Dispose()
    $srcBmp.Dispose()

    # D. WinRT OCR Execution
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

    # E. Filtering & Cleaning
    $dateRegex = '^\d{1,2}\s+[A-Za-z]+\s+\d{4}\s+(at|pukul)\s+\d{1,2}[.:]\d{2}'
    $timeRegex = '^[lu0-9OIl]{1,2}[.:][0-9OIl]{2}\b'
    $calloutRegex = '^(Cut|Copy|Paste|Delete|Select|Select All|Look Up|Translate|Share\.\.\.|No Replacements Found|>\s*)$'

    $lines = @()
    foreach ($l in $ocrResult.Lines) {
        $t = $l.Text.Trim()
        if (!$t) { continue }
        if ($t -match $dateRegex -or $t -match $timeRegex) { continue }
        if ($t -match $calloutRegex) { continue }
        if ($t -match '^[<✓>]+$' -or $t -eq 'Notes' -or $t -eq 'Catatan' -or $t -eq 'Done' -or $t -eq 'Selesai') { continue }
        # Remove embedded callout words if any
        $t = $t -replace '\b(Cut|Copy|Paste|Delete)\b', ''
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
