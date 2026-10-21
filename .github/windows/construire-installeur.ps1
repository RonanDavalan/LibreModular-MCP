# Construit l'installeur Windows de LibreModular-MCP sur une machine Windows.
# Rend l installeur et l archive libremodular-mcp-<version>-windows.zip (l installeur seul), puis leurs empreintes SHA-256.
# Usage : powershell -NoProfile -ExecutionPolicy Bypass -File construire-installeur.ps1 -Source <arbre du depot de code> -Out <dossier de sortie> -Makensis <makensis.exe>
# Le chemin de l'arbre, de la cible Cargo et de TEMP doit etre ASCII : le linker de MinGW ne lit pas un profil accentue.
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Out,
    [Parameter(Mandatory = $true)][string]$Makensis
)
$ErrorActionPreference = 'Continue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $Source
$version = (Select-String -Path 'Cargo.toml' -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$release = Join-Path $env:CARGO_TARGET_DIR 'release'
$payload = Join-Path $Out 'payload'
Remove-Item -Recurse -Force $payload -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $payload 'bridges'), (Join-Path $payload 'docs') | Out-Null
Copy-Item (Join-Path $release 'libremodular-mcp.exe'), (Join-Path $release 'libremodular-launch.exe') $payload
Copy-Item 'LICENSE' (Join-Path $payload 'LICENSE.txt')
Copy-Item 'bridges\generated\*.py' (Join-Path $payload 'bridges')
foreach ($name in 'README.md', 'SECURITY.md', 'CONTRIBUTORS.md', 'CHANGELOG.md') {
    Copy-Item $name (Join-Path $payload 'docs')
}
Get-ChildItem 'docs' -Recurse -File | Where-Object { $_.Name -notlike '*.1.md' } | ForEach-Object {
    $relative = $_.FullName.Substring((Resolve-Path 'docs').Path.Length + 1)
    $target = Join-Path (Join-Path $payload 'docs') $relative
    New-Item -ItemType Directory -Force (Split-Path -Parent $target) | Out-Null
    Copy-Item $_.FullName $target
}
$installer = Join-Path $Out "libremodular-mcp-$version-setup.exe"
$icon = Join-Path $Source 'logo\favicon.ico'
& $Makensis -V1 "-DVERSION=$version" "-DPAYLOAD=$payload" "-DOUTFILE=$installer" "-DICON=$icon" (Join-Path $here 'libremodular-mcp.nsi')
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
# tar.exe et non Compress-Archive : celui de Windows PowerShell 5.1 ecrit des \ dans les noms d'entree du zip
$archive = Join-Path $Out "libremodular-mcp-$version-windows.zip"
Remove-Item -Force $archive -ErrorAction SilentlyContinue
tar.exe -a -c -f $archive -C $Out (Split-Path -Leaf $installer)
if ($LASTEXITCODE -ne 0) { throw "tar failed" }
Get-FileHash (Join-Path $payload 'libremodular-mcp.exe'), (Join-Path $payload 'libremodular-launch.exe'), $installer, $archive -Algorithm SHA256 | ForEach-Object { "{0}  {1}" -f $_.Hash.ToLower(), (Split-Path -Leaf $_.Path) }
