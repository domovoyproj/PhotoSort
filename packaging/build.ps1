$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
python -m unittest discover -s tests -v
if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
python -m PyInstaller --noconfirm PhotoSort.spec
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
python packaging\collect_licenses.py
if ($LASTEXITCODE -ne 0) { throw 'License collection failed' }
$compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
if (-not (Test-Path $compiler)) { $compiler = Join-Path (Get-Location).Path '.photosort\tools\inno\ISCC.exe' }
if (Test-Path $compiler) {
    & $compiler packaging\installer.iss
    if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' }
}
Compress-Archive -Path dist\PhotoSort\* -DestinationPath dist\PhotoSort-0.1.0-windows-x64.zip -Force
