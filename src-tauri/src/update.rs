//! İmzalı güncelleme paketi (.ffupdate).
//!
//! Paket biçimi: `FFUPD1` (6 bayt) + u32 (big-endian) başlık uzunluğu + başlık JSON + kurulum dosyası.
//! Başlıktaki imza, yalnızca yayıncının elindeki Ed25519 gizli anahtarıyla üretilebilir.
//! Uygulamaya gömülü açık anahtarla doğrulanmayan paket hiçbir koşulda çalıştırılmaz.
//!
//! İmzalanan metin: "fastfiles-update-v1\n{sürüm}\n{boyut}\n{sha256}"
//! Böylece sürüm numarası da imzaya dahildir: eski bir paketin sürümü değiştirilerek
//! "yeni" gibi gösterilemez.

use ring::signature::{Ed25519KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Yayıncının açık anahtarı (hex). Gizli anahtar depoda DEĞİL, yayıncının bilgisayarındadır.
pub const PUBLIC_KEY_HEX: &str = "176eb116dc361791a54fd1b22b65dde7b584a4e6b53390aae122ab072dcd9ff2";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAGIC: &[u8; 6] = b"FFUPD1";
/// Kurulum dosyası için üst sınır (kötü niyetli dev paketlere karşı).
pub const MAX_INSTALLER_SIZE: u64 = 300 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageHeader {
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub signature: String,
}

pub fn public_key() -> Vec<u8> {
    hex::decode(PUBLIC_KEY_HEX).unwrap_or_default()
}

fn signed_message(version: &str, size: u64, sha256: &str) -> Vec<u8> {
    format!("fastfiles-update-v1\n{version}\n{size}\n{sha256}").into_bytes()
}

/// "1.2.10" → (1, 2, 10). Geçersizse None.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().split('.');
    let parse = |s: Option<&str>| s.and_then(|x| x.parse::<u64>().ok());
    let r = (parse(it.next())?, parse(it.next())?, parse(it.next())?);
    if it.next().is_some() {
        return None;
    }
    Some(r)
}

/// `candidate`, `current`'tan kesin olarak yeni mi? Geçersiz sürümler asla yeni sayılmaz.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        // Çalışan sürüm çözümlenemiyorsa (ör. "1.4.0-beta") hiçbir paket "daha yeni" sayılmaz;
        // yoksa eski imzalı paketler de kurulurdu (sürüm düşürme).
        _ => false,
    }
}

/// Başlığın imzasını ve sınırlarını doğrular (dosyayı indirmeden önce çağrılır).
pub fn verify_header(h: &PackageHeader, public_key: &[u8]) -> Result<(), String> {
    if parse_version(&h.version).is_none() {
        return Err("Geçersiz sürüm numarası".into());
    }
    if h.size == 0 || h.size > MAX_INSTALLER_SIZE {
        return Err("Geçersiz paket boyutu".into());
    }
    if h.sha256.len() != 64 || !h.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Geçersiz özet".into());
    }
    let sig = hex::decode(&h.signature).map_err(|_| "Geçersiz imza".to_string())?;
    if public_key.len() != 32 {
        return Err("Uygulamada güncelleme anahtarı yok".into());
    }
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&signed_message(&h.version, h.size, &h.sha256), &sig)
        .map_err(|_| "İmza doğrulanamadı: bu paket yetkili yayıncıdan gelmiyor".to_string())
}

/// Paketi okur, imzayı ve içeriğin özetini doğrular. Kurulum dosyasının başladığı ofseti döndürür.
pub fn open_verified(path: &Path, public_key: &[u8]) -> Result<(PackageHeader, u64), String> {
    let mut f = File::open(path).map_err(|e| format!("Paket açılamadı: {e}"))?;
    let mut magic = [0u8; 6];
    f.read_exact(&mut magic).map_err(|_| "Bu bir FastFiles güncelleme paketi değil".to_string())?;
    if &magic != MAGIC {
        return Err("Bu bir FastFiles güncelleme paketi değil".into());
    }
    let mut len = [0u8; 4];
    f.read_exact(&mut len).map_err(|_| "Bozuk paket".to_string())?;
    let len = u32::from_be_bytes(len) as usize;
    if len > 64 * 1024 {
        return Err("Bozuk paket".into());
    }
    let mut head = vec![0u8; len];
    f.read_exact(&mut head).map_err(|_| "Bozuk paket".to_string())?;
    let header: PackageHeader = serde_json::from_slice(&head).map_err(|_| "Bozuk paket başlığı".to_string())?;
    verify_header(&header, public_key)?;

    let offset = (6 + 4 + len) as u64;
    let total = f.metadata().map_err(|e| e.to_string())?.len();
    if total != offset + header.size {
        return Err("Paket boyutu uyuşmuyor (eksik veya fazla veri)".into());
    }
    let digest = sha256_of(&mut f, header.size).map_err(|e| e.to_string())?;
    if digest != header.sha256 {
        return Err("Paket içeriği imzayla uyuşmuyor (dosya değiştirilmiş)".into());
    }
    Ok((header, offset))
}

fn sha256_of(r: &mut impl Read, mut size: u64) -> io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let cap = buf.len() as u64;
    while size > 0 {
        let n = r.read(&mut buf[..size.min(cap) as usize])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eksik veri"));
        }
        h.update(&buf[..n]);
        size -= n as u64;
    }
    Ok(hex::encode(h.finalize()))
}

