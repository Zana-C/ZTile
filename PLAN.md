# ZTile — Uygulama Planı

Windows 11 için ultra hafif, dizilim (layout) tabanlı pencere yöneticisi.
Dil: **Rust + saf Win32 API (`windows-rs`)**, UI Direct2D/GDI ile elle çizilir.

## Performans hedefleri
- Boşta CPU %0 (polling/timer yok, tamamen olay güdümlü)
- Private RAM < 10 MB
- Soğuk açılış < 50 ms, açık uygulamalarla dizilim uygulama < 100 ms
- Tek exe, kurulumsuz çalışabilir

## Davranış kararları (kullanıcı ile netleşti)

| Konu | Karar |
|---|---|
| Çoklu workspace | Aynı anda birden fazla dizilim açık kalabilir. Her birinin taskbar'da ayrı birleşik simgesi olur. Tıklanan grup öne gelir, aynı ekrandaki diğer grup arkaya geçer (sanal masaüstü mantığı). |
| Taskbar tıklama | Grup arkadaysa hepsi öne gelir, zaten öndeyse hepsi minimize olur. |
| Elle sürükleme | Başka hücreye bırakılırsa **swap**. Grid dışına (ör. diğer monitöre) bırakılırsa gruptan **kopar**, hücrede "+" çıkar. Küçük kaydırmalarda kendi hücresine **snap** ile geri döner. |
| Başlık çubuğu | Varsayılan: başlık çubukları kalır. Dizilim/hücre bazında **borderless** seçeneği. |
| "+" tıklanınca | Önce "Açık pencerelerden seç" listesi, altında "Yeni uygulama başlat" (Başlat menüsü + kayıtlı uygulamalar). |
| Odak | ZTile gruptaki uygulamalara **asla odak vermez** (WhatsApp/Instagram'ın "görüldü" göndermemesi için). Grup öne gelince odak ZTile'ın görünmez temsilci penceresinde kalır. Yalnızca kullanıcı bir pencereye kendisi tıklarsa o pencere odaklanır. |
| Oyunlar | Tam ekran/exclusive pencereler hiçbir zaman gruba alınmaz, hook'lar yok sayar. |

## Mimari

```
ztile.exe (tek süreç, tek UI thread)
├─ layout     : bölme ağacı (H/V split + oranlar, yapraklar = hücre), rect hesaplama
├─ winapi     : pencere listeleme/filtreleme, place() (görünmez kenarlık telafisi), borderless
├─ workspace  : workspace = layout + monitör + hücre↔hwnd; WinEvent hook'ları
├─ taskbar    : temsilci pencere, birleşik ikon, DeleteTab ile üye butonlarını gizleme
├─ placeholder: boş hücrede "+" penceresi ve seçici
├─ store      : %APPDATA%\ZTile\layouts.json
├─ launcher   : uygulama başlatma + yeni pencereyi yakalama (exe/sınıf/AUMID/başlık)
└─ ui         : ana pencere, preset galerisi, editör, tray, hotkey, tek-instance (named pipe)
```

### Odak / z-order modeli (çoklu workspace)
- Her workspace bir monitöre bağlıdır. Bir monitörde aynı anda yalnızca bir workspace "aktif" görünür.
- Workspace B öne alındığında aynı monitördeki A'nın pencereleri **minimize edilir** (yalnızca arkada bırakmak yerine). Böylece taskbar davranışı tutarlı olur ve gizlenmiş pencereler tıklamaları yakalamaz.
- Farklı monitörlerdeki workspace'ler birbirinden bağımsızdır (laptop'ta geçiş yaparken monitördeki oyun etkilenmez).

### Sürükleme mantığı
- `EVENT_SYSTEM_MOVESIZESTART` ile sürükleme başlar, `MOVESIZEEND` ile biter. Bırakma noktası imleç konumuna göre değerlendirilir:
  - İmleç başka bir hücrenin içindeyse → swap
  - İmleç workspace alanının dışındaysa → kopar, hücreye "+"
  - Diğer durumlarda (kendi hücresi / küçük kayma) → snap ile yerine dön
