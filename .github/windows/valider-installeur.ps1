# Palier 3 de la validation Windows : installation silencieuse, usage du binaire installe, retrait.
# Usage : powershell -NoProfile -ExecutionPolicy Bypass -File valider-installeur.ps1 -Installer <setup.exe> -Version <x.y.z> -Work <dossier ASCII jetable> [-Default]
# -Default : installation sans /D et emplacements par defaut (%LOCALAPPDATA%), pour un compte a nom accentue ; refuse de
# demarrer si %LOCALAPPDATA%\libremodular-mcp existe deja, et le supprime a la fin.
# Chaque ligne "OK ..." est un controle reussi ; une ligne "ECHEC ..." arrete le script avec un code non nul.
param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Work,
    [switch]$Default
)
$ErrorActionPreference = 'Continue'
function Check($ok, $text) { if ($ok) { "OK $text" } else { "ECHEC $text"; exit 1 } }
$data = Join-Path $Work 'data'
$install = if ($Default) { Join-Path $env:LOCALAPPDATA 'Programs\LibreModular-MCP' } else { Join-Path $Work 'installed' }
$ownData = Join-Path $env:LOCALAPPDATA 'libremodular-mcp'
if ($Default -and (Test-Path $ownData)) { 'ECHEC the user already has data in ' + $ownData; exit 1 }
Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Work, $data | Out-Null
$pathBefore = (Get-ItemProperty 'HKCU:\Environment' -Name Path -ErrorAction SilentlyContinue).Path
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\LibreModular-MCP'

Check (-not (Test-Path $uninstallKey)) 'no earlier installation'
$installArguments = if ($Default) { @('/S') } else { @('/S', "/D=$install") }
$run = Start-Process -FilePath $Installer -ArgumentList $installArguments -Wait -PassThru
Check ($run.ExitCode -eq 0) "silent install exits 0 ($($run.ExitCode))"
foreach ($file in 'libremodular-mcp.exe', 'libremodular-launch.exe', 'LICENSE.txt', 'uninstall.exe', 'bridges\uno_bridge.py', 'bridges\gimp_bridge.py', 'bridges\krita_bridge.py', 'bridges\blender_bridge.py', 'docs\README.md') {
    Check (Test-Path (Join-Path $install $file)) "installed file $file"
}
Check (-not (Test-Path (Join-Path $install 'docs\libremodular-mcp.1.md'))) 'manual page sources are not installed'
$pathAfter = (Get-ItemProperty 'HKCU:\Environment' -Name Path).Path
Check ($pathAfter -like "*$install*") 'the install directory is on the user PATH'
$entry = Get-ItemProperty $uninstallKey
Check ($entry.DisplayVersion -eq $Version) "uninstall entry carries the version ($($entry.DisplayVersion))"
Check ($entry.InstallLocation -eq $install) 'uninstall entry carries the install location'

$exe = Join-Path $install 'libremodular-mcp.exe'
$reported = & $exe --version
Check ($reported -eq "libremodular-mcp $Version") "--version is exact ($reported)"
if ($Default) {
    $dataRoot = $ownData
    Remove-Item Env:LIBREMODULAR_CONFIG_DIR, Env:LIBREMODULAR_STATE_DIR, Env:LIBREMODULAR_RUNTIME_DIR -ErrorAction SilentlyContinue
    $tokenFile = Join-Path $ownData 'config\token'
} else {
    $env:LIBREMODULAR_CONFIG_DIR = Join-Path $data 'config'
    $env:LIBREMODULAR_STATE_DIR = Join-Path $data 'state'
    $env:LIBREMODULAR_RUNTIME_DIR = Join-Path $data 'run'
    $tokenFile = Join-Path $data 'config\libremodular-mcp\token'
}
$initText = & $exe init 2>&1 | Out-String
Check ($LASTEXITCODE -eq 0) 'init exits 0'
Check (Test-Path $tokenFile) 'init created the token'
Check (-not ($initText -match '[0-9a-f]{64}')) 'init never prints the token'
$helpLaunch = & (Join-Path $install 'libremodular-launch.exe') libreoffice --stop 2>&1 | Out-String
Check ($LASTEXITCODE -eq 2 -and $helpLaunch -match 'not available on Windows') 'libremodular-launch is refused with exit 2 and its reason'

$serve = New-Object System.Diagnostics.ProcessStartInfo
$serve.FileName = $exe
$serve.Arguments = 'serve'
$serve.RedirectStandardInput = $true
$serve.RedirectStandardOutput = $true
$serve.RedirectStandardError = $true
$serve.UseShellExecute = $false
$process = [System.Diagnostics.Process]::Start($serve)
$init = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"check","version":"1"}}}'
$process.StandardInput.WriteLine($init)
$process.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized"}')
$process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":2,"method":"tools/list"}')
$first = $process.StandardOutput.ReadLine()
$second = $process.StandardOutput.ReadLine()
Check ($first -match 'libremodular') 'serve answers initialize'
Check (($second -match 'execute_script') -and ($second -match 'get_state')) 'serve lists execute_script and get_state'
$connections = @(Get-NetTCPConnection -OwningProcess $process.Id -ErrorAction SilentlyContinue | Where-Object { $_.State -ne 'Listen' -and $_.RemoteAddress -notin '127.0.0.1', '::1', '0.0.0.0', '::' })
Check ($connections.Count -eq 0) 'no external connection'
$process.StandardInput.Close()
$exited = $process.WaitForExit(10000)
Check $exited 'serve stops when its parent closes the input'

$run = Start-Process -FilePath (Join-Path $install 'uninstall.exe') -ArgumentList '/S', "_?=$install" -Wait -PassThru
Check ($run.ExitCode -eq 0) "silent uninstall exits 0 ($($run.ExitCode))"
Start-Sleep -Seconds 1
$left = @(Get-ChildItem -Recurse -Force $install | ForEach-Object { $_.Name })
Check ($left.Count -eq 1 -and $left[0] -eq 'uninstall.exe') "uninstall left nothing but its own executable ($($left -join ', '))"
Remove-Item -Force (Join-Path $install 'uninstall.exe') -ErrorAction SilentlyContinue
Remove-Item -Force $install -ErrorAction SilentlyContinue
Check (-not (Test-Path (Join-Path $install 'libremodular-mcp.exe'))) 'uninstall removed the program'
Check (-not (Test-Path $uninstallKey)) 'uninstall removed its entry'
$pathEnd = (Get-ItemProperty 'HKCU:\Environment' -Name Path -ErrorAction SilentlyContinue).Path
Check ($pathEnd -eq $pathBefore) 'the user PATH is back to its earlier value'
Check (Test-Path $tokenFile) 'uninstall left the token of the user'
if ($Default) { Remove-Item -Recurse -Force $ownData }
"VALIDATED $Version"
exit 0
