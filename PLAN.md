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
