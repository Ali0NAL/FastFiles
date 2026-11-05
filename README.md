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
- **Hız.** Disk ve ağ birbirini beklemez: dosyalar ayrı bir iş parçacığında okunur, özetlenir ve sıkıştırılır. Alıcıda büyük dosyalar ayrı bir yazıcıya, küçük dosyalar (≤256 KB) 8 paralel yazıcıya gider. Ağ sınırı olmadan (aynı bilgisayarda) tek dosyada ~900 MB/sn ölçüldü; gigabit ethernette hat sınırı (~110 MB/sn), 2,5/10 GbE'de disk sınırı belirleyicidir.
- **Akıllı sıkıştırma.** Metin, log, CSV, kaynak kod gibi dosyalar blok blok zstd ile sıkıştırılarak gider. Zip, video, fotoğraf, Office belgeleri gibi zaten sıkıştırılmış biçimler ve sıkışmadığı görülen dosyalar olduğu gibi gider. Ayarlardan kapatılabilir.
- **Duraklat / devam ettir.** Süren bir gönderim duraklatılabilir. Alıcıdaki yarım dosyalar korunur; devam ettirince (uygulama kapatılıp açılsa bile) kaldığı yerden sürer.
- **Hız sınırı.** Ayarlardan bu bilgisayarın toplam gönderim hızı sınırlanabilir (ör. 10 MB/sn), böylece büyük gönderimler ofis ağını tıkamaz.
- **Canlı ilerleme.** Yüzde, hız ve kalan süre görünür. Aktarım her iki taraftan da iptal edilebilir.
- **Kaldığı yerden devam.** Bağlantı koparsa (Wi-Fi gitti, kablo çıktı, bilgisayar uyudu) gönderen birkaç saniye arayla kendiliğinden yeniden bağlanır. Alıcı tekrar onay sormaz, aktarım kaldığı yerden sürer. İlerleme olduğu sürece denemeye devam eder; olmazsa aktarım "başarısız" olur ve "Tekrar dene" ile yine kaldığı yerden sürdürülebilir. Yarım dosyalar alıcıda 3 gün saklanır. Gönderen iptal ederse hemen silinir.
- **Bütünlük doğrulaması.** Her dosyanın BLAKE3 özeti aktarım sırasında iki tarafta da hesaplanır ve karşılaştırılır. Diskte bozulan ya da iki deneme arasında değişen dosya kabul edilmez, yeniden gönderilir.
- **Aktarım geçmişi.** Biten gönderim ve alımlar (son 300) uygulama kapansa da saklanır. Alınanlar "Klasörde göster", gönderilenler "Tekrar gönder" ile açılır veya yeniden gönderilir. Kayıtlar tek tek ya da topluca silinebilir.
- **Panoyu gönder.** Cihaz kartındaki pano düğmesi veya ana ekranda **Ctrl+V**, panodakini gönderir: Gezgin'de kopyalanan dosyalar ve ekran görüntüsü dosya olarak (`Ekran görüntüsü <tarih>.png`), kısa metin mesaj olarak, 4000 karakterden uzun metin `.txt` olarak gider. Sohbette ekran görüntüsü yapıştırmak da onu o kişiye gönderir. Mesajların yanındaki düğmeyle metin tek tıkla kopyalanır.
- **Güvenli kayıt.** Aynı isimde dosya varsa üzerine yazılmaz, `dosya (1).txt` olarak kaydedilir. Yarım dosyalar `.ffpart` uzantısıyla tutulur ve özet doğrulanmadan asıl adını almaz. `../` gibi zararlı yollar reddedilir.
- **Sistem tepsisi.** Pencere kapatılsa da arka planda çalışmaya ve dosya almaya devam eder.
- **IP ile ekleme.** Otomatik keşfin ulaşmadığı ağlar için cihaz adresiyle eklenebilir.
- **Toplu gönderme.** "Toplu gönder" ile aynı dosya ve klasörler birden fazla kişiye tek seferde gönderilir. Dosyaları kartların dışındaki boş alana bırakmak da bu pencereyi açar. Her alıcı ayrıca onaylar.
- **Mesajlaşma.** Cihaz kartındaki mesaj butonuyla sohbet açılır. Okunmamış mesajlar rozetle gösterilir. Pencere arkadayken Windows bildirimi gelir ve görev çubuğu yanıp söner.
- **Grup mesajlaşma.** Mesajlar sekmesinde "Yeni grup" ile grup kurulur. Mesajlar her üyeye şifreli olarak ayrı ayrı gider. **Çevrimdışı üyeler geri geldiklerinde bekleyen mesajları otomatik alır** (en fazla 3 gün). Mesajın kaç kişiye ulaştığı görünür (ör. 2/3). Grubu yalnızca oluşturan kişi (yönetici) yeniden adlandırabilir, üye ekleyip çıkarabilir veya silebilir; yönetici sertifika parmak iziyle tanındığı için bu yetki taklit edilemez. Üyeler istedikleri zaman ayrılabilir.
- **Ortak klasörler.** "Klasörler" sekmesinden bir klasör oluşturup kişi eklenir. Klasöre eklenen, düzenlenen veya silinen dosyalar her üyenin bilgisayarındaki kopyaya (`İndirilenler\FastFiles\Ortak\<ad>`) otomatik yansır. Klasörün sahibi isteğe bağlı şifre koyabilir. Ayrıntılar aşağıda.
- **Oylama.** "Oylama" sekmesinden soru ve seçenekler (2–10) yazılır, oy verecek kişiler seçilir. Seçilenlere bildirim gider. Sonuçlar tüm katılımcılarda canlı güncellenir. Oy, oylama kapanana kadar değiştirilebilir. İsteğe bağlı **gizli oylama** seçilirse kimin neye oy verdiği kimseye gönderilmez, yalnızca sayılar görünür. Oyları yalnızca oylamayı açan kişi toplar ve bitirir. Oylama bilgisi yalnızca açan kişiden kabul edilir (sertifika parmak iziyle doğrulanır). Oy yalnızca katılımcılardan, açık oylamaya ve geçerli bir seçeneğe kabul edilir.
- **Profil resmi.** Ayarlar'da 20 hazır avatardan biri seçilir (`ui/avatars/`) ya da kendi resminiz yüklenir. Resim kare kırpılıp küçültülür ve diğer cihazlarda kartlarda, sohbetlerde ve üye listelerinde görünür.
- **Engelleme.** Engellenen cihazdan gelen mesaj ve dosyalar otomatik reddedilir.

