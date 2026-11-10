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
- **Panoyu gönder.** Cihaz kartındaki pano düğmesi veya ana ekranda **Ctrl+V**, panodakini gönderir: Gezgin'de kopyalanan dosyalar ve ekran görüntüsü dosya olarak (`Ekran görüntüsü <tarih>.png`), kısa metin mesaj olarak, 4000 karakterden uzun metin `.txt` olarak gider. Sohbette ekran görüntüsü yapıştırmak da onu o kişiye gönderir. Mesajların yanındaki düğmeyle metin tek tıkla kopyalanır. Mesajdaki `http://`, `https://` ve `www.` ile başlayan bağlantılar tıklanabilir; varsayılan tarayıcıda açılır (yalnızca web adresleri açılır, dosya ya da program bağlantıları açılmaz).
- **Güvenli kayıt.** Aynı isimde dosya varsa üzerine yazılmaz, `dosya (1).txt` olarak kaydedilir. Yarım dosyalar `.ffpart` uzantısıyla tutulur ve özet doğrulanmadan asıl adını almaz. `../` gibi zararlı yollar reddedilir.
- **Sistem tepsisi.** Pencere kapatılsa da arka planda çalışmaya ve dosya almaya devam eder.
- **IP ile ekleme.** Otomatik keşfin ulaşmadığı ağlar için cihaz adresiyle eklenebilir.
- **Toplu gönderme.** "Toplu gönder" ile aynı dosya ve klasörler birden fazla kişiye tek seferde gönderilir. Dosyaları kartların dışındaki boş alana bırakmak da bu pencereyi açar. Her alıcı ayrıca onaylar.
- **Mesajlaşma.** Cihaz kartındaki mesaj butonuyla sohbet açılır. Okunmamış mesajlar rozetle gösterilir. Pencere arkadayken Windows bildirimi gelir ve görev çubuğu yanıp söner.
- **Grup mesajlaşma.** Mesajlar sekmesinde "Yeni grup" ile grup kurulur. Mesajlar her üyeye şifreli olarak ayrı ayrı gider. **Çevrimdışı üyeler geri geldiklerinde bekleyen mesajları otomatik alır** (en fazla 3 gün). Mesajın kaç kişiye ulaştığı görünür (ör. 2/3). Grubu yalnızca oluşturan kişi (yönetici) yeniden adlandırabilir, üye ekleyip çıkarabilir veya silebilir; yönetici sertifika parmak iziyle tanındığı için bu yetki taklit edilemez. Üyeler istedikleri zaman ayrılabilir.
- **Sohbette dosyalar.** Bir kişiyle yapılan her gönderim ve alım, o kişinin sohbetinde dosya balonu olarak görünür: ilerleme, durum, "Klasörde göster", "İptal", "Tekrar dene". Sohbetteki ataç düğmesi ya da dosyaları açık sohbetin üzerine sürüklemek o kişiye gönderir.
- **Gruba dosya gönderme.** Grup sohbetine dosya/klasör eklenince her üyeye ayrı bir aktarım başlar (her üye kendi ekranında onaylar). **Çevrimdışı üyeler geri geldiklerinde dosyayı otomatik alır** (en fazla 3 gün; kaynak dosyalar yerinde durmalı). Balonda kaç kişinin aldığı görünür. Alıcı, dosyayı grup sohbetinde ancak gönderen de kendisi de o grubun üyesiyse gösterir; aksi halde birebir sohbette görünür (grup kimliği taklit edilemez).
- **Gezgin'den gönder.** Dosya ya da klasöre sağ tık → "FastFiles ile gönder" (Windows 11'de "Diğer seçenekleri göster" altında; çok sayıda dosya için "Gönder" menüsünde de bulunur). Seçilenler "Toplu gönder" penceresine gelir, alıcı seçilir. Menü yalnızca bu kullanıcı için eklenir (yönetici izni gerekmez) ve Ayarlar'dan kaldırılabilir.
- **Ortak klasörler.** "Klasörler" sekmesinden bir klasör oluşturup kişi eklenir. Klasöre eklenen, düzenlenen veya silinen dosyalar her üyenin bilgisayarındaki kopyaya (`İndirilenler\FastFiles\Ortak\<ad>`) otomatik yansır. Klasörün sahibi isteğe bağlı şifre koyabilir. Ayrıntılar aşağıda.
- **Oylama.** "Oylama" sekmesinden soru ve seçenekler (2–10) yazılır, oy verecek kişiler seçilir. Seçilenlere bildirim gider. Sonuçlar tüm katılımcılarda canlı güncellenir. Oy, oylama kapanana kadar değiştirilebilir. İsteğe bağlı **gizli oylama** seçilirse kimin neye oy verdiği kimseye gönderilmez, yalnızca sayılar görünür. Oyları yalnızca oylamayı açan kişi toplar ve bitirir. Oylama bilgisi yalnızca açan kişiden kabul edilir (sertifika parmak iziyle doğrulanır). Oy yalnızca katılımcılardan, açık oylamaya ve geçerli bir seçeneğe kabul edilir.
- **Oyunlar.** Üst çubuktaki "Oyunlar" düğmesiyle (ya da sohbet başlığındaki oyun kolu simgesiyle) bir oyun ve rakip seçilip davet gönderilir. Rakip kabul edince oyun odası açılır ve karşılıklı oynanır. İki oyun var:
  - **Satranç** (2 kişi): tıklayarak ya da sürükleyerek hamle, yasal kareler, son hamle ve şah vurgusu, terfi seçimi, rok, geçerken alma, şah mat, pat, üç kez tekrar, 50 hamle kuralı ve yetersiz malzeme, beraberlik teklifi, pes etme, rövanş. Hamleler Türkçe notasyonla (Ş V K F A) listelenir.
  - **Mini golf** (2–6 kişi): 6 delik (duvar, kum, su, direkler). Sırayla vurulur: top geriye doğru çekilip bırakılır, ne kadar çekilirse o kadar sert gider. Topun yolu herkeste canlandırılır, skor tablosunda delik delik vuruşlar ve par farkı görünür. Delik başına en fazla 8 vuruş; suya düşen top +1 ceza ile geri gelir. Davet edilenlerden biri reddederse oyun kalanlarla başlar; oyundan çekilen olursa diğerleri devam eder.

  Ayrıntılar aşağıda.
