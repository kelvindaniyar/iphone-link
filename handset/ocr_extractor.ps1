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
    $bmp = [System.Drawing.Bitmap]::FromFile($fullPath)
    $storageFile = Await-Op ([Windows.Storage.StorageFile]::GetFileFromPathAsync($fullPath)) ([Windows.Storage.StorageFile])
    $stream = Await-Op ($storageFile.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
    $decoder = Await-Op ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
    $softwareBitmap = Await-Op ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
    
    $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
    if ($null -eq $engine) {
        $bmp.Dispose()
        $stream.Dispose()
        exit 0
    }

    $ocrResult = Await-Op ($engine.RecognizeAsync($softwareBitmap)) ([Windows.Media.Ocr.OcrResult])

    $highlighted = @()
    $content = @()

    foreach ($line in $ocrResult.Lines) {
        $minX = 999999
        $maxX = 0
        $minY = 999999
        $maxY = 0

        foreach ($w in $line.Words) {
            $r = $w.BoundingRect
            if ($r.X -lt $minX) { $minX = [int]$r.X }
            if (($r.X + $r.Width) -gt $maxX) { $maxX = [int]($r.X + $r.Width) }
            if ($r.Y -lt $minY) { $minY = [int]$r.Y }
            if (($r.Y + $r.Height) -gt $maxY) { $maxY = [int]($r.Y + $r.Height) }
        }

        $minX = [Math]::Max(0, $minX)
        $maxX = [Math]::Min($bmp.Width - 1, $maxX)
        $minY = [Math]::Max(0, $minY)
        $maxY = [Math]::Min($bmp.Height - 1, $maxY)

        $yellowVotes = 0
        $blueVotes = 0
        $samples = 0

        for ($y = $minY; $y -le $maxY; $y += 2) {
            for ($x = $minX; $x -le $maxX; $x += 4) {
                $p = $bmp.GetPixel($x, $y)
                # Skip text and dark pixels
                if ($p.R -lt 70 -and $p.G -lt 70 -and $p.B -lt 70) { continue }
                $samples++
                # Yellow highlight: R > 180, G > 150, R - B >= 18
                if (($p.R -gt 180) -and ($p.G -gt 150) -and ($p.R - $p.B -ge 18)) { $yellowVotes++ }
                # Blue highlight: B > 120, B - R >= 18
                if (($p.B -gt 120) -and ($p.B - $p.R -ge 18)) { $blueVotes++ }
            }
        }

        $isHigh = ($samples -gt 0) -and ((($yellowVotes / $samples) -gt 0.20) -or (($blueVotes / $samples) -gt 0.20))
        if ($isHigh) {
            $highlighted += $line.Text
        }
        $midY = ($minY + $maxY) / 2
        # Exclude header/status bar (< 14% H) and bottom navigation (> 85% H)
        if ($midY -gt (0.14 * $bmp.Height) -and $midY -lt (0.85 * $bmp.Height)) {
            $content += $line.Text
        }
    }

    $bmp.Dispose()
    $stream.Dispose()

    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    if ($highlighted.Count -gt 0) {
        $resultText = $highlighted -join "`r`n"
        [Console]::WriteLine($resultText)
    } elseif ($content.Count -gt 0) {
        $resultText = $content -join "`r`n"
        [Console]::WriteLine($resultText)
    }
} catch {
    # Fail silently without breaking server
}