### Ortak klasörler nasıl çalışır?

- **Eşitleme:** FastFiles her üyenin yerel kopyasını birkaç saniyede bir tarar ve değişiklikleri çevrimiçi üyelere şifreli bağlantıyla iletir. Kapalı olan bilgisayar açılınca eksiklerini tamamlar. Klasörü Gezgin'de doğrudan kullanabilirsiniz; dosya kopyalamak, düzenlemek veya silmek yeterlidir.
- **Çakışma:** Aynı dosya iki yerde değişirse en son değişiklik geçerli olur. Üzerine yazılan veya silinen her dosyanın eski hâli `.fastfiles\silinen` klasörüne taşınır; hiçbir şey kalıcı olarak kaybolmaz.
- **Üyelik:** Klasörü yalnızca oluşturan kişi (sahip) yönetebilir: üye ekleme/çıkarma, ad değiştirme, şifre. Sahip, sertifika parmak iziyle tanınır, taklit edilemez. Eklenmeyen kişiler klasörü göremez ve hiçbir dosya isteyemez. Çıkarılan üyenin eşitlemesi durur; o ana kadar aldığı dosyalar onun bilgisayarında kalır.
- **Şifre:** Sahip isterse şifre koyar. Üyeler şifreyi **bir kez** girer. Şifreyi girmemiş üyeye dosya gönderilmez ve o üye dosya listesini de alamaz. Şifre yalnızca sahibin bilgisayarında PBKDF2-HMAC-SHA256 özeti olarak (100.000 tur, rastgele tuz) saklanır ve ağda yalnızca TLS içinde gider. Tahmin denemelerine karşı cihaz başına 5 dakikada en fazla 5 deneme hakkı vardır.
- **Silme:** Klasör ekranındaki çöp kutusu simgesiyle yapılır. Sahip silerse klasör tüm üyeler için kapanır; çevrimdışı olanlara açıldıklarında bildirilir. Üye silerse klasörden ayrılır. İsteğe bağlı olarak bu bilgisayardaki dosyalar da silinir; diğer üyelerin kopyalarına dokunulmaz.
- **Güvenlik:** Ağdan gelen dosya yolları temizlenir (`..` gibi klasör dışına çıkan yollar reddedilir). Kısayol/sembolik bağlantılar izlenmez ve paylaşılmaz.
- **Windows bildirimleri.** Pencere arkadayken veya tepsideyken bildirim gelir:
  - güvenilir cihazdan dosya gelmeye başladığında,
  - dosya alımı bittiğinde ya da başarısız olduğunda,
  - onay bekleyen bir gönderim geldiğinde,
  - yeni mesaj geldiğinde.

  Bildirime tıklayınca uygulama açılır ve ilgili ekran gelir: mesajda o sohbet, dosyada aktarımlar.
- **Otomatik güncelleme.** Güncellemeler sunucu olmadan ağdaki FastFiles'lar arasında kendiliğinden yayılır. Ayrıntılar aşağıda.

