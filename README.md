# ⚡ FastFiles

Ofisteki bilgisayarlar arasında **internete çıkmadan**, **şifreli** ve **hızlı** dosya/klasör gönderip alma uygulaması.
Aynı ağa (Wi-Fi veya ethernet) bağlı bilgisayarlar birbirini otomatik bulur. Dosya bir cihazın üzerine sürüklenir, karşı taraf onaylar ve aktarım başlar.

## Özellikler

- **Otomatik keşif.** Uygulamayı açan herkes birkaç saniye içinde listede görünür. IP yazmaya gerek yoktur.
- **Sürükle-bırak.** Dosya ve klasörleri bir cihazın kartına bırakmak yeterlidir. "Dosya" ve "Klasör" butonları da var.
- **Klasör desteği.** Klasör yapısı karşı tarafta aynen korunur.
- **Onay penceresi.** Gelen her gönderim için kimden geldiği, dosya listesi ve toplam boyut gösterilir. Kabul veya red seçilir.
- **Güvenilir cihazlar.** "Bir daha sorma" işaretlenen cihazlardan gelen dosyalar otomatik kabul edilir.
- **Uçtan uca şifreleme.** Bağlantı karşılıklı TLS 1.3 ile kurulur. Her cihazın kendi sertifikası vardır ve kimlik, sertifika parmak iziyle doğrulanır.
- **Hız.** Dosyalar sarmalanmadan, 1 MB'lık bloklar halinde akıtılır. Gigabit ethernette hattın sınırına (~110 MB/sn) kadar çıkar.
- **Canlı ilerleme.** Yüzde, hız ve kalan süre görünür. Aktarım her iki taraftan da iptal edilebilir.
- **Güvenli kayıt.** Aynı isimde dosya varsa üzerine yazılmaz, `dosya (1).txt` olarak kaydedilir. Yarım kalan dosyalar silinir. `../` gibi zararlı yollar reddedilir.
- **Sistem tepsisi.** Pencere kapatılsa da arka planda çalışmaya ve dosya almaya devam eder.
- **IP ile ekleme.** Otomatik keşfin ulaşmadığı ağlar için cihaz adresiyle eklenebilir.
- **Toplu gönderme.** "Toplu gönder" ile aynı dosya ve klasörler birden fazla kişiye tek seferde gönderilir. Dosyaları kartların dışındaki boş alana bırakmak da bu pencereyi açar. Her alıcı ayrıca onaylar.
- **Mesajlaşma.** Cihaz kartındaki mesaj butonuyla sohbet açılır. Okunmamış mesajlar rozetle gösterilir. Pencere arkadayken Windows bildirimi gelir ve görev çubuğu yanıp söner.
- **Engelleme.** Engellenen cihazdan gelen mesaj ve dosyalar otomatik reddedilir.
- **Windows bildirimleri.** Pencere arkadayken veya tepsideyken bildirim gelir:
  - güvenilir cihazdan dosya gelmeye başladığında,
  - dosya alımı bittiğinde ya da başarısız olduğunda,
  - onay bekleyen bir gönderim geldiğinde,
  - yeni mesaj geldiğinde.
- **Otomatik güncelleme.** Güncellemeler sunucu olmadan ağdaki FastFiles'lar arasında kendiliğinden yayılır. Ayrıntılar aşağıda.

### Otomatik güncelleme nasıl çalışır?

1. Yeni sürümü yayınlamak için sürüm numarasını artırın (`src-tauri/tauri.conf.json` ve `src-tauri/Cargo.toml` aynı olmalı), sonra çalıştırın:

   ```powershell
   .\release.ps1
   ```

2. `release\FastFiles-<sürüm>.ffupdate` dosyasını **kendi FastFiles penceresine sürükleyin** (ya da Ayarlar → "Güncelleme paketi yükle").
3. Her FastFiles birkaç saniyede bir yaptığı duyuruda sürümünü ve elindeki paketi bildirir. Daha eski sürümdeki cihazlar paketi indirir, doğrular ve kendileri de dağıtmaya başlar.
4. Paket hazır olduktan yaklaşık 1 dakika sonra, **süren aktarım veya bekleyen istek yoksa** sessizce kurulur. Uygulama birkaç saniyeliğine kapanıp yeniden açılır (tepsideyse tepsiye geri döner). Kullanıcı üstteki çubuktan "Şimdi güncelle" veya "Sonra" (2 saat erteleme) diyebilir.

