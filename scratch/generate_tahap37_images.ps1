Add-Type -AssemblyName System.Drawing

$projectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$keepFullResPath = Join-Path $projectRoot "test_google_keep_fullres.png"
$notesFullResPath = Join-Path $projectRoot "test_apple_notes_fullres.png"

$w = 1216
$h = 2656

$fontRegular = [System.Drawing.Font]::new("Arial", 28.0, [System.Drawing.FontStyle]::Regular)
$brushText = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(35, 35, 35))
$brushDebris = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(100, 100, 100))
$penSep = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(210, 210, 210), 3)

$stringFormat = New-Object System.Drawing.StringFormat
$stringFormat.Alignment = [System.Drawing.StringAlignment]::Near
$stringFormat.LineAlignment = [System.Drawing.StringAlignment]::Near

# ---------------------------------------------------------------------------
# 1. Generate Google Keep Full-Resolution Blue Selection PNG (1216 x 2656)
#    Crop height = 1800px (Y = 400 to 2200). Previously caused 4620px > 4096px overflow!
# ---------------------------------------------------------------------------
$bmpKeep = New-Object System.Drawing.Bitmap $w, $h
$gKeep = [System.Drawing.Graphics]::FromImage($bmpKeep)
$gKeep.Clear([System.Drawing.Color]::White)

# Unselected draft header above selection
$gKeep.DrawString("#toltangerangmerak #infotangerang", $fontRegular, $brushDebris, 75, 310)
$gKeep.DrawLine($penSep, 75, 375, 1140, 375)

# Blue selection block: Y = 400 to 2200, X = 65 to 1150
$brushBlueHl = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(210, 230, 255))
$gKeep.FillRectangle($brushBlueHl, 65, 400, 1085, 1800)

# Blue handle lollipops
$brushBlueHandle = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(30, 120, 255))
$gKeep.FillEllipse($brushBlueHandle, 55, 380, 45, 45)
$gKeep.FillEllipse($brushBlueHandle, 1120, 2180, 45, 45)

# Full 5 Paragraphs text for Google Keep
$textKeep = @"
Unggahan bertajuk 'Pesona Tol Balaraja Jam 11 Malam' yang dibagikan oleh akun kreator Instagram @kingtruck.id mendadak viral dan memicu gelombang komentar dari warganet.

Dalam unggahan tersebut, tampak potret seorang wanita berdiri di samping badan truk tronton hijau yang tengah berhenti di bahu jalan tol pada larut malam. Konten tersebut langsung menyedot puluhan ribu tanda suka dan memantik beragam spekulasi netizen di kolom komentar, mulai dari kelakar menanyakan titik kilometer ruas tol Merak–Jakarta hingga dugaan bahwa visual tersebut merupakan hasil olahan teknologi kecerdasan buatan (AI Generated Content). Fenomena ini memperlihatkan bagaimana dinamika kehidupan malam para pengemudi truk di jalur lintas barat Jawa kerap diangkat menjadi materi hiburan digital yang menarik atensi luas publik pengguna media sosial.

Bagaimana tanggapan kalian mengenai potret viral suasana malam di ruas Tol Balaraja ini, Banteners?

Source: @kingtruck.id

#tolbalaraja #tolmerakjakarta #kingtruckid #sopirtruk #infotangerang
"@

$rectKeep = New-Object System.Drawing.RectangleF 85, 420, 1045, 1760
$gKeep.DrawString($textKeep, $fontRegular, $brushText, $rectKeep, $stringFormat)
$gKeep.Dispose()
$bmpKeep.Save($keepFullResPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmpKeep.Dispose()
Write-Output "Created: $keepFullResPath (1216 x 2656, CropH = 1800px)"


# ---------------------------------------------------------------------------
# 2. Generate Apple Notes Full-Resolution Yellow Selection PNG (1216 x 2656)
#    Crop height = 1880px (Y = 340 to 2220).
# ---------------------------------------------------------------------------
$bmpNotes = New-Object System.Drawing.Bitmap $w, $h
$gNotes = [System.Drawing.Graphics]::FromImage($bmpNotes)
$gNotes.Clear([System.Drawing.Color]::White)

# Yellow selection block: Y = 340 to 2220, X = 65 to 1150
$brushYellowHl = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(253, 245, 205))
$gNotes.FillRectangle($brushYellowHl, 65, 340, 1085, 1880)

# Yellow handle lollipops
$brushYellowHandle = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(245, 175, 45))
$gNotes.FillEllipse($brushYellowHandle, 55, 320, 45, 45)
$gNotes.FillEllipse($brushYellowHandle, 1120, 2200, 45, 45)

# Full KMP Batumandi Caption with optical typo "d, tengah" to test recovery
$textNotes = @"
Rekaman video amatir yang memperlihatkan momen menegangkan evakuasi para penumpang KMP Batumandi yang kandas di perairan Pulau Merak Besar, Cilegon, Banten. Peristiwa ini terjadi saat kapal ferry tersebut mengalami kendala teknis dalam pelayaran dari Bakauheni menuju Merak pada Rabu (22/10) malam.

Dalam video yang diabadikan oleh salah seorang penumpang @andribotox, tampak seorang ayah menggendong erat buah hatinya saat menuruni tali evakuasi dengan penuh kehati-hatian menuju kapal penyelamat (tugboat) milik nelayan dan tim SAR gabungan. Evakuasi berlangsung cukup dramatis dan berisiko tinggi karena ombak yang cukup besar serta posisi kapal yang miring akibat kandas di karang.

Momen menegangkan ini menuai beragam simpati dari warganet setelah diunggah ke media sosial. Banyak yang mengaku terharu sekaligus ngeri menyaksikan perjuangan sang ayah demi menyelamatkan anaknya di tengah laut lepas dalam kondisi gelap gulita.

Situasi darurat d, tengah kepulan asap tebal tersebut memaksa proses evakuasi dilakukan bertahap hingga seluruh penumpang berhasil diselamatkan tanpa adanya korban jiwa.

Bagaimana perasaan kalian melihat perjuangan seorang ayah di tengah evakuasi KMP Batumandi ini, Banteners?

Source: Threads @andribotox

#kmpbatumandi #evakuasikapal #pelabuhanmerak #selatsunda #cilegon
"@

$rectNotes = New-Object System.Drawing.RectangleF 85, 360, 1045, 1840
$gNotes.DrawString($textNotes, $fontRegular, $brushText, $rectNotes, $stringFormat)
$gNotes.Dispose()
$bmpNotes.Save($notesFullResPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmpNotes.Dispose()
Write-Output "Created: $notesFullResPath (1216 x 2656, CropH = 1880px)"
