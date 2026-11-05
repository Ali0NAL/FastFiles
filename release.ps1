# FastFiles yayın betiği — ofisteki tüm bilgisayarları güncellemek için tek komut
#
# Kullanım (proje klasöründe):
#   .\release.ps1 -Version 1.4.0     Sürümü 1.4.0 yapar, derler, imzalar ve ağa dağıtır
#   .\release.ps1                    Dosyalardaki mevcut sürümle derler ve dağıtır
#
# Yaptıkları:
# 1. (-Version verilirse) sürüm numarasını tauri.conf.json ve Cargo.toml'da günceller
# 2. Kurulum dosyasını derler ve gizli anahtarla imzalı güncelleme paketini üretir
# 3. İkisini .\release\ klasörüne koyar
# 4. Paketi bu bilgisayardaki FastFiles'a verir. FastFiles önce kendini günceller, sonra paketi
#    ağdaki herkese dağıtır. Her bilgisayar, süren aktarımı yoksa ~1 dakika içinde kendini kurar.
#    O an kapalı olan bilgisayarlar açılınca güncellemeyi alır.

param(
    [string]$Version,
    [string]$Key = "$env:USERPROFILE\.fastfiles-release\update-key.pk8"
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$confPath = "$root\src-tauri\tauri.conf.json"
$cargoPath = "$root\src-tauri\Cargo.toml"
$cargoVersionRegex = '(?m)^version\s*=\s*"([^"]+)"'

$confVersion = (Get-Content $confPath -Raw -Encoding UTF8 | ConvertFrom-Json).version
$cargoVersion = ([regex]::Match((Get-Content $cargoPath -Raw -Encoding UTF8), $cargoVersionRegex)).Groups[1].Value
if ($confVersion -ne $cargoVersion) {
    throw "Sürüm uyuşmuyor: tauri.conf.json=$confVersion, Cargo.toml=$cargoVersion"
}
if (-not (Test-Path $Key)) {
    throw "İmza anahtarı bulunamadı: $Key"
}

if ($Version) {
    # Yalnızca X.Y.Z biçiminde ve mevcuttan büyük sürüm kabul edilir (uygulama eski sürümü kurmaz).
    if ($Version -notmatch '^\d+\.\d+\.\d+$') {
        throw "Sürüm X.Y.Z biçiminde olmalı (ör. 1.4.0), verilen: $Version"
    }
    if ([version]$Version -le [version]$confVersion) {
        throw "Yeni sürüm ($Version) mevcut sürümden ($confVersion) büyük olmalı"
    }
    $utf8 = New-Object System.Text.UTF8Encoding $false
    $conf = [IO.File]::ReadAllText($confPath)
    $conf = [regex]::Replace($conf, '"version"\s*:\s*"[^"]+"', "`"version`": `"$Version`"", 1)
    [IO.File]::WriteAllText($confPath, $conf, $utf8)
    $cargo = [IO.File]::ReadAllText($cargoPath)
    $cargo = ([regex]$cargoVersionRegex).Replace($cargo, "version = `"$Version`"", 1)
    [IO.File]::WriteAllText($cargoPath, $cargo, $utf8)
    Write-Host "Sürüm $confVersion -> $Version yapıldı (tauri.conf.json, Cargo.toml)" -ForegroundColor Cyan
    $confVersion = $Version
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

# Paketi bu bilgisayardaki FastFiles'ın gelen kutusuna koy: uygulama imzayı doğrulayıp içe alır,
# kendini günceller ve paketi ağdaki herkese dağıtır. Elle sürüklemeye gerek kalmaz.
$appData = Join-Path $env:APPDATA "io.twinup.fastfiles"
if (Test-Path $appData) {
    $inbox = Join-Path $appData "updates\inbox"
    New-Item -ItemType Directory -Force $inbox | Out-Null
    Copy-Item $pkg $inbox -Force
    Write-Host ""
    Write-Host "Paket FastFiles'a verildi. Bu bilgisayar ~1 dakika içinde $version'a geçer," -ForegroundColor Green
    Write-Host "ardından güncelleme ağdaki tüm FastFiles'lara kendiliğinden yayılır." -ForegroundColor Green
    Write-Host "Kimin güncellendiğini cihaz kartlarındaki sürüm numarasından görebilirsiniz."
} else {
    Write-Host ""
    Write-Host "Bu bilgisayarda FastFiles kurulu değil: paketi dağıtmak için" -ForegroundColor Yellow
    Write-Host "release\FastFiles-$version.ffupdate dosyasını bir FastFiles penceresine sürükleyin." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "Dosyalar:" -ForegroundColor Green
Write-Host "  Elle kurulum (eski sürümler / yeni bilgisayar): release\FastFiles_${version}_x64-setup.exe"
Write-Host "  Ağ güncellemesi                             : release\FastFiles-$version.ffupdate"