/// Paket başlığını yazar (kurulum baytları ardından eklenir).
pub fn write_header(w: &mut impl Write, h: &PackageHeader) -> io::Result<()> {
    let head = serde_json::to_vec(h).map_err(io::Error::other)?;
    w.write_all(MAGIC)?;
    w.write_all(&(head.len() as u32).to_be_bytes())?;
    w.write_all(&head)
}

/// Doğrulanmış paketten kurulum dosyasını çıkarır ve çıkarılanı yeniden doğrular.
pub fn extract_installer(package: &Path, out: &Path, public_key: &[u8]) -> Result<PackageHeader, String> {
    let (header, offset) = open_verified(package, public_key)?;
    let mut src = File::open(package).map_err(|e| e.to_string())?;
    src.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut dst = File::create(out).map_err(|e| format!("Kurulum dosyası yazılamadı: {e}"))?;
    io::copy(&mut (&mut src).take(header.size), &mut dst).map_err(|e| e.to_string())?;
    drop(dst);
    let mut check = File::open(out).map_err(|e| e.to_string())?;
    if sha256_of(&mut check, header.size).map_err(|e| e.to_string())? != header.sha256 {
        let _ = std::fs::remove_file(out);
        return Err("Çıkarılan kurulum dosyası doğrulanamadı".into());
    }
    Ok(header)
}

// ───────────── Yayıncı tarafı (ff-release aracı ve testler kullanır) ─────────────

pub fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>), String> {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "Anahtar üretilemedi".to_string())?;
    let kp = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "Anahtar okunamadı".to_string())?;
    Ok((pkcs8.as_ref().to_vec(), ring::signature::KeyPair::public_key(&kp).as_ref().to_vec()))
}

pub fn public_key_of(pkcs8: &[u8]) -> Result<Vec<u8>, String> {
    let kp = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| "Geçersiz gizli anahtar".to_string())?;
    Ok(ring::signature::KeyPair::public_key(&kp).as_ref().to_vec())
}

/// Kurulum dosyasından imzalı .ffupdate paketi üretir.
pub fn build_package(installer: &Path, version: &str, pkcs8: &[u8], out: &Path) -> Result<PackageHeader, String> {
    if parse_version(version).is_none() {
        return Err(format!("Geçersiz sürüm: {version}"));
    }
    let kp = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| "Geçersiz gizli anahtar".to_string())?;
    let size = std::fs::metadata(installer).map_err(|e| e.to_string())?.len();
    let mut f = File::open(installer).map_err(|e| e.to_string())?;
    let sha256 = sha256_of(&mut f, size).map_err(|e| e.to_string())?;
    let signature = hex::encode(kp.sign(&signed_message(version, size, &sha256)).as_ref());
    let header = PackageHeader { version: version.to_string(), size, sha256, signature };

    let mut w = io::BufWriter::new(File::create(out).map_err(|e| e.to_string())?);
    write_header(&mut w, &header).map_err(|e| e.to_string())?;
    let mut f = File::open(installer).map_err(|e| e.to_string())?;
    io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("1.2.0", "1.1.9"));
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.0", "1.2.0"));
        assert!(!is_newer("1.2", "1.1.0"));
        assert!(!is_newer("abc", "1.0.0"));
        assert!(!is_newer("1.2.0.1", "1.0.0"));
        // Çalışan sürüm çözümlenemiyorsa hiçbir paket daha yeni sayılmaz (sürüm düşürme olmasın).
        assert!(!is_newer("0.0.1", "1.4.0-beta"));
        assert!(!is_newer("9.9.9", ""));
    }

    #[test]
    fn package_roundtrip_and_tamper_detection() {
        let dir = tempfile::tempdir().unwrap();
        let (sk, pk) = generate_keypair().unwrap();
        let (_, other_pk) = generate_keypair().unwrap();
        let installer = dir.path().join("setup.exe");
        std::fs::write(&installer, vec![0xABu8; 200_000]).unwrap();
        let pkg = dir.path().join("p.ffupdate");
        build_package(&installer, "2.0.0", &sk, &pkg).unwrap();

        // Doğru anahtarla geçer, çıkarılan dosya aynıdır.
        let (h, _) = open_verified(&pkg, &pk).unwrap();
        assert_eq!(h.version, "2.0.0");
        let out = dir.path().join("out.exe");
        extract_installer(&pkg, &out, &pk).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&installer).unwrap());

        // Başka bir anahtarla imzalanmış gibi davranan paket reddedilir.
        assert!(open_verified(&pkg, &other_pk).unwrap_err().contains("İmza"));

        // İçeriğin tek baytı değişirse reddedilir.
        let mut bytes = std::fs::read(&pkg).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&pkg, &bytes).unwrap();
        assert!(open_verified(&pkg, &pk).unwrap_err().contains("değiştirilmiş"));

        // Sürüm numarası değiştirilirse imza tutmaz.
        let mut h2 = h.clone();
        h2.version = "9.0.0".into();
        assert!(verify_header(&h2, &pk).is_err());

        // Paket olmayan dosya reddedilir.
        assert!(open_verified(&installer, &pk).is_err());
    }
}