**Güvenlik:**
- Her paket yayıncının **Ed25519 gizli anahtarıyla** imzalanır. Uygulamada yalnızca açık anahtar bulunur.
- İmzası tutmayan paket indirilmez, dağıtılmaz ve **asla çalıştırılmaz**. İmza, veri indirilmeden önce başlıkta kontrol edilir.
- Sürüm numarası imzaya dahildir. Eski bir paket "yeni" gibi gösterilemez ve daha eski sürüme dönülemez.
- Paketin SHA-256 özeti indirildikten sonra ve kurulumdan hemen önce yeniden doğrulanır. Diskte bozulan paket dağıtılmaz.

> ⚠️ **Gizli anahtar:** `%USERPROFILE%\.fastfiles-release\update-key.pk8`. Bu dosya depoda **yoktur** ve olmamalıdır. Güvenli bir yere (ör. şifreli USB) yedekleyin. Kaybolursa yeni güncelleme yayınlanamaz; bu durumda herkese elle yeni anahtarlı bir sürüm kurmak gerekir. Başkasının eline geçerse o kişi herkese güncelleme dağıtabilir.

### Mesajlaşmanın güvenliği

| Risk | Önlem |
|---|---|
| Ağı dinleme | Mesajlar dosyalarla aynı karşılıklı TLS 1.3 bağlantısından gider. Sunucu yoktur. |
| Başkası adına mesaj | Sohbetler isme veya cihaz ID'sine göre değil, **sertifika parmak izine** göre tutulur. Gönderenin beyan ettiği kimlik TLS sertifikasıyla eşleşmezse mesaj reddedilir. |
| HTML/script enjeksiyonu | Mesajlar her zaman düz metin olarak basılır. Bağlantılar tıklanabilir yapılmaz (kimlik avı riski yok). |
| Görünmez karakter hileleri | Yön değiştiren (`U+202E` gibi) ve görünmez Unicode karakterler mesajlardan ve cihaz adlarından temizlenir. |
| Spam / taşma | Mesaj başına en fazla 4000 karakter, cihaz başına 30 sn'de en fazla 30 mesaj. Sohbet başına son 500 mesaj saklanır. |
| İstenmeyen kişi | Engelle: o cihazdan mesaj ve dosya kabul edilmez. |

Mesaj geçmişi yalnızca bu bilgisayarda (`%APPDATA%\io.twinup.fastfiles\chats.json`) tutulur ve sohbet ekranından silinebilir.

## Kurulum (kullanıcılar için)

`FastFiles_<sürüm>_x64-setup.exe` dosyasını çalıştırın. Yönetici izni gerekmez. Eski sürümü kaldırmaya gerek yoktur: önce tepsideki FastFiles'tan **Çıkış** yapın, sonra kurulumu çalıştırın.

1.2.0 ve sonrası **kendini otomatik günceller**. 1.1.x veya daha eski sürüm kullananların 1.2.0'ı bir kez elle kurması gerekir.

FastFiles aynı anda yalnızca bir kez çalışır. Tepsideyken kısayola tekrar tıklamak yeni bir kopya açmaz, mevcut pencereyi öne getirir.

İlk açılışta Windows Güvenlik Duvarı izin isteyebilir. **"Özel ağlar"** seçeneğine izin verin. Aksi halde diğer bilgisayarlar sizi göremez ve size dosya gönderemez.

> Gelen dosyalar varsayılan olarak `İndirilenler\FastFiles` klasörüne kaydedilir. Bunu Ayarlar'dan değiştirebilirsiniz.

## Teknoloji