- **Durum.** Üst çubuktaki adınıza tıklayıp durum seçilir: 🟢 Müsait, 📅 Toplantıda, 🎧 Odakta, ☕ Dışarıda. Durum cihaz kartlarında, sohbet başlığında, hızlı gönderimde ve 3B ofiste görünür (odaktaki kişi kulaklık takar, dışarıdakinin koltuğu boşalır). "Müsait"teyken 10 dakika klavye/fare kullanılmazsa kendiliğinden "Dışarıda" olur. **"Odakta" iken Windows bildirimleri susar**; grupta adınız geçerse (`@Ad`, `@herkes`) ve onay bekleyen dosya geldiğinde yine bildirilir.
- **Tepki, yanıt, düzenleme, silme.** Mesajın üzerine gelince çıkan araçlarla: 👍 ❤️ 😂 😮 😢 🙏 🎉 🔥 tepkisi (çift tıklama hızlı 👍), alıntılayarak yanıt (alıntıya tıklayınca asıl mesaja gidilir), kendi mesajını 24 saat içinde düzenleme (boş kutuda ↑ son mesajı düzenler) ve herkesten silme. Yalnızca mesajı yazan kişi düzenleyip silebilir; bu, TLS parmak iziyle doğrulanır.
- **@bahsetme.** Grupta `@` yazınca üye listesi açılır. Bahsedilen kişide mesaj vurgulanır ve "Odakta" olsa bile bildirim gelir.
- **Sohbette arama.** Sohbet menüsünden ya da **Ctrl+F** ile; eşleşmeler işaretlenir, Enter/Shift+Enter ile gezilir.
- **Dürt.** Birebir sohbet başlığındaki el düğmesi karşının penceresini titretir (kişi başına 10 saniyede bir; "Odakta" olan kişide yalnızca sohbete not düşer).
- **Çevrimdışı kişiye dosya.** Kişi çevrimdışıyken de sohbetine dosya eklenebilir ya da sürüklenebilir; kişi çevrimiçi olunca (3 gün içinde) kendiliğinden gönderilir.
- **Hızlı gönderim (Ctrl+Shift+Boşluk).** Hangi programdayken olursanız olun küçük bir pencere açılır: isim yazıp **Enter** panodakini (dosya, ekran görüntüsü, metin) o kişiye gönderir, **Shift+Enter** sohbeti açar. Uygulama içinde **Ctrl+K**.
- **Ekran görüntüsü → çiz → gönder (Ctrl+Shift+PrtSc).** Windows alıntı aracıyla alan seçilir; açılan düzenleyicide kalem, ok, kutu, vurgulayıcı ve yazı ile işaretlenip kişiye ya da gruba gönderilir veya panoya kopyalanır. Sohbetteki kamera düğmesi de aynı şeyi yapar. Kısayollar Ayarlar'dan kapatılabilir.
- **Telefon (QR kod).** Üst çubuktaki telefon düğmesi bir QR kod gösterir. Aynı Wi-Fi'daki telefon kodu okutunca açılan sayfadan bilgisayara fotoğraf, video ve dosya yükler (`İndirilenler\FastFiles\Telefon`), "Telefona gönder" ile seçilen dosyaları da indirir. Telefona uygulama kurmak gerekmez. Bağlantı yerel ağda kalır; adres tek seferlik rastgele bir anahtar içerir ve pencere kapanınca (ya da 30 dakika kullanılmazsa) geçersiz olur.
- **Tek tıkla ekran paylaşımı.** Sohbet başlığındaki ekran düğmesi ana ekranı o kişiyle (grupta tüm üyelerle) paylaşır. Karşı tarafa "İzle" daveti gider; izleyici tam ekran, sığdır/gerçek boyut seçenekleriyle izler. Paylaşırken üstte kırmızı bir çubuk kimin izlediğini gösterir ve "Paylaşımı durdur" ile biter. Ekran saniyede ~8 kare JPEG olarak, diğer her şey gibi karşılıklı TLS ile şifreli gider; yalnızca davet edilen kişiler bağlanabilir ve ekran yalnızca izleyen varken yakalanır.
- **Birlikte izle (YouTube).** Sohbet başlığındaki TV düğmesine YouTube bağlantısı yapıştırılır (ya da sohbetteki bir YouTube bağlantısının yanındaki **▶ Birlikte izle**'ye basılır); birebir sohbette o kişi, grupta tüm üyeler davet edilir. Davet edilenler bildirimden ya da üstteki kırmızı çubuktan **Katıl** der. Oynat, duraklat, sar ve video değiştir herkeste aynı anda olur; YouTube oynatıcısının kendi düğmeleri de kullanılabilir. **Sıra**: herkes bağlantı ekler, video bitince sıradaki kendiliğinden başlar. İzlerken 👍 ❤️ 😂 😮 😢 👏 🔥 🎉 tepkileri videonun üzerinde uçar, kimin durdurduğu/sardığı kısa notla görünür. Odayı açan kişi kontrolü yalnızca kendine alabilir (🔒). **Küçült** (Esc) ile oynatıcı köşede yüzer ve sürüklenebilir; izlerken sohbet ve diğer her şey kullanılır. Ayrıntılar aşağıda.
- **Sanal ofis (3B).** Üst çubuktaki "Ofis" düğmesi ofisteki masanın 3B dijital ikizini açar: üstte 4, altta 3 koltuk. Herkes "Koltuğumu seç" ile oturduğu yeri seçer; seçim diğer bilgisayarlara kendiliğinden gider. Koltuk seçmemiş kişiler boş koltuklara oturtulur, çevrimdışı olanların koltuğunda kapalı dizüstü durur. Mesaj gönderilince kişiden kişiye kâğıt uçak uçar, yazan kişinin üstünde "yazıyor…" görünür, dosya aktarımında iki masa arasında veri akar ve paket gerçek ilerlemeyle yol alır (bitince yeşil, reddedilince kırmızı). Süren satranç oyunu iki oyuncunun arasında masada tahta olarak görünür. Kişiye tıklamak sohbeti açar, dosyayı 3B'deki kişinin üzerine bırakmak ona gönderir. Fareyle döndürülür, tekerlekle yakınlaştırılır. Şimdilik yalnızca bu bilgisayarın taraf olduğu mesaj ve aktarımlar canlandırılır. Three.js (MIT) uygulamanın içindedir (`ui/vendor/`), internet gerekmez; ekran yalnızca açıkken çizilir.
- **Profil resmi.** Ayarlar'da 20 hazır avatardan biri seçilir (`ui/avatars/`) ya da kendi resminiz yüklenir. Resim kare kırpılıp küçültülür ve diğer cihazlarda kartlarda, sohbetlerde ve üye listelerinde görünür.
- **Emoji.** Sohbette gülen yüz düğmesiyle emoji eklenir; son kullanılanlar en üstte durur.
- **"Yazıyor…"** Karşı taraf yazarken sohbet başlığında ve sohbet listesinde "yazıyor…" görünür; grupta kimin yazdığı yazar. Sinyal yalnızca çevrimiçi kişilere gider, saklanmaz.
- **Kabul bildirimi.** Gönderdiğiniz dosyayı karşı taraf onaylayınca size bildirim gelir (güvenilir cihazların anında kabulünde bildirim çıkmaz).
- **Renk temaları.** Ayarlar'dan 6 renk teması (Mor, Okyanus, Orman, Gün batımı, Gül, Grafit) ve görünüm (sistemle aynı / açık / koyu) seçilir.
- **Uygulama kilidi (isteğe bağlı).** Ayarlar'dan 4 haneli PIN belirlenirse uygulama kilitli açılır. Üst çubuktaki kilit düğmesiyle istenen an, ayardaki süre boyunca işlem yapılmazsa otomatik kilitlenir. Kilitliyken bildirimler gelir ama içeriği (kimden, ne) gösterilmez. PIN yalnızca bu bilgisayarda PBKDF2 özeti olarak saklanır; 5 yanlış denemeden sonra bekleme süresi uygulanır. **PIN unutulursa** kilit ekranındaki "PIN'imi unuttum" ile sıfırlanır: bu bilgisayardaki tüm mesajlar ve aktarım geçmişi silinir (grup üyelikleri, ortak klasörler ve indirilmiş dosyalar kalır). Bu, ofis içi kullanım için bilinçli olarak seçilmiş bir koruma düzeyidir: PIN'i bilmeyen biri eski mesajları göremez.
- **Engelleme.** Engellenen cihazdan gelen mesaj ve dosyalar otomatik reddedilir.

### Oyunlar nasıl çalışır?

- **Oda:** Daveti gönderen kişi odanın sahibidir; sunucu yoktur. Her hamle şifreli bağlantıyla sahibine gider; sahip hamleyi kurallarla doğrular, odayı günceller ve yeni hâlini rakibe iletir. Rakip çevrimdışıysa davet ve hamleler o geri gelince otomatik ulaşır; oyun kaldığı yerden sürer.
- **Güvenlik:** Oda bilgisi yalnızca sahibinden kabul edilir (sertifika parmak izi). Oyuncu, sahipten gelen hamle listesini kendi bilgisayarında kurallarla baştan oynatıp doğrular: geçmiş değiştirilemez, sahip oyuncu adına hamle uyduramaz, "kabul etti", "pes etti" ya da "beraberliğe razı oldu" diyemez. Aynı kişiden bekleyen davet sayısı sınırlıdır.
- **Golf fiziği:** Yalnızca arka uçtadır (`games/golf.rs`, sabit 1/240 sn adımlı). Oda sahibi de diğer oyuncular da aynı vuruşu aynı kodla oynatır; arayüz topun yolunu arka uçtan alıp canlandırır. Bu yüzden kimse topunun nereye gittiğini değiştiremez.
- **Yeni oyun eklemek:** Kurallar `src-tauri/src/games/` altında saf fonksiyonlardır (`games/mod.rs` içindeki `CATALOG`, `evaluate`, `move_allowed`, `move_actor`, `forfeit_move`). Davet, oda, ağ ve kayıt (`src-tauri/src/game.rs`) oyundan bağımsızdır; arayüzde yalnızca oyun alanının çizimi oyuna özeldir. Oyun başına oyuncu sayısı (`min_players`/`max_players`) tanımlıdır: satranç 2, mini golf 2–6 kişiliktir.

### Birlikte izle nasıl çalışır?

- **Video ağdan geçmez.** Herkes videoyu kendi bilgisayarında YouTube'dan oynatır (internet gerekir). FastFiles yalnızca odanın durumunu eşitler: hangi video, oynuyor mu, kaçıncı saniyede. Bu küçük mesajlar diğer her şey gibi karşılıklı TLS ile şifreli gider.
- **Oda:** Odayı açan kişi sahibidir; sunucu yoktur. İstekler (oynat, duraklat, sar, sıraya ekle) sahibine gider, sahip uygular ve odanın yeni hâlini herkese iletir. Oda yalnızca bellekte tutulur; sahip kapatınca ya da uygulamadan çıkınca herkeste biter. Sonradan çevrimiçi olan davetli de odaya katılabilir.
- **Eşitleme:** Bilgisayarların saatleri farklı olabileceği için saat kullanılmaz: mesaj, gönderildiği andaki konumu taşır, alan taraf aldığı andan beri geçen süreyi ekler. Oynatıcı 1,5 saniyeden fazla geri ya da ileri düşerse (yavaş bağlantı, reklam, takılma) kendiliğinden sarılır. Alttaki yeşil "Herkesle aynı yerde" bunu gösterir.
- **Güvenlik:** Oda bilgisi yalnızca sahibinden kabul edilir (sertifika parmak izi). İstek yalnızca davetlilerden, kontrol için de odada izliyor olanlardan kabul edilir; cihaz başına hız sınırı vardır. Ağdan yalnızca 11 karakterlik YouTube video kimliği alınır; oynatıcı her zaman `youtube.com/embed/<kimlik>` olarak açılır, ağdan gelen adres açılmaz. Uygulamanın güvenlik politikası yalnızca `www.youtube.com` çerçevesine ve `i.ytimg.com` küçük resimlerine izin verir; YouTube'un dış betiği yüklenmez (oynatıcı `postMessage` ile denetlenir, `ui/ytplayer.js`).
- **Oynatılamayan video:** Sahibi gömülü oynatmayı kapattıysa oynatıcıda uyarı ve "YouTube'da aç" düğmesi çıkar; sıradan başka bir video seçilebilir.

### Ortak klasörler nasıl çalışır?

- **Eşitleme:** FastFiles her üyenin yerel kopyasını birkaç saniyede bir tarar ve değişiklikleri çevrimiçi üyelere şifreli bağlantıyla iletir. Kapalı olan bilgisayar açılınca eksiklerini tamamlar. Klasörü Gezgin'de doğrudan kullanabilirsiniz; dosya kopyalamak, düzenlemek veya silmek yeterlidir.
- **Çakışma:** Aynı dosya iki yerde değişirse en son değişiklik geçerli olur. Üzerine yazılan veya silinen her dosyanın eski hâli `.fastfiles\silinen` klasörüne taşınır; hiçbir şey kalıcı olarak kaybolmaz.
- **Üyelik:** Klasörü yalnızca oluşturan kişi (sahip) yönetebilir: üye ekleme/çıkarma, ad değiştirme, şifre. Sahip, sertifika parmak iziyle tanınır, taklit edilemez. Eklenmeyen kişiler klasörü göremez ve hiçbir dosya isteyemez. Çıkarılan üyenin eşitlemesi durur; o ana kadar aldığı dosyalar onun bilgisayarında kalır.
- **Şifre:** Sahip isterse şifre koyar. Üyeler şifreyi **bir kez** girer. Şifreyi girmemiş üyeye dosya gönderilmez ve o üye dosya listesini de alamaz. Şifre yalnızca sahibin bilgisayarında PBKDF2-HMAC-SHA256 özeti olarak (100.000 tur, rastgele tuz) saklanır ve ağda yalnızca TLS içinde gider. Tahmin denemelerine karşı cihaz başına 5 dakikada en fazla 5 deneme hakkı vardır.
- **Silme:** Klasör listesinde satırın üzerine gelince çıkan ya da klasör ekranındaki çöp kutusu simgesiyle yapılır. Sahip silerse klasör tüm üyeler için kapanır; çevrimdışı olanlara açıldıklarında bildirilir. Üye silerse klasörden ayrılır. İsteğe bağlı olarak bu bilgisayardaki dosyalar da silinir; diğer üyelerin kopyalarına dokunulmaz.
- **Güvenlik:** Ağdan gelen dosya yolları temizlenir (`..` gibi klasör dışına çıkan yollar reddedilir). Kısayol/sembolik bağlantılar izlenmez ve paylaşılmaz.
- **Windows bildirimleri.** Pencere arkadayken veya tepsideyken bildirim gelir:
  - güvenilir cihazdan dosya gelmeye başladığında,
  - dosya alımı bittiğinde ya da başarısız olduğunda,
  - onay bekleyen bir gönderim geldiğinde,
  - yeni mesaj geldiğinde.

  Bildirime tıklayınca uygulama açılır ve ilgili ekran gelir: mesajda o sohbet, dosyada aktarımlar.
- **Otomatik güncelleme.** Güncellemeler sunucu olmadan ağdaki FastFiles'lar arasında kendiliğinden yayılır. Ayrıntılar aşağıda.

### Otomatik güncelleme nasıl çalışır?

1. Ofisteki herkesi güncellemek için proje klasöründe tek komut yeterlidir (yeni sürüm numarasını siz verirsiniz, mevcuttan büyük olmalı):

   ```powershell
   .\release.cmd -Version 1.4.1
   ```

   (`release.cmd`, Windows'un `.ps1` betik kısıtlamasını yalnızca bu çalıştırma için atlayıp `release.ps1`'i çalıştırır. Betik çalıştırmaya izin verilen bir terminalde doğrudan `.\release.ps1 -Version 1.4.1` de olur.) Komutu çalıştırırken **bu bilgisayardaki FastFiles açık olmalıdır** (tepside de olur). Kapalıysa güncelleme, FastFiles açılana kadar gelen kutusunda bekler.

   Betik sürüm numarasını `src-tauri/tauri.conf.json` ve `src-tauri/Cargo.toml` içinde günceller, derler ve gizli anahtarla imzalar. Komut, imza anahtarının bulunduğu bilgisayarda çalıştırılmalıdır. `git push` tek başına kimseyi güncellemez.

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
        ├── shell.rs       Gezgin sağ tık menüsü ve "Gönder" kısayolu
        ├── applock.rs     İsteğe bağlı açılış PIN'i (uygulama kilidi)
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
- Gruptan gönderilen teklif `group_id` taşır; alıcı bunu kendi grup bilgisiyle doğrular.
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
