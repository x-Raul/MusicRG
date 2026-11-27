# Arranca MusicRG desde el código para probarlo.
#
# Por defecto en modo desarrollo (cargo tauri dev): compila en debug, abre la
# app y la recompila y reinicia sola cada vez que guardas algo en src-tauri.
# Con -Release compila la versión optimizada (sin instalador) y la abre, para
# ver cómo va de verdad.
#
# Necesita la CLI de Tauri desde crates.io (no hace falta npm):
#   cargo install tauri-cli --version "^2" --locked
#
# Uso (desde la raíz del repo):
#   powershell -File scripts/run.ps1            # desarrollo, con recarga
#   powershell -File scripts/run.ps1 -Release   # build optimizada

param([switch]$Release)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tauri = Join-Path $root "src-tauri"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "No encuentro cargo. Instala Rust desde https://rustup.rs"
}
cargo tauri --version *> $null
if ($LASTEXITCODE -ne 0) {
    throw 'Falta la CLI de Tauri. Instálala con: cargo install tauri-cli --version "^2" --locked'
}

# Si ya hay una copia abierta, el icono de bandeja y los atajos chocan.
$running = Get-Process musicrg -ErrorAction SilentlyContinue
if ($running) {
    Write-Host "Cerrando la copia de MusicRG que ya estaba abierta..."
    $running | Stop-Process -Force
    # Espera a que suelte el candado de instancia única; si no, la nueva copia
    # cree que ya hay una abierta y se cierra nada más arrancar.
    $running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
}

Push-Location $root
try {
    if ($Release) {
        cargo tauri build --no-bundle
        if ($LASTEXITCODE -ne 0) { throw "La compilación falló." }
        $exe = Join-Path $tauri "target\release\musicrg.exe"
        Write-Host "Abriendo $exe"
        Start-Process $exe
    } else {
        Write-Host "Modo desarrollo: Ctrl+C aquí para cerrar la app."
        cargo tauri dev
    }
} finally {
    Pop-Location
}