- Kenardan boyutlandırma (move değil size) → Faz 7'de komşu hücrelerin oranını ayarlar. O zamana kadar snap ile geri döner.

## Faz 0 bulguları (2026-10-31)
- Exe 182 KB, açılış ~10 ms, grup çalışırken RAM ~1,6 MB.
- Birleşik taskbar ikonu: görünüm yeterli. `DeleteTab` Explorer yeniden başlatılınca da kalıcı (`TaskbarCreated` ile yeniden uygulanıyor).
- Swap / kopma / snap ve "+" seçici çalışıyor.
- Win11 snap edilmemiş pencerelerin köşelerini yuvarlıyor, bu yüzden aralarda boşluk varmış gibi görünüyor → üye pencerelere `DWMWCP_DONOTROUND` uygulanıyor, çıkışta geri alınıyor.
- Köşe ayarı yalnızca odaktaki pencerede görünüyordu → ayardan sonra `SWP_FRAMECHANGED` ile çerçeve yeniden çizdiriliyor.
- Borderless: Chromium/Electron uygulamaları (Brave PWA, Discord, Spotify) başlığı kendi içeriğine çiziyor, stil değişikliği işe yaramıyor. `SetWindowRgn` ile üst şeridi kırpma denendi ama Discord/Spotify'ın oradaki butonları da gidiyor → **kullanıcı kararıyla kaldırıldı**. Borderless yalnızca klasik başlıklı uygulamalarda çalışır.
- Taskbar tıklama senkron hatası: (1) önde/arkada durumu geciken olaylardan tahmin ediliyordu → tıklama anında z-order'a bakılıyor. (2) Toggle'ı temsilcinin odak almasına bağlamak hatalı: Windows ona başka yollardan da odak veriyor (ör. terminal küçülünce), bu da grubu rastgele küçültüyordu → **temsilci pencere her zaman küçültülmüş tutuluyor**. Her taskbar tıklaması "geri yükle" (`SC_RESTORE`) olarak geliyor, temsilci geri yüklenmeden toggle yapılıyor. Pencereler odak almadan TOPMOST→NOTOPMOST yöntemiyle öne getiriliyor.

## Fazlar

### Faz 0 — Prototip ve risk avı ✅ (2026-10-31, kullanıcı testleri geçti)
- Cargo iskeleti, release profili (`opt-level="z"`, LTO, `panic=abort`, strip)
- Deneyler: `DeleteTab` kalıcılığı (Explorer yeniden başlatma dahil), dinamik birleşik ikon, Chrome/UWP/Electron piksel hassasiyetinde yerleştirme
- Çıktı: hangi yaklaşım tutuyor raporu → gerekirse plan revizyonu

### Faz 1+2 durumu (2026-10-31)
Kod tamam, 13 birim testi geçiyor; kullanıcı testi bekleniyor.
- `layout.rs`: bölme ağacı (`Cell` / `Split{cols|rows, sizes, children}`), komşu hücreler aynı kenar değerini paylaşır (piksel boşluğu/çakışması yok), metin biçimi `cols(60:c, 40:rows(c, c))`.
- `json.rs`: serde'siz minimal JSON (parse + pretty print).
- `store.rs`: `%APPDATA%\ZTile\layouts.json`, atomik yazma (tmp + rename), bozuk dosya `.bad` olarak kenara alınır.
- `apps.rs`: uygulama kimliği = exe + AUMID + sınıf + başlık. Brave PWA'ların (WhatsApp, Instagram) her birinin ayrı AUMID'si var (doğrulandı), başlık değişse de doğru pencere bulunur. Eşleştirme en güçlü eşleşmeden başlayarak açgözlü atama ile yapılır.
- Monitör kimliği: GDI cihaz adı + çözünürlük, bulunamazsa çözünürlük, sonra birincil.
- CLI: `grid --save AD`, `open AD`, `layouts`, `delete AD`, `--layout` özel ağaç kabul eder.

