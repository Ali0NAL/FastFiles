# FastFiles yayın betiği
#
# Kullanım (proje klasöründe):   .\release.ps1
#
# 1. Kurulum dosyasını derler (src-tauri\target\release\bundle\nsis\...)
# 2. Dijital imzalı güncelleme paketini üretir
# 3. İkisini de .\release\ klasörüne koyar
#
# Sonra: release\FastFiles-<sürüm>.ffupdate dosyasını kendi FastFiles penceresine sürükleyin.
# Güncelleme ağdaki diğer bilgisayarlara kendiliğinden yayılır.
#
# Sürüm numarasını değiştirmeyi unutmayın: src-tauri\tauri.conf.json ve src-tauri\Cargo.toml (ikisi aynı olmalı).

param(
    [string]$Key = "$env:USERPROFILE\.fastfiles-release\update-key.pk8"
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$confVersion = (Get-Content "$root\src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json).version
$cargoVersion = ([regex]::Match((Get-Content "$root\src-tauri\Cargo.toml" -Raw), '(?m)^version\s*=\s*"([^"]+)"')).Groups[1].Value
if ($confVersion -ne $cargoVersion) {
    throw "Sürüm uyuşmuyor: tauri.conf.json=$confVersion, Cargo.toml=$cargoVersion"
}
if (-not (Test-Path $Key)) {
    throw "İmza anahtarı bulunamadı: $Key"
}
$version = $confVersion
Write-Host "FastFiles $version derleniyor..." -ForegroundColor Cyan

Push-Location "$root\src-tauri"
# cargo bilgi mesajlarını stderr'e yazar; PowerShell 5.1 bunları hata sanmasın, çıkış koduna bakıyoruz.
$ErrorActionPreference = 'Continue'
try {
    cargo tauri build
    if ($LASTEXITCODE) { throw "Derleme başarısız" }
    cargo build --release --bin ff-release
    if ($LASTEXITCODE) { throw "ff-release derlenemedi" }

    $setup = "target\release\bundle\nsis\FastFiles_${version}_x64-setup.exe"
    $out = Join-Path $root "release"
    New-Item -ItemType Directory -Force $out | Out-Null
    Copy-Item $setup $out -Force
    $pkg = Join-Path $out "FastFiles-$version.ffupdate"
    .\target\release\ff-release.exe package $setup $version $Key $pkg
    if ($LASTEXITCODE) { throw "Paketleme başarısız" }
}
finally {
    Pop-Location
}

Write-Host ""
Write-Host "Hazır:" -ForegroundColor Green
Write-Host "  Elle kurulum      : release\FastFiles_${version}_x64-setup.exe"
Write-Host "  Ağ güncellemesi   : release\FastFiles-$version.ffupdate  (FastFiles penceresine sürükleyin)"
