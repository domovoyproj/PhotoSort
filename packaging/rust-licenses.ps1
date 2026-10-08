param([string]$Destination)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Destination | Out-Null
$metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
$manifest = foreach ($package in $metadata.packages) {
    $package.name + ' ' + $package.version + ' — ' + $package.license
    $source = Split-Path $package.manifest_path -Parent
    $licenses = Get-ChildItem -LiteralPath $source -File | Where-Object Name -Match '^(LICENSE|COPYING|NOTICE)'
    if ($licenses) {
        $target = Join-Path $Destination ($package.name + '-' + $package.version)
        New-Item -ItemType Directory -Force $target | Out-Null
        foreach ($license in $licenses) { Copy-Item -LiteralPath $license.FullName -Destination $target }
    }
}
$manifest | Set-Content -Encoding utf8 (Join-Path $Destination 'COMPONENTS.txt')