### Faz 3 durumu (2026-10-31)
Kod tamam, kullanıcı testi bekleniyor. `group.rs` → `workspace.rs`.
- Tek süreç (daemon): gizli mesaj penceresi `ZTileDaemon`; sonraki `grid/open/quit` komutları WM_COPYDATA ile ona iletilir, ana döngüde işlenir (iç içe çağrı yok).
- Birden çok workspace; her birinin kendi temsilci penceresi ve taskbar butonu var. Aynı monitörde (GDI cihaz adı) bir workspace öne gelince diğerleri küçülür.
- Bir pencere tek workspace'e ait olabilir; başka gruba eklenirse eskisinden çıkar.
- Sürükleme göstergesi: üstte duran, tıklamayı geçiren yarı saydam pencere (mavi = swap/doldur, gri = yerine dön, kırmızı = gruptan çık). Yalnızca sürüklenen pencerenin konum olayları işlenir.
- Gruplar arası sürükle-bırak (iki pencere gruplar arasında yer değiştirir), gruba ait olmayan pencereyi boş "+" hücresine sürükleyerek ekleme.
- Yerleşme animasyonu henüz yok (sonraya).

### Arayüz adımı durumu (2026-10-31) — Faz 5/6/8'den parçalar öne çekildi
Kullanıcı kararı: uygulama terminalsiz, tamamen arayüzden kullanılacak. Kod tamam, 17 test geçiyor, kullanıcı testi bekleniyor.
- `#![windows_subsystem = "windows"]`: konsol yok. Argümansız açılış = tepsi + ana pencere; ZTile zaten çalışıyorsa sadece pencereyi öne getirir. `--tray` = sadece tepsi (otomatik başlangıç). CLI komutları hata ayıklama için duruyor (üst konsola bağlanır, gerekirse tepsi sürecini başlatır).
- `tray.rs`: tepsi simgesi (kendi çizdiğimiz 2×2 ikon), menü: aç, kayıtlı dizilimler (açık olanlar işaretli), Windows ile başlat (HKCU Run), çıkış. Explorer yeniden başlayınca simge geri eklenir.
- `ui.rs`: GDI ile çift tamponlu ana pencere, sistem açık/koyu temasını izler. Ana sayfa: dizilim kartları (önizleme + uygulama ikonları, "açık" işareti), tıkla = aç, sağ tık = öne getir/düzenle/kapat/sil. Editör: ad, ekran seçimi, hazır dizilimler, büyük önizleme (hücreye tıkla: açık pencereden/programdan uygulama seç, böl, sil; çizgiyi sürükle: oran), başlıksız mod, hücre boşluğu, Kaydet / Kaydet ve aç. Pencere kapanınca tüm UI kaynakları serbest bırakılır.
- Eksik uygulamaları başlatma: AUMID varsa `shell:AppsFolder\<AUMID>` (Store ve tarayıcı PWA'ları), yoksa exe. Yeni pencere 30 sn boyunca 250 ms'lik zamanlayıcıyla aranır (yalnızca bekleyen başlatma varken çalışır). Açılış ekranı kapanırsa gerçek pencere beklenmeye devam edilir.
- Hücre uygulamasını hatırlar: uygulama kapanınca "+" menüsünde "▶ X başlat" çıkar. Kayıtlı gruptaki sürükleme / "+" değişiklikleri dizilime otomatik kaydedilir (uygulamanın kapanması kaydı değiştirmez).
- Program seçici: .exe veya Başlat menüsü kısayolu (.lnk → hedef exe + AUMID okunur).
- Akıllı Uygulama Denetimi (SAC) açık olan bir sistemde imzasız derlemeler engellenir; geliştirme için SAC'ın kapalı olması gerekir (README'de anlatıldı).
- Kural: dışarıdan bağımlılık yok (yalnızca Microsoft'un `windows` crate'i); bir şey eklemek gerekirse önce kullanıcıya sorulur.