| Katman | Teknoloji |
|---|---|
| Masaüstü kabuğu | [Tauri 2](https://tauri.app) (Windows'ta WebView2) |
| Arka uç | Rust, `tokio` (asenkron ağ/disk) |
| Şifreleme | `rustls` (karşılıklı TLS), `rcgen` (cihaz sertifikası) |
| Keşif | UDP multicast `239.255.53.18:53318` ve alt ağ broadcast |
| Aktarım | TCP `53317` (doluysa rastgele bir port) |
| Arayüz | Derleme adımı gerektirmeyen düz HTML/CSS/JS (`ui/`) |

```
FastFiles/
├── ui/                    Arayüz (index.html, styles.css, main.js)
└── src-tauri/
    ├── tauri.conf.json    Pencere, paketleme ayarları
    └── src/
        ├── lib.rs         Tauri komutları, sistem tepsisi
        ├── node.rs        Çekirdek: cihaz listesi, gönderme/alma, aktarım durumu (+ uçtan uca testler)
        ├── discovery.rs   UDP ile cihaz keşfi
        ├── protocol.rs    Mesaj formatı, güvenli yol kontrolü
        ├── tls.rs         Sertifika parmak izi ile karşılıklı TLS
        └── config.rs      Cihaz kimliği ve ayarlar
```

### Protokol kısaca

1. Her cihaz 2,5 saniyede bir `{id, ad, port, parmak izi}` içeren bir UDP paketi yayınlar. 10 saniye duyulmayan cihaz listeden düşer.
2. Gönderen, alıcıya TLS ile bağlanır. Alıcının sertifika parmak izi duyurulanla eşleşmezse bağlantı reddedilir.
3. Gönderen bir `Offer` (dosya listesi ve boyutlar) yollar. Alıcı kullanıcıya sorar ve `Accept` ya da `Decline` döner.
4. Kabul edilirse dosyaların ham byte'ları sırayla akıtılır. Alıcı her dosyayı önce `.ffpart` uzantısıyla yazar, tamamlanınca yeniden adlandırır, en sonda `Done` gönderir.

## Geliştirme

Gereksinimler: [Rust](https://rustup.rs) ve Visual Studio C++ Build Tools. Node.js **gerekmez**.

```bash
cargo install tauri-cli --version "^2" --locked
```

Uygulamayı geliştirme modunda çalıştırmak için (`src-tauri` klasöründe):

```bash
cargo tauri dev
```

Testleri çalıştırmak için (iki sanal cihaz arasında gerçek TLS aktarımı yapılır):

```bash
cargo test
```

Kurulum dosyası üretmek için. Çıktı `src-tauri/target/release/bundle/nsis/` altına yazılır:

```bash
cargo tauri build
```

### Aynı bilgisayarda iki cihaz denemek

Her profil ayrı kimlik ve ayarlarla ayrı bir "cihaz" gibi davranır. PowerShell'de:

```powershell
$env:FASTFILES_PROFILE="ikinci"; .\target\debug\fastfiles.exe
```

### Arayüzü tarayıcıda önizlemek

`ui/index.html` Tauri dışında açılırsa örnek verilerle çalışır. Gelen istek penceresini görmek için `#incoming`, boş durumu görmek için `#empty` ekleyin.

```bash
py -m http.server 5178 --directory ui
```

## Sorun giderme

| Sorun | Çözüm |
|---|---|
| Diğer bilgisayarlar görünmüyor | İkisinde de FastFiles açık mı? Güvenlik Duvarı'nda FastFiles'a "Özel ağ" izni verildi mi? Ağ profili "Ortak" değil "Özel" olmalı. |
| Wi-Fi'daki bilgisayar ethernet'tekileri görmüyor | Modemde "AP/Client isolation" kapalı olmalı. Farklı alt ağlardaysanız (ör. 192.168.1.x ve 192.168.2.x) **IP ile ekle** kullanın. |
| Aktarım yavaş | Wi-Fi hızı ortamdan etkilenir. İki taraf da ethernet'teyse gigabit hız beklenir. |
| Güvenlik Duvarı izni yanlışlıkla reddedildi | Windows Güvenlik → Güvenlik duvarı → "Uygulamaya izin ver" menüsünden FastFiles için "Özel"i işaretleyin. |
