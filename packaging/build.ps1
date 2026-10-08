param([string]$Toolchain = '', [switch]$SkipAssets)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$cargoArgs = @()
if (-not $Toolchain -and (Test-Path '.photosort\tools\mingw\mingw64\bin\gcc.exe')) {
    $Toolchain = 'stable-x86_64-pc-windows-gnu'
    $env:PATH = (Join-Path (Get-Location).Path '.photosort\tools\mingw\mingw64\bin') + ';' + $env:PATH
}
if ($Toolchain) { $cargoArgs += ('+' + $Toolchain) }
& .\packaging\prepare-runtime.ps1
& cargo @cargoArgs test --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed' }
node --check photosort/web/app.js
if ($LASTEXITCODE -ne 0) { throw 'Frontend validation failed' }
& cargo @cargoArgs build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
if (-not $SkipAssets) { & .\packaging\fetch-assets.ps1 }
$distribution = Join-Path (Get-Location).Path 'dist\PhotoSort-0.2.1'
New-Item -ItemType Directory -Force $distribution | Out-Null
Copy-Item -LiteralPath target\release\photosort.exe -Destination (Join-Path $distribution 'PhotoSort.exe')
Copy-Item -LiteralPath target\release\WebView2Loader.dll -Destination $distribution
Copy-Item -LiteralPath .photosort\tools\MicrosoftEdgeWebview2Setup.exe -Destination $distribution
New-Item -ItemType Directory -Force (Join-Path $distribution 'codecs') | Out-Null
$codecFiles = Get-ChildItem -LiteralPath .photosort\tools\magick -File | Where-Object { $_.Name -eq 'magick.exe' -or $_.Extension -in '.xml','.icc' -or $_.Name -in 'LICENSE.txt','NOTICE.txt' }
foreach ($file in $codecFiles) { Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $distribution 'codecs') -Force }
New-Item -ItemType Directory -Force (Join-Path $distribution 'vision') | Out-Null
Copy-Item -Path .photosort\tools\vision\* -Destination (Join-Path $distribution 'vision') -Recurse -Force
& .\packaging\rust-licenses.ps1 -Destination (Join-Path $distribution 'licenses')
$compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
if (-not (Test-Path $compiler)) { $compiler = Join-Path (Get-Location).Path '.photosort\tools\inno\ISCC.exe' }
if (Test-Path $compiler) { & $compiler /Qp packaging\installer.iss; if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' } }
Compress-Archive -Path (Join-Path $distribution '*') -DestinationPath dist\PhotoSort-0.2.1-windows-x64.zip -Force
Get-FileHash -Algorithm SHA256 dist\PhotoSort-Setup-0.2.1-windows-x64.exe,dist\PhotoSort-0.2.1-windows-x64.zip | ForEach-Object { $_.Hash.ToLower() + '  ' + (Split-Path $_.Path -Leaf) } | Set-Content -Encoding ascii dist\SHA256SUMS-0.2.1.txt