### Otomatik güncelleme nasıl çalışır?

1. Yeni sürümü yayınlamak için sürüm numarasını artırın (`src-tauri/tauri.conf.json` ve `src-tauri/Cargo.toml` aynı olmalı), sonra çalıştırın:

   ```powershell
   .\release.ps1
   ```

2. Başka bir şey yapmanız gerekmez. Betik paketi bu bilgisayardaki FastFiles'ın gelen kutusuna (`%APPDATA%\io.twinup.fastfiles\updates\inbox`) koyar. FastFiles imzayı doğrulayıp paketi içe alır, önce kendini günceller, sonra paketi ağa dağıtır. (Paketi elle vermek isterseniz `.ffupdate` dosyasını FastFiles penceresine sürükleyin ya da Ayarlar → "Güncelleme paketi yükle".)
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
        ├── pipeline.rs    Disk tarafı: okuyucu, paralel yazıcılar, blok çerçevesi, zstd
        ├── resume.rs      Yarıda kalan alımların kaydı (kaldığı yerden devam)
        ├── clipboard.rs   "Panoyu gönder": dosya / ekran görüntüsü / metin
        ├── tls.rs         Sertifika parmak izi ile karşılıklı TLS
        └── config.rs      Cihaz kimliği ve ayarlar
```

### Protokol kısaca

1. Her cihaz 2,5 saniyede bir `{id, ad, port, parmak izi}` içeren bir UDP paketi yayınlar. 10 saniye duyulmayan cihaz listeden düşer.
2. Gönderen, alıcıya TLS ile bağlanır. Alıcının sertifika parmak izi duyurulanla eşleşmezse bağlantı reddedilir.
3. Gönderen bir `Offer` (dosya listesi, boyutlar, değişiklik zamanları ve desteklediği özellikler) yollar. Alıcı kullanıcıya sorar ve `Accept` ya da `Decline` döner.
4. Kabul edilirse dosyaların ham byte'ları sırayla akıtılır. Alıcı her dosyayı önce `.ffpart` uzantısıyla yazar, tamamlanınca yeniden adlandırır, en sonda `Done` gönderir.

**Protokol v2 (`ff2` özelliği).** İki taraf da destekliyorsa:

- Alıcı `Accept` içinde her dosya için elindeki bayt sayısını (`offsets`) bildirir. Gönderen yalnızca kalan kısmı akıtır. Alıcıda tamamlanmış dosyalar hiç gönderilmez.
- Veri en fazla 1 MB'lık bloklar halinde gider: `u32` başlık (üst bit = zstd ile sıkıştırılmış, kalan 31 bit = uzunluk) + veri. Her dosyanın blokları bitince **tüm dosyanın** 32 baytlık BLAKE3 özeti gelir. Alıcı sıkıştırılmış bloğu en fazla dosyanın kalan boyutu kadar açar; "sıkıştırma bombası" belleği şişiremez.
- Alıcı her kabulü `resume.json`'a yazar. Kayıtta gönderenin parmak izi, aktarım kimliği ve dosya listesinin özeti bulunur. Aynı parmak izinden aynı aktarım aynı dosya listesiyle tekrar gelirse onay sorulmaz. Başka bir cihaz bu kaydı kullanamaz.
- Gönderen iptal ederse alıcıya ayrı bir bağlantıyla `Abort` gönderir ve alıcı yarım dosyaları siler. Duraklatırsa `PauseNotice` gönderir; yarım dosyalar korunur.
- Eski sürümler bu alanları yok sayar ve aktarım eski yöntemle sürer.

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

Hız ölçümleri (aynı bilgisayarda iki sanal cihaz; sürüm derlemesiyle):

```bash
cargo test --release bench_ -- --ignored --nocapture --test-threads=1
```

> Not: Çok sayıda küçük dosyada Windows Defender her yeni dosyayı taradığı için alıcıdaki hız genellikle tarama hızıyla sınırlıdır (bu makinede ~550 dosya/sn). Tarama önbellekteyken ölçülen yazılım hızı ~2.000 dosya/sn'dir.

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
| Aktarım yavaş | Büyük dosyalarda hız ağla sınırlıdır: gigabit ethernet'te ~110 MB/sn, Wi-Fi'da çok daha az. Binlerce küçük dosyada sınır diskteki dosya oluşturma hızıdır (Windows + Defender, NVMe'de saniyede ~1000 dosya). FastFiles ağ ve diski paralel kullanır. Ölçmek için: `cargo test --release --lib bench -- --ignored --nocapture` |
| Güvenlik Duvarı izni yanlışlıkla reddedildi | Windows Güvenlik → Güvenlik duvarı → "Uygulamaya izin ver" menüsünden FastFiles için "Özel"i işaretleyin. |
