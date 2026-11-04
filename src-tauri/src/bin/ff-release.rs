//! FastFiles yayın aracı.
//!
//!   ff-release keygen <anahtar.pk8>                      Yeni imza anahtarı üretir, açık anahtarı yazdırır
//!   ff-release pubkey <anahtar.pk8>                      Açık anahtarı yazdırır
//!   ff-release package <setup.exe> <sürüm> <anahtar.pk8> <çıktı.ffupdate>
//!
//! Gizli anahtarı (.pk8) ASLA depoya koymayın ve yedekleyin: kaybolursa yeni güncelleme yayınlanamaz.

use fastfiles_lib::update;
use std::path::Path;
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", out] => keygen(Path::new(out)),
        ["pubkey", key] => std::fs::read(key)
            .map_err(|e| e.to_string())
            .and_then(|k| update::public_key_of(&k))
            .map(|pk| println!("{}", hex::encode(pk))),
        ["package", installer, version, key, out] => package(installer, version, key, out),
        _ => {
            eprintln!("Kullanım:\n  ff-release keygen <anahtar.pk8>\n  ff-release pubkey <anahtar.pk8>\n  ff-release package <setup.exe> <sürüm> <anahtar.pk8> <çıktı.ffupdate>");
            exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("HATA: {e}");
        exit(1);
    }
}

fn keygen(out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!("{} zaten var; üzerine yazılmadı", out.display()));
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let (sk, pk) = update::generate_keypair()?;
    std::fs::write(out, sk).map_err(|e| e.to_string())?;
    println!("{}", hex::encode(pk));
    Ok(())
}

fn package(installer: &str, version: &str, key: &str, out: &str) -> Result<(), String> {
    let sk = std::fs::read(key).map_err(|e| format!("Anahtar okunamadı: {e}"))?;
    let h = update::build_package(Path::new(installer), version, &sk, Path::new(out))?;
    // Üretilen paketi uygulamanın kullanacağı açık anahtarla hemen doğrula.
    update::open_verified(Path::new(out), &update::public_key_of(&sk)?)?;
    if update::public_key() != update::public_key_of(&sk)? {
        return Err("Bu anahtar uygulamaya gömülü açık anahtarla eşleşmiyor!".into());
    }
    println!("Paket hazır: {out}\n  sürüm  {}\n  boyut  {} bayt\n  sha256 {}", h.version, h.size, h.sha256);
    Ok(())
}
