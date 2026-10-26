$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$destination=Join-Path $PSScriptRoot '../src/ThodHub/assets'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$images=@()
foreach($size in @(16,24,32,48,64,128,256)) {
    $bitmap=[Drawing.Bitmap]::new($size,$size)
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode='AntiAlias'
    $graphics.Clear([Drawing.Color]::FromArgb(54,118,232))
    $brush=[Drawing.SolidBrush]::new([Drawing.Color]::White)
    $unit=$size/8.0
    foreach($x in @(1.5,4.5)){foreach($y in @(1.5,4.5)){$graphics.FillRectangle($brush,[single]($unit*$x),[single]($unit*$y),[single]($unit*2),[single]($unit*2))}}
    $memory=[IO.MemoryStream]::new();$bitmap.Save($memory,[Drawing.Imaging.ImageFormat]::Png)
    $images+=,@{Size=$size;Bytes=$memory.ToArray()}
    $memory.Dispose();$graphics.Dispose();$bitmap.Dispose();$brush.Dispose()
}
$file=[IO.File]::Create((Join-Path $destination 'thodhub.ico'));$writer=[IO.BinaryWriter]::new($file)
try {
    $writer.Write([uint16]0);$writer.Write([uint16]1);$writer.Write([uint16]$images.Count)
    $offset=6+16*$images.Count
    foreach($image in $images){$s=if($image.Size-eq 256){0}else{$image.Size};$writer.Write([byte]$s);$writer.Write([byte]$s);$writer.Write([uint16]0);$writer.Write([uint16]1);$writer.Write([uint16]32);$writer.Write([uint32]$image.Bytes.Length);$writer.Write([uint32]$offset);$offset+=$image.Bytes.Length}
    foreach($image in $images){$writer.Write([byte[]]$image.Bytes)}
} finally {$writer.Dispose();$file.Dispose()}
