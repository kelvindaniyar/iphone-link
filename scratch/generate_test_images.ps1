Add-Type -AssemblyName System.Drawing

$projectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$keepPath = Join-Path $projectRoot "test_google_keep_blue.png"
$notesPath = Join-Path $projectRoot "test_apple_notes_yellow.png"

# 1. Generate Google Keep Blue Selection PNG
$w = 1170
$h = 2532
$bmpKeep = New-Object System.Drawing.Bitmap $w, $h
$gKeep = [System.Drawing.Graphics]::FromImage($bmpKeep)
$gKeep.Clear([System.Drawing.Color]::White)

$fontRegular = [System.Drawing.Font]::new("Arial", 28.0, [System.Drawing.FontStyle]::Regular)
$fontBold = [System.Drawing.Font]::new("Arial", 28.0, [System.Drawing.FontStyle]::Bold)

$brushText = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(35, 35, 35))
$brushDebris = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(100, 100, 100))
$penSep = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(210, 210, 210), 3)

# Unselected draft header above selection
$gKeep.DrawString("#toltangerangmerak #infotangerang", $fontRegular, $brushDebris, 70, 320)
$gKeep.DrawLine($penSep, 70, 390, 1100, 390)

# Blue selection block: Y = 440 to 1950, X = 60 to 1110
$brushBlueHl = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(200, 225, 252))
$gKeep.FillRectangle($brushBlueHl, 60, 440, 1050, 1510)

# Blue handle lollipops
$brushBlueHandle = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(30, 120, 255))
$gKeep.FillEllipse($brushBlueHandle, 50, 420, 40, 40)
$gKeep.FillEllipse($brushBlueHandle, 1080, 1930, 40, 40)

# News article text inside blue highlight
$textKeep = @"
Unggahan bertajuk 'Pesona Tol Balaraja Jam 11 Malam' yang dibagikan oleh akun kreator Instagram @kingtruck.id mendadak viral dan memicu gelombang komentar dari warganet.

Dalam unggahan tersebut, tampak potret seorang wanita berdiri di samping badan truk tronton hijau yang tengah berhenti di bahu jalan tol pada larut malam. Konten tersebut langsung menyedot puluhan ribu tanda suka dan memantik beragam spekulasi netizen di kolom komentar, mulai dari kelakar menanyakan titik kilometer ruas tol Merak–Jakarta hingga dugaan bahwa visual tersebut merupakan hasil olahan teknologi kecerdasan buatan (AI Generated Content). Fenomena ini memperlihatkan bagaimana dinamika kehidupan malam para pengemudi truk di jalur lintas barat Jawa kerap diangkat menjadi materi hiburan digital yang menarik atensi luas publik pengguna media sosial.

Bagaimana tanggapan kalian mengenai potret viral suasana malam di ruas Tol Balaraja ini, Banteners?

Source: @kingtruck.id

#tolbalaraja #tolmerakjakarta #kingtruckid #sopirtruk #infotangerang
"@

$rectKeep = New-Object System.Drawing.RectangleF 80, 460, 1010, 1470
$stringFormat = New-Object System.Drawing.StringFormat
$stringFormat.Alignment = [System.Drawing.StringAlignment]::Near
$stringFormat.LineAlignment = [System.Drawing.StringAlignment]::Near

$gKeep.DrawString($textKeep, $fontRegular, $brushText, $rectKeep, $stringFormat)
$gKeep.Dispose()
$bmpKeep.Save($keepPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmpKeep.Dispose()
Write-Output "Created: $keepPath"


# 2. Generate Apple Notes Yellow Selection PNG
$bmpNotes = New-Object System.Drawing.Bitmap $w, $h
$gNotes = [System.Drawing.Graphics]::FromImage($bmpNotes)
$gNotes.Clear([System.Drawing.Color]::White)

# Yellow selection block: Y = 460 to 1150
$brushYellowHl = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(253, 240, 185))
$gNotes.FillRectangle($brushYellowHl, 60, 460, 1050, 690)

# Yellow handle lollipops
$brushYellowHandle = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(245, 175, 45))
$gNotes.FillEllipse($brushYellowHandle, 50, 440, 40, 40)
$gNotes.FillEllipse($brushYellowHandle, 750, 1130, 40, 40)

# Short text with double Enter spacing
$rectNotes = New-Object System.Drawing.RectangleF 80, 480, 1010, 650
$textNotes = "pamungkas kenangan manis`r`n`r`nbarasuara`r`n`r`nthe beatles`r`n`r`nhindia"
$gNotes.DrawString($textNotes, $fontRegular, $brushText, $rectNotes, $stringFormat)

# Red spellcheck dots under "beatles"
$penSpellcheck = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(235, 60, 60), 3)
for ($x = 240; $x -le 350; $x += 6) {
    $gNotes.DrawLine($penSpellcheck, $x, 830, $x + 3, 830)
}

$gNotes.Dispose()
$bmpNotes.Save($notesPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmpNotes.Dispose()
Write-Output "Created: $notesPath"
