# Compila el instalador firmado de MusicRG y deja todo lo de un release en
# publish/v<versión>/: el instalador, su firma (.sig) y latest.json.
#
# Es lo mismo que hace el workflow de GitHub con una etiqueta v*, pero en
# local y sin publicar nada. Necesita:
#   - la clave privada en %USERPROFILE%\.tauri\musicrg.key (la de siempre:
#     NO generes otra, las copias instaladas dejarían de actualizarse);
#   - la CLI de Tauri desde crates.io: cargo install tauri-cli --version "^2" --locked
#
# Uso (desde la raíz del repo):  powershell -File scripts/publish.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tauri = Join-Path $root "src-tauri"

# --- versión: tauri.conf.json manda; Cargo.toml tiene que coincidir --------
$ver = (Get-Content (Join-Path $tauri "tauri.conf.json") -Raw | ConvertFrom-Json).version
$cargoVer = (Select-String -Path (Join-Path $tauri "Cargo.toml") -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if ($cargoVer -ne $ver) {
    throw "La versión no coincide: tauri.conf.json = $ver, Cargo.toml = $cargoVer."
}

# --- clave de firma ---------------------------------------------------------
$key = Join-Path $env:USERPROFILE ".tauri\musicrg.key"
if (-not (Test-Path $key)) {
    throw "No encuentro la clave privada en $key. Sin ella el instalador no se puede auto-actualizar."
}
$pub = (Get-Content "$key.pub" -Raw).Trim()
$conf = (Get-Content (Join-Path $tauri "tauri.conf.json") -Raw | ConvertFrom-Json).plugins.updater.pubkey
if ($pub -ne $conf) {
    throw "La clave pública de $key.pub no es la de tauri.conf.json: las copias instaladas rechazarían este instalador."
}
if (-not (Get-Command cargo-tauri -ErrorAction SilentlyContinue)) {
    throw 'Falta la CLI de Tauri: cargo install tauri-cli --version "^2" --locked'
}

# --- compilar ---------------------------------------------------------------
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $key -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""  # la clave no tiene contraseña
Push-Location $tauri
try {
    cargo tauri build --config tauri.updater.json
    if ($LASTEXITCODE) { throw "cargo tauri build falló ($LASTEXITCODE)." }
} finally {
    Pop-Location
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}

# --- recoger el release -----------------------------------------------------
$exe = Get-Item (Join-Path $tauri "target\release\bundle\nsis\MusicRG_${ver}_*-setup.exe")
$sigFile = "$($exe.FullName).sig"
if (-not (Test-Path $sigFile)) { throw "No se generó la firma $sigFile." }

$out = Join-Path $root "publish\v$ver"
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $exe.FullName, $sigFile $out -Force

# latest.json: lo que consulta la app instalada. La URL apunta al release de
# GitHub con la etiqueta v<versión>, como el que crea el workflow.
$url = "https://github.com/x-Raul/MusicRG/releases/download/v$ver/$($exe.Name)"
[ordered]@{
    version   = $ver
    notes     = "MusicRG v$ver"
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = @{ "windows-x86_64" = @{ signature = (Get-Content $sigFile -Raw).Trim(); url = $url } }
} | ConvertTo-Json -Depth 5 | ForEach-Object {
    # UTF-8 sin BOM: Windows PowerShell 5.1 lo añade con -Encoding utf8 y el
    # lector JSON del actualizador no lo espera.
    [IO.File]::WriteAllText((Join-Path $out "latest.json"), $_, (New-Object Text.UTF8Encoding $false))
}

Write-Host "Listo: $out"
Get-ChildItem $out | Format-Table Name, Length -AutoSize