### Güvenlik ve performans taraması (2026-10-31)
Düzeltilenler:
- Exe sertleştirme (`.cargo/config.toml`): bağımlı DLL'ler yalnızca System32'den (`/DEPENDENTLOADFLAG:0x800`, DLL planting'e karşı), Control Flow Guard, CET uyumlu. ASLR/DEP zaten açık. `dumpbin` ile doğrulandı.
- Çalışma zamanı: uzak/düşük bütünlüklü konumlardan DLL yükleme kapalı, System32 öncelikli, eski enjeksiyon noktaları (AppInit, başka uygulamaların global hook'ları) kapalı.
- Girdi sınırları: JSON derinliği ≤ 64, dizilim derinliği ≤ 8, hücre ≤ 16, kayıt dosyası ≤ 1 MB, ≤ 200 dizilim, oranlar sonlu ve sınırlı, boşluk 0–64 (yığın taşması / kaynak tüketimi ile çökertmeye karşı).
- Başlatma: yalnızca yerel diskte var olan `.exe` ya da geçerli biçimde AUMID (betik, belge, ağ yolu, shell namespace'te gezinme yok). Program seçici yalnızca .exe/.lnk.
- IPC: daemon penceresi ztile.exe'ye ait mi doğrulanıyor (sahte pencereyle engelleme yok); WM_COPYDATA boyut/biçim kontrolü, yalnızca grid/open/show/quit. `AllowSetForegroundWindow` herkese değil sadece ZTile'a.
- Panik durumunda (panic=abort) üye pencerelerin taskbar butonları, köşeleri ve küçültme durumu geri veriliyor.
- Performans: konum/kapanma olayları artık sistem genelinden değil yalnızca üye süreçlerden ve sürüklenen pencereden dinleniyor (önceden her fare hareketi ZTile'ı uyandırıyordu). Ölçüm: tepside 10 sn boşta 0,0 ms CPU, 2,0 MB RAM.

Kabul edilen / açık kalanlar:
- Aynı kullanıcı oturumundaki bir program ZTile'a komut gönderebilir (yalnızca zararsız komutlar; zaten aynı yetkiye sahip). Düşük bütünlüklü süreçler UIPI ile engelli.
- ZTile Görev Yöneticisi'nden zorla kapatılırsa gizlenen taskbar butonları Explorer yeniden başlayana kadar geri gelmez.
- ~~`layouts.json` pencere başlıklarını saklıyor~~ → çözüldü: artık yalnızca uygulama adı saklanıyor; başlık parçası sadece kullanıcı bir hücreyi belirli bir pencereye sabitlerse tutuluyor.

### İngilizce arayüz ve uygulama adları (2026-10-31)
- Arayüz, menüler, CLI ve README İngilizce. Kod yorumları zaten İngilizce.
- `AppSpec`: `title` alanı kaldırıldı → `name` (görünen uygulama adı) + `window` (isteğe bağlı başlık filtresi). Ad kaynağı sırası: pencerenin RelaunchDisplayName özelliği → AUMID'nin Başlat menüsü adı (PWA'lar ve Store uygulamaları: "WhatsApp Web", "Instagram") → exe'nin FileDescription'ı ("Discord") → exe adı. Eski dosyalardaki `title` okunurken atılır.
- Editör hücre menüsü: "Open apps" (uygulamanın herhangi bir penceresi), "A specific window" alt menüsü (başlık filtresi, başındaki "(3)" gibi sayaçlar atılır), "Match any window" (filtreyi kaldır).
- Repoda makineye özel yol/ayar yok; kayıtlar `%APPDATA%\ZTile` altında, ilk açılışta boş başlar.

### Dizilimler sabit (2026-10-31, kullanıcı kararı)
- Kayıtlı dizilim yalnızca editörden değişir. Açık grupta sürükleme / "+" / gruptan çıkarma sadece o anki grubu etkiler.
- Dizilim başına isteğe bağlı ayar: "Save changes made in the open group" (`keep_changes`, varsayılan kapalı; eski dosyalarda da kapalı sayılır). Açıksa eski davranış: canlı değişiklikler kayda yazılır.
- Dizilim açılırken yapılan tek kayıt işlemi uygulama adlarını tazelemek (uygulama/hücre düzenini değiştirmez; eski dosyalardaki başlıkları da temizler).
- Exe imzasız (Faz 10).

### Faz 10 — Yayın
- GitHub Releases: taşınabilir zip, README'de SmartScreen "Yine de çalıştır" adımı.
- Uyarısız dağıtım seçenekleri (karar sonra): Microsoft Store (MSIX, Microsoft imzalar, bireysel hesap ücretsiz), SignPath Foundation (açık kaynak için ücretsiz imza), ücretli sertifika. winget paketi tamamlayıcı olarak.
- Exe'ye sürüm bilgisi ve ikon kaynağı gömme.

### Faz 1 — Pencere motoru
- Gerçek uygulama penceresi filtresi (görünür, sahipsiz, tool window değil, cloaked değil, tam ekran değil)
- `place(hwnd, rect)`: `DWMWA_EXTENDED_FRAME_BOUNDS` ile kenarlık telafisi, restore, `DeferWindowPos`
- Per-Monitor DPI v2, monitör listeleme (çalışma alanı)
- Borderless: stil kaydet/geri yükle

### Faz 2 — Dizilim modeli ve presetler
- Bölme ağacı, gap ayarı, rect hesaplama
- Presetler: tam, 2 dikey, 2 yatay, 3 dikey, 3 yatay, 2×2, 1+2, 1+3, 2+1
- JSON serileştirme

### Faz 3 — Workspace çalışma zamanı
- Hook'lar: destroy → hücre boşalır, location change → geri koy, move/size → sürükleme mantığı
- "+" placeholder ve seçici (açık pencereler + yeni uygulama başlat)
- Swap / kopma / snap
- Sürüklerken görsel geri bildirim: hedef hücrede yarı saydam vurgu (swap = mavi, kendi hücresi = snap gölgesi, grid dışı = "kopacak" işareti), bırakınca kısa bir yerleşme animasyonu. Yalnızca sürükleme sırasında çalışır, boştayken maliyeti sıfır.

### Faz 4 — Taskbar entegrasyonu
- Workspace başına temsilci pencere + ayrı AppUserModelID
- Birleşik ikon (16/24/32/48 px, DPI'a göre), hover'da dizilim önizlemesi
- Üye butonlarını gizle; çıkışta/çökmede mutlaka geri yükle; `TaskbarCreated`'da yeniden uygula
- Tıklama: arkadaysa öne getir (aynı monitördeki diğer workspace'i minimize et), öndeyse hepsini minimize et

### Faz 5 — Kalıcılık ve geri yükleme
- `layouts.json`: ağaç, monitör, hücre başına uygulama tanımı
- Dizilimi aç: açık eşleşen pencereleri kullan, eksikleri başlat ve yakala (zaman aşımlı)
- `ztile.exe --open "İş"` kısayolları, tek instance (named pipe)

### Faz 6 — Ana pencere / UI
- Preset galerisi, kayıtlı dizilim kartları, monitör seçimi, editör
- Açık/koyu tema; kapatınca tray'e iner ve UI kaynaklarını serbest bırakır

### Faz 7 — Ayarlanabilir dizilim
- Canlı grupta kenar sürükleyince komşular birlikte boyutlanır; oranı kaydetme
- Hücre bazında: borderless, "zorlama" (serbest pencere)

### Faz 8 — Konfor
- Global kısayollar, tray menüsü, Windows ile başlama (Run anahtarı)
- Monitör takma/çıkarma, uyku/uyanma, Explorer yeniden başlaması

### Faz 9 — Sağlamlaştırma ve optimizasyon
- Zor uygulamalar test listesi (Chrome, Firefox, VS Code, Discord, WhatsApp, Spotify, Teams, Steam, Explorer, Terminal, UWP, yönetici pencereler)
- Minimum boyut dayatan uygulamalar için hücre içinde ortalama
- RAM/CPU/açılış ölçümü, 24 saat boşta sızıntı testi
- Panic hook ile güvenli kapanış, paketleme (portable zip / Inno Setup)
