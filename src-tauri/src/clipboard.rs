//! "Panoyu gönder": panodaki kopyalanmış dosyaları, ekran görüntüsünü veya metni okur.
//!
//! Ekran görüntüsü PNG olarak, uzun metin .txt olarak geçici bir klasöre yazılır ve normal
//! dosya gibi gönderilir. Kısa metin sohbet mesajı olarak gider.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Geçici pano dosyaları bu süreden sonra silinir (gönderim onay bekliyor olabilir).
const CLIP_FILE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug)]
pub enum ClipContent {
    /// Gezgin'de kopyalanan dosya/klasörler.
    Files(Vec<String>),
    /// Ekran görüntüsü (PNG olarak kaydedildi).
    Image(PathBuf),
    Text(String),
}

/// Panoyu okur. Öncelik: kopyalanan dosyalar > görüntü > metin.
/// `stamp`: dosya adında kullanılacak yerel tarih-saat (arayüzden gelir, ör. "2026-11-05 14.32.10").
pub fn read(dir: &Path, stamp: &str) -> Result<ClipContent, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("Panoya erişilemedi: {e}"))?;
    if let Ok(files) = cb.get().file_list() {
        let files: Vec<String> = files.into_iter().filter(|p| p.exists()).map(|p| p.to_string_lossy().into_owned()).collect();
        if !files.is_empty() {
            return Ok(ClipContent::Files(files));
        }
    }
    if let Ok(img) = cb.get().image() {
        if img.width > 0 && img.height > 0 {
            let path = save_png(dir, stamp, img.width as u32, img.height as u32, &img.bytes)?;
            return Ok(ClipContent::Image(path));
        }
    }
    match cb.get().text() {
        Ok(t) if !t.trim().is_empty() => Ok(ClipContent::Text(t)),
        _ => Err("Pano boş ya da gönderilebilecek bir şey içermiyor".into()),
    }
}

/// Dosya adı için güvenli damga; boşsa zamandan türetilir.
fn clean_stamp(stamp: &str) -> String {
    let s: String = stamp.chars().filter(|c| c.is_ascii_digit() || matches!(c, '-' | '.' | ' ')).take(24).collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        let ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        ms.to_string()
    } else {
        s
    }
}

/// `dir` içinde var olmayan bir dosya adı seçer: "Ad.png", "Ad (2).png", ...
fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|i| dir.join(format!("{stem} ({i}).{ext}"))).find(|p| !p.exists()).unwrap()
}

fn prepare_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Geçici klasör oluşturulamadı: {e}"))?;
    cleanup(dir);
    Ok(())
}

/// Eski geçici pano dosyalarını siler.
pub fn cleanup(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map_or(false, |age| age > CLIP_FILE_TTL);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// RGBA görüntüyü PNG olarak kaydeder.
pub fn save_png(dir: &Path, stamp: &str, width: u32, height: u32, rgba: &[u8]) -> Result<PathBuf, String> {
    if rgba.len() != (width as usize) * (height as usize) * 4 {
        return Err("Panodaki görüntü okunamadı".into());
    }
    prepare_dir(dir)?;
    let path = free_path(dir, &format!("Ekran görüntüsü {}", clean_stamp(stamp)), "png");
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| format!("Görüntü kaydedilemedi: {e}"))?;
    w.write_image_data(rgba).map_err(|e| format!("Görüntü kaydedilemedi: {e}"))?;
    w.finish().map_err(|e| format!("Görüntü kaydedilemedi: {e}"))?;
    Ok(path)
}

/// Hazır PNG verisini (ör. üzerine çizilmiş ekran görüntüsü) kaydeder.
pub fn save_png_bytes(dir: &Path, stamp: &str, data: &[u8]) -> Result<PathBuf, String> {
    prepare_dir(dir)?;
    let path = free_path(dir, &format!("Ekran görüntüsü {}", clean_stamp(stamp)), "png");
    std::fs::write(&path, data).map_err(|e| format!("Görüntü kaydedilemedi: {e}"))?;
    Ok(path)
}

/// Mesaja sığmayan metni .txt dosyası olarak kaydeder (Windows satır sonlarıyla).
pub fn save_text(dir: &Path, stamp: &str, text: &str) -> Result<PathBuf, String> {
    prepare_dir(dir)?;
    let path = free_path(dir, &format!("Pano {}", clean_stamp(stamp)), "txt");
    let body = text.replace("\r\n", "\n").replace('\n', "\r\n");
    // UTF-8 BOM: Not Defteri Türkçe karakterleri doğru göstersin.
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(body.as_bytes());
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_and_text_files_are_written() {
        let tmp = tempfile::tempdir().unwrap();
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| i as u8).collect();
        let p = save_png(tmp.path(), "2026-11-05 14.32.10", 3, 2, &rgba).unwrap();
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "Ekran görüntüsü 2026-11-05 14.32.10.png");
        let p2 = save_png(tmp.path(), "2026-11-05 14.32.10", 3, 2, &rgba).unwrap();
        assert_eq!(p2.file_name().unwrap().to_string_lossy(), "Ekran görüntüsü 2026-11-05 14.32.10 (2).png");
        let data = std::fs::read(&p).unwrap();
        assert_eq!(&data[1..4], b"PNG");
        assert!(save_png(tmp.path(), "x", 3, 3, &rgba).is_err(), "boyut tutmayan veri reddedilmeli");

        let t = save_text(tmp.path(), "../../evil", "satır 1\nşğüöçı").unwrap();
        assert_eq!(t.parent().unwrap(), tmp.path(), "damga yol olarak yorumlanmamalı");
        let bytes = std::fs::read(&t).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        assert_eq!(String::from_utf8(bytes[3..].to_vec()).unwrap(), "satır 1\r\nşğüöçı");
    }
}
