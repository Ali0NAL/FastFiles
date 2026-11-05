//! Gezgin entegrasyonu: dosya/klasöre sağ tık → "FastFiles ile gönder".
//!
//! - **Sağ tık menüsü** (`HKCU\Software\Classes\*\shell` ve `Directory\shell`): yalnızca bu
//!   kullanıcı için, yönetici izni gerektirmez. Windows 11'de "Diğer seçenekleri göster" altındadır.
//!   Birden fazla öğe seçilince Windows komutu her öğe için ayrı çalıştırır; tek örnek koruması
//!   bunları açık pencereye iletir, uygulama kısa bir süre içinde gelenleri tek gönderimde toplar.
//! - **"Gönder" menüsü** kısayolu (`%APPDATA%\Microsoft\Windows\SendTo\FastFiles.lnk`): çok sayıda
//!   seçili öğeyi tek komutta iletir.
//!
//! Komut satırı: `fastfiles.exe --send <yol> [<yol> ...]`

use std::path::{Path, PathBuf};

/// Komut satırından gönderilecek yolları çıkarır. Göreli yollar `cwd`'ye göre çözülür,
/// var olmayan yollar atlanır.
pub fn parse_send_args(args: &[String], cwd: &Path) -> Vec<String> {
    let Some(i) = args.iter().position(|a| a == "--send") else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for a in &args[i + 1..] {
        if a.starts_with("--") {
            continue;
        }
        let p = PathBuf::from(a);
        let p = if p.is_absolute() { p } else { cwd.join(p) };
        if p.exists() {
            let s = p.to_string_lossy().into_owned();
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const VERB_KEYS: [&str; 2] = [r"Software\Classes\*\shell\FastFilesSend", r"Software\Classes\Directory\shell\FastFilesSend"];

    fn send_to_link() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join(r"Microsoft\Windows\SendTo\FastFiles.lnk"))
    }

    pub fn register(exe: &Path) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let exe_s = exe.display().to_string();
        for k in VERB_KEYS {
            let (key, _) = hkcu.create_subkey(k).map_err(|e| e.to_string())?;
            key.set_value("", &"FastFiles ile gönder").map_err(|e| e.to_string())?;
            key.set_value("Icon", &format!("\"{exe_s}\",0")).map_err(|e| e.to_string())?;
            let (cmd, _) = key.create_subkey("command").map_err(|e| e.to_string())?;
            cmd.set_value("", &format!("\"{exe_s}\" --send \"%1\"")).map_err(|e| e.to_string())?;
        }
        if let Some(link) = send_to_link().filter(|l| !l.exists()) {
            create_shortcut(&link, exe)?;
        }
        Ok(())
    }

    pub fn unregister() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for k in VERB_KEYS {
            let _ = hkcu.delete_subkey_all(k);
        }
        if let Some(link) = send_to_link() {
            let _ = std::fs::remove_file(link);
        }
    }

    /// .lnk oluşturmak için Windows'un kendi kabuk nesnesi (PowerShell üzerinden) kullanılır.
    fn create_shortcut(link: &Path, exe: &Path) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Tek tırnak içindeki tek tırnaklar PowerShell'de ikilenerek kaçırılır.
        let q = |p: &Path| p.display().to_string().replace('\'', "''");
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='--send';$s.IconLocation='{},0';$s.Description='FastFiles ile gönder';$s.Save()",
            q(link),
            q(exe),
            q(exe)
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("Gönder menüsü kısayolu oluşturulamadı".into())
        }
    }
}

/// Ayara göre menü kaydını ekler ya da kaldırır. Geliştirme derlemesinde kayıt defterine dokunmaz.
pub fn apply(enabled: bool) {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else { return };
        let dir = exe.parent().map(|d| d.to_string_lossy().to_lowercase()).unwrap_or_default();
        if dir.ends_with("\\target\\debug") || dir.ends_with("\\target\\release") {
            return;
        }
        if enabled {
            if let Err(e) = imp::register(&exe) {
                eprintln!("[fastfiles] Gezgin menüsü eklenemedi: {e}");
            }
        } else {
            imp::unregister();
        }
    }
    #[cfg(not(windows))]
    let _ = enabled;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_args_are_parsed_and_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
        std::fs::create_dir(tmp.path().join("Klasör")).unwrap();
        let abs = tmp.path().join("a.txt").to_string_lossy().into_owned();
        let args: Vec<String> = ["fastfiles.exe", "--hidden", "--send", &abs, "Klasör", "yok.txt", &abs]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got = parse_send_args(&args, tmp.path());
        assert_eq!(got, vec![abs.clone(), tmp.path().join("Klasör").to_string_lossy().into_owned()]);
        assert!(parse_send_args(&["fastfiles.exe".into(), abs], tmp.path()).is_empty(), "--send yoksa boş");
    }
}
