# swarm-core — Proje Ana Dökümanı

> Bu doküman, projenin tek referans kaynağıdır. Yeni bir oturuma başlarken önce bu okunur.
> Kod ve karar çelişirse, doküman güncellenir; kod sessizce dokümandan sapmaz.

---

## 1. Ana fikir

**Doğrulanabilir sürü koordinasyonu (verifiable swarm coordination).**

Merkezi bir komuta düğümü olmadan çalışan, ağ bölündüğünde durmayan, ve sonradan "hiçbir üye çelişkili davranmadı, hiçbir üye yetki zarfının dışına çıkmadı" iddiasını **güvenmeyen bir üçüncü tarafa kanıtlayabilen** bir drone sürüsü koordinasyon katmanı.

Bu bir otopilot değil, bir C2 (komuta-kontrol) platformu da değil. İkisinin **arasındaki** dağıtık sistemler katmanı: mevcut otonomi yığınının altına giren, platform-agnostik bir koordinasyon primitifi. "Otonomi yığınını değiştirme, sadece koordinasyonu bize devret."

### Tasarım ilkesi

> **Act optimistically, prove accountably.**
> İyimser davran, hesap verebilir şekilde kanıtla.

Sürü, izin beklemeden hareket eder. Güvenlik, eylemi *engelleyerek* değil, her eylemi *kanıtlanabilir kılarak* sağlanır. Optimistic rollup'ın fraud-proof mantığının havada uçan versiyonu.

---

## 2. Hangi problemi çözüyor

**Askeri bağlam (EW — elektronik harp):** Merkezi C2 düğümüne bağımlı sürüler jamming altında körleşir. Jamming'e dayanıklı olanlar ise her üreticinin kapalı yığınına gömülüdür. İki ihtiyaç var ve ikisi ayrı eksende:

- **Konsensüs** → "grup mutabık mı oldu?" (iletişim/sıralama katmanı)
- **Provability** → "mutabık kalınan şey yetkili miydi?" (doğrulama katmanı)

Konsensüs ikincisini garanti etmez. Protokole uygun davranan bozuk bir düğüm, pekâlâ politika-dışı bir karar üretebilir; diğerleri bunu mutabakatla kabul eder. O yüzden iki katman birden gerekiyor.

**Sivil bağlam:** Aynı çekirdek, farklı ambalaj. Sivilde "düşman" jammer değil; çıkarları çatışan ikinci taraf — paylaşımlı lojistik sahasındaki rakip firma, sorumluluktan kaçınabilecek üretici, ham telemetriyi görmeden uyum ispatı isteyen regülatör (UTM/U-space). Sivilde dağıtık-sistemler katmanı lokomotif, doğrulama katmanı opsiyon; savunmada sıralama tersine.

### Neden klasik BFT değil

Klasik BFT (Tendermint/HotStuff tarzı) **yanlış primitif**. 2f+1 quorum ister; ağ bölündüğünde azınlık partisyonu **durur**. Drone'un durması görev kaybıdır.

Gerçek gereksinim tersine: partition altında herkes hareket etmeye devam etsin, ama sonradan kimsenin çelişkili davranmadığı **ispatlanabilsin**. Yani safety, *önleme* (prevention) yerine *hesap verebilirlik* (accountability) olarak kurgulanıyor.

---

## 3. Çekirdek veri yapısı

Kritik tasarım kararı: **yayınlanan mesaj, log kaydı ve ispat nesnesi aynı şeydir.**

```rust
struct Entry {
    mission_id: [u8; 32],   // roster Merkle kökü — cross-mission replay'i engeller
    epoch: u32,             // roster versiyonu
    node: NodeId,
    seq: u64,               // bu node'un monoton log indeksi
    prev: Hash,             // H(Entry_{seq-1}) → hash chain
    deps: VersionVector,    // causal bağımlılıklar
    body: Body,             // Claim / Track / Telemetry / Certificate / PoE
    sig: Signature,         // Ed25519, kanonik encoding üzerinde
}
```

Bu tek struct aynı anda:

| Görünüm | Hangi alan sayesinde |
|---|---|
| Causal broadcast mesajı | `deps` |
| Hash chain kaydı | `prev` |
| Equivocation kanıtı malzemesi | `(node, seq)` çakışması |
| İleride ZK devresinin girdisi | tamamı |

Ayrı katmanlar yazılırsa üç kere serialize edilip üç kere imzalanır; birleştirilirse tek imza her şeyi yapar. "Koordinasyon → accountability → validity" yol haritasının ucuz kalmasının tek sebebi bu.

---

## 4. Beş bileşen

### 4.1 İmzalı causal broadcast
Kayıplı, bölünebilen ağda mesajları nedensel sırada teslim eder ve kimlik doğrular. `deps` bir version vector'dür; entry ancak `deps ≤ yerel_VV` ve aynı node'dan `seq-1` teslim edilmişse uygulanır, aksi halde causal buffer'da bekler.

Asıl iş **anti-entropy**: periyodik VV değişimi + fark tamamlama. Partition sonrası rejoin bedava gelir — VV farkı zaten "kaçırdıklarım" kümesidir. Ayrı bir merge protokolü gerekmez.

**Kritik nokta:** causal buffer sınırlı olmalı; dolunca en eskiyi düş ve anti-entropy'ye güven.

### 4.2 Task-claim CRDT
Üç veri tipi, üç zorluk seviyesi:

- **Telemetri/konum:** LWW-register, key = node'un kendisi. Sadece sahibi yazar → çakışma imkânsız.
- **Sensör track'leri:** OR-set + track başına LWW attribute. Add-wins.
- **Görev talepleri:** `Map<TaskId, ORSet<Claim>>`, deterministik kazanan: `min by (priority, logical_clock, node_id)`.

**Dürüst sınır:** Saf CRDT ile mutual exclusion **yapılamaz** — CRDT invariant korumaz, sadece yakınsar. İki node aynı görevi talep ederse ikisi de bir süre hareket eder, sonra biri geri çekilir. Geçici olarak güvensiz, nihai olarak tutarlı. `Degradable` sınıfı için tamam, `ExclusiveCostly` için değil.

Tüketilebilir kaynak için doğru primitif **escrow / bounded counter** (bkz. 4.5).

**Kritik nokta:** tombstone GC'si için causal stability kullan; yoksa state monoton büyür ve uzun görevde bellek biter.

### 4.3 Per-node hash chain
Yazma yolu zincir (`prev`), ispat yolu **MMR (Merkle Mountain Range)**. MMR append-only, yeniden dengeleme yok, O(log n) inclusion ve consistency proof veriyor. Düz zincirle "bu karar log'umda var" demek tüm zinciri göndermeyi gerektirir; MMR'la 10-15 hash yeter.

**Cross-signing:** komşular periyodik olarak birbirinin head'ini imzalar. Kendi log'unu kendin tutarsan sonradan yeniden yazabilirsin; dışarıdan tanıklık bunu engeller. Certificate Transparency'nin gossip protokolü aynı problemi çözüyor — oradan adapte et, sıfırdan tasarlama.

**En tehlikeli kriter — crash monotonicity:** Node çöküp log kuyruğunu kaybeder ve aynı `seq`'i yeniden kullanırsa **kazara equivocate eder** ve kendini suçlu ilan eder (PoS'taki slashing-on-restart problemi). Çözüm: gönderimden *önce* `seq`'i fsync'le yaz (write-ahead), veya secure element'teki monotonic counter'ı kullan. Baştan doğru yapılmazsa saha testinde açıklanamaz hatalar gelir.

### 4.4 Equivocation detector
Aynı `(node, seq)` için iki farklı imzalı entry görüldüğünde kanıt üretir. **Kanıt = iki imzanın kendisi.** Başka hiçbir şey gerekmez, self-verifying, ~200 byte.

Çok değerli sonuç: kanıt kendi kendini doğruladığı için **suçlu node'u dışlamak konsensüs gerektirmez.** "Kanıtlanmış hatalı node'lar" kümesi grow-only set'tir — CRDT'nin ta kendisi. BFT'nin quorum ihtiyacından kaçmanın sebebi de bu.

**Dürüst sınırlar (değerlendiriciye sorulmadan söylenmeli):**
- Eclipse edilmiş bir node iki partisyona farklı yalan söyler ve partisyonlar hiç buluşmazsa tespit hiç gerçekleşmez. Accountability *post-hoc*'tur, önleyici değil.
- Hash chain, ifadelerin **tutarlılığını** ispatlar, **doğruluğunu** değil. Bir node tüm sensör verisini uydurup mükemmel tutarlı bir zincir üretebilir. Sensör doğruluğu ayrı problem (çapraz füzyon, zkML) ve bu katmanın kapsamı dışında.

### 4.5 Tutarlılık sınıfı politikası
Runtime kontrolüyle değil, **tip sistemiyle** zorlanır:

```rust
trait Action { const CLASS: Class; type Cert; }

enum Class { Degradable, ExclusiveCostly, SafetyCritical }

// Effect üretmenin TEK yolu bu — kanıt olmadan derlenmiyor
fn commit<A: Action>(a: A, cert: A::Cert, log: &mut Log) -> Effect
```

| Sınıf | Örnek | `Cert` | Not |
|---|---|---|---|
| `Degradable` | formasyon, ISR, röle | `()` | Lokal karar, CRDT yakınsar |
| `ExclusiveCostly` | tüketilebilir kaynak | `QuorumCert` | Partisyon-içi, lidersiz, 1 RTT |
| `SafetyCritical` | angajman yetkisi | `OperatorSig \| GlobalThresholdCert` | Toplayamıyorsan yapamazsın |

**Kritik nokta:** `QuorumCert` **global mutual exclusion vermez** — iki partisyon ayrı ayrı quorum kurabilir. Gerçek garanti escrow'dan gelir; quorum sadece escrow'u partisyon içinde yeniden dağıtır.

`SafetyCritical` reddedilirse bu da log'a yazılır: "yetkiyi istedim, alamadım, kanıtı burada." ROE anlatısı için bu kayıt, eylemin kendisi kadar değerli.

Sertifika formatı: N ≤ 20 için bitmap + N adet Ed25519 imzası. BLS threshold'a ancak link bütçesi zorlarsa geçilir (pairing doğrulaması ~1-2 ms, karmaşıklık ciddi).

---

## 5. Crate mimarisi

```
swarm-core/      # no_std, sıfır I/O — saf durum makinesi
  wire/          # Entry, kanonik encoding, domain-separated imzalama
  causal/        # VV, causal buffer, anti-entropy delta
  log/           # hash chain + MMR, crash-safe seq
  state/         # LWW, OR-set, escrow counter
  policy/        # Class, Action trait, commit gating
  fault/         # PoE üretimi/doğrulaması, faulty-set
swarm-net/       # Zenoh / UDP adaptörleri            [Faz 2]
swarm-sim/       # deterministik simülatör + jammer modeli
swarm-verify/    # offline replay + invariant checker
swarm-node/      # binary: ROS 2 / MAVLink köprüsü     [Faz 2]
```

### En önemli mimari karar: `swarm-core` sans-I/O ve deterministik

```rust
fn step(state: &State, ev: Event, now: LogicalTime) -> (State, Vec<Effect>)
```

İçeride ağ yok, saat yok, rastgelelik yok — hepsi dışarıdan enjekte edilir. Üç getirisi var:

1. **Deterministik simülasyon testi.** Binlerce seed ile partition/reorder/drop kombinasyonları koşturulur. Robotikçiler "on kere uçurduk, çalıştı" der; burada invariant ihlali aranmış olur.
2. **Replay = birebir yeniden üretim.** Kaza sonrası log beslenip aynı kararlar çıkarılır. Kara kutu iddiasının teknik temeli.
3. **Faz 3 bedavaya gelir.** `swarm-verify`'ın yaptığı replay, SP1/RISC Zero içine konduğu anda ZK ispatı olur. Devre yeniden yazılmaz — aynı fonksiyon farklı hedefte derlenir. Verifiability iddiası ancak bu karar baştan alınırsa ucuz kalır; sonradan retrofit imkânsıza yakındır.

---

## 6. Korunacak invariantlar

Hem test hedefi, hem model checker girdisi, hem ileride ZK devresinin kendisi:

| # | Invariant |
|---|---|
| **I1** | Her `(node, seq)` için en fazla bir imzalı entry |
| **I2** | Entry, `deps`'i teslim edilmeden uygulanmaz |
| **I3** | Aynı entry kümesini görmüş iki node aynı türetilmiş duruma sahiptir |
| **I4** | Tüm partisyonlarda harcanabilir hakların toplamı ≤ yetkilendirilen toplam |
| **I5** | Geçerli sertifika log'da yoksa safety-critical effect üretilmez |
| **I6** | Üretilen her effect, imzalı bir entry zincirine geri izlenebilir |

---

## 7. Gözden kaçırılırsa canını yakacak kriterler

| Kriter | Risk |
|---|---|
| Kanonik encoding | İki farklı byte dizisi aynı struct'a decode olursa **sahte equivocation** üretilir. Tek kanonik form + domain separation tag (`b"SWARM_ENTRY_V1"`) zorunlu |
| Duvar saati | Tie-break **asla** wall clock'a bağlanmaz — GPS spoof edilebilir, yani saldırgan claim yarışını kazanır. Logical clock + node_id |
| Roster churn | Dinamik üyelik = reconfiguration protokolü. Görev-kapsamlı statik roster + operatör imzalı epoch ile başla; karmaşıklığın %90'ı gider |
| Bellek sınırı | Causal buffer, tombstone, log — üçü de sınırlı ve sınırı ispatlanabilir olmalı |
| Real-time izolasyon | Koordinasyon katmanı uçuş döngüsünü **bloklamamalı**. Ayrı thread, sınırlı kuyruk, dolunca düş. Katman çökerse drone güvenli otonom davranışa düşmeli. Robotik tarafının ilk sorusu bu olacak |
| Bant genişliği bütçesi | Baştan hesapla: `(VV + imza + payload) × Hz × N`. Link kapasitesini aşıyorsa mimari yanlış, optimizasyon kurtarmaz |

---

## 8. Fazlar (özet)

| Faz | Kapsam | Durum |
|---|---|---|
| **Faz 1** | Saf Rust, simülasyonda 5 node. İmza + Merkle + causal broadcast + CRDT + equivocation detection + escrow. Donanım yok, ağ yok, ZK yok. | **Aktif** |
| **Faz 2** | Gerçek transport (Zenoh) + basit fizik (gym-pybullet-drones / PX4 SITL). `swarm-core` değişmez, sadece etrafına adaptör takılır. | Sonra |
| **Faz 3** | TEE attestation (OP-TEE / TrustZone): karar mantığı attested binary olarak koşar, çıktı "onaylı binary üretti" imzasıyla çıkar. | Sonra |
| **Faz 4** | IVC / folding (Nova-tarzı, Sonobe). `swarm-verify`'ın replay'i succinct proof'a dönüşür. Devre içeriği NN değil — geofence, yetki zarfı, sequence bütünlüğü gibi küçük deterministik predicate'ler. | Ufuk |
| **Faz 5** | zkML (perception modelinin kendisinin ispatı). Konseptin dışında. | Ufuk |

---

# 9. Faz 1 yol haritası

## Faz 1'in tek hedefi

> **Hiç uçmadan, hiç donanım almadan, tek bir Rust crate'i içinde: 5 simüle drone'un, ağ bölündüğünde çalışmaya devam ettiğini, birleştiğinde çelişkisiz yakınsadığını ve içlerinden birinin hile yaptığını matematiksel olarak kanıtladığını gösteren çalışan bir demo.**

Bu cümlenin dışında kalan her şey Faz 1 değil. Bu cümle pusula olsun; "şunu da ekleyeyim" denilen her an, bu cümle tekrar okunmalı.

---

## Kapsam dışı (bilinçli olarak "hayır" dediklerimiz)

Overload'ı önleyen şey yapılacaklar listesi değil, **yapılmayacaklar listesidir**. Faz 1'de bunların hiçbiri yok:

| Yok | Neden erteliyoruz |
|---|---|
| Gerçek donanım, gerçek uçuş | Katkı koordinasyon mantığında; uçuş kısmı zaman ve para yakar, hiçbir şey öğretmez |
| ROS 2, Zenoh, PX4, Gazebo | Bunlar *transport* ve *fizik*. İkisi de bu projenin problemi değil, ve ikisi de kurulum/bağımlılık cehennemi |
| Gerçek soket, gerçek thread, async/tokio | Determinizmi öldürür — aşağıda açıklanıyor, bu Faz 1'in en kritik kararı |
| ZK, TEE, folding, zkVM | Faz 3. Şimdi dokunulursa aylar yenir |
| Dinamik üyelik (drone katılma/ayrılma) | Roster (sürü üye listesi) görev başında sabit. Karmaşıklığın %90'ı buradan gelir |
| Gerçek sensör, gerçek görev, gerçek harita | Görev = soyut bir `TaskId`. Konum = 2B'de bir sayı çifti. Bu yeterli |
| Performans optimizasyonu | 5 node, saniyede 10 mesaj. Her şey yeterince hızlı |
| GUI, görselleştirme | Terminal çıktısı ve test sonucu yeter |

---

## Bağımlılıklar (toplam 5 crate)

```
ed25519-dalek    # imza
blake3           # hash
serde + postcard # serialization (verinin byte dizisine çevrilmesi)
proptest         # property-based test
rand             # seed'li (tekrarlanabilir) rastgelelik
```

**`turmoil` veya `madsim` kullanma.** Bunlar tokio (Rust'ın async/eşzamanlılık kütüphanesi) üzerine kurulu ve async yazmaya zorlar. Faz 1'de gereken simülatör ~150 satırlık düz bir `for` döngüsü: bir mesaj kuyruğu, bir node listesi, bir seed'li rastgele sayı üreteci. Daha basit, daha hızlı, %100 deterministik. Turmoil, gerçek ağa çıkarken düşünülür.

---

## Yol haritası: 6 kilometre taşı

Sıralamanın mantığı: **tasarımı en erken çürütebilecek şeyi en önce yap.** Yanlış bir kararı 5. haftada keşfetmek 1. haftada keşfetmekten 20 kat pahalı.

### M0 — Simülatör iskeleti (protokolden ÖNCE)

Henüz hiçbir protokol yok. Sadece:

```
Node listesi + mesaj kuyruğu + tick döngüsü + seed'li kayıp/gecikme/partition modeli
```

Node'un içi şimdilik boş; sadece "aldığım mesajı sayıp geri yolluyorum" kadar. Amaç sadece: kanalın kendisini test etmek.

**Neden en önce:** Simülatör sonra yazılırsa, protokol ister istemez `std::net` ve `thread::sleep` ile yazılmış olur ve bir daha deterministik hale getirilemez. Bu, geri dönülemez bir karardır. Sıfırıncı gün kararı olmak zorunda.

> *Determinizm ne demek:* aynı seed (rastgelelik tohumu) ile program 100 kere çalıştırıldığında **birebir aynı** olayların, aynı sırayla olması. Bir hata yakalandığında "seed 4271'de kırılıyor" denebilmesi ve o hatanın istendiği kadar tekrar üretilebilmesi demek. Dağıtık sistemlerde hataların %90'ı "bazen oluyor" tipindedir; determinizm o "bazen"i ortadan kaldırır.

**Bitti sayılır:** Aynı seed ile iki çalıştırma birebir aynı log'u üretiyor. Farklı seed farklı üretiyor.

---

### M1 — `Entry` + imza + hash chain (tek node)

Henüz ağ yok, tek bir node var. Node bir dizi kayıt üretiyor, her biri imzalı ve bir öncekinin hash'ini içeriyor. Ayrı bir doğrulayıcı fonksiyon, zinciri baştan sona kontrol ediyor.

**Bitti sayılır:** 1000 kayıtlık bir zincir üretiliyor ve doğrulanıyor. Ortadaki bir kaydın tek bir byte'ı elle değiştirildiğinde doğrulama **kırılıyor**. (Bu test mutlaka yazılmalı — "kurcalanamazlık" iddiasının tek somut kanıtı bu.)

---

### M2 — Causal teslimat + anti-entropy (3 node)

Version vector devreye giriyor. Mesajlar bağımlılıkları teslim edilmeden uygulanmıyor, bekleme kuyruğunda tutuluyor. Periyodik olarak node'lar VV'lerini değiş tokuş edip eksikleri tamamlıyor.

> *Anti-entropy:* "entropi karşıtı" — dağıtık sistemlerde node'ların periyodik olarak "sende ne var, bende ne var" diye karşılaştırıp farkları kapatma mekanizması. Yayın (broadcast) güvenilir olmadığı için gereklidir; kaybolan mesajları er ya da geç yakalar.

**Bitti sayılır:** 3 node {A,B} ve {C} olarak bölünüyor, 100 tick çalışıyor, birleşiyor, 50 tick daha çalışıyor → üçü de **aynı kayıt kümesine** sahip. Bu, "partition-tolerant" iddiasının ilk gerçek kanıtı.

---

### M3 — Task-claim CRDT (görev talebi)

Şimdi kayıtların bir *anlamı* oluyor: "7 numaralı görevi ben üstleniyorum". İki partisyon aynı görevi talep ederse, birleşmede deterministik bir kazanan çıkıyor, kaybeden geri çekiliyor.

**Bitti sayılır:** İki partisyon aynı görevi talep ediyor, birleşme sonrası **her iki node da aynı kazananı** hesaplıyor (kimse "ben kazandım" sanmıyor). Ve kaybeden node'un log'unda geri çekilme kaydı var.

---

### M4 — Equivocation detector (hileci node)

Simülatöre kasıtlı olarak bozuk bir node ekleniyor: aynı sıra numarasıyla iki farklı imzalı mesaj üretiyor, birini A'ya birini B'ye yolluyor.

**Bitti sayılır:** A ve B buluştuğunda, ikisi de ~200 byte'lık bir "hile kanıtı" üretiyor; üçüncü, olaya hiç tanık olmamış bir node bu kanıtı **tek başına** doğrulayıp aynı sonuca varıyor. Hiç kimseyle anlaşmaya gerek kalmadan.

Bu, demonun en etkileyici anı. Sunumda gösterilecek kare bu olacak.

---

### M5 — Escrow sayacı ve I4 invariantı

> *Escrow (emanet/tahsis):* Her drone'a görev başında sabit bir hak veriliyor — "sen en fazla 3 birim harcayabilirsin". Kendi bütçesi içinde kimseye sormadan harcıyor. Bütçe transferi el sıkışma gerektiriyor.

Bunun büyüsü: ağ **hiç** çalışmasa, sürü 5 parçaya bölünse bile, toplam harcama asla tahsis edileni aşamaz. Konsensüs gerekmeden, matematiksel olarak.

> *Invariant:* sistemin her durumunda doğru kalması gereken kural. I4 = "tüm partisyonlardaki harcanabilir hakların toplamı ≤ yetkilendirilen toplam".

**Bitti sayılır:** Rastgele partition/birleşme senaryolarında 1000 farklı seed koşuluyor, I4 hiç ihlal edilmiyor.

**Dürüstlük notu (`PHASE1-REMEDIATION.md` C3):** burada bütçe *transferi* yok
— `docs/spec.md` §13 bunu kapsam dışı bırakıyor. Yani escrow, `step` içinde
yerel bir `if remaining >= 1` kontrolüne indirgeniyor, ve I4 sadece bu
if-cümlesinin doğru olmasından ötürü tutuyor: düğüm-başına sabit, statik
tahsis, transfer yok — global sınır, yerel sınırların trivial bir sonucu.
Bounded counter'ların asıl ilginç kısmı — partition altında yeniden
dağıtım, ki handshake ve gerçek güvenlik argümanı tam orada yaşar — burada
yok; kasıtlı bir Phase 2 kararı, atlanmış bir detay değil.

Bu haliyle konsept **en satılabilir** parçasının iddia ettiğinden daha az
şey gösteriyor: "haberleşme tamamen giderse toplam harcama asla aşılmaz"
iddiası doğru ve gerçek, ama "escrow" kelimesinin çağrıştırdığı transfer/
yeniden dağıtım hikayesi yok. Savunma tarafındaki biri "ya haberleşme
tamamen giderse?" diye sorduğunda cevap hâlâ burası — ama "ya bütçemi
partner düğüme aktarmak istersem?" sorusunun cevabı Phase 2'de.

---

### M6 — Invariant checker + property test

> *Property-based testing:* Tek tek senaryo yazmak yerine, "şu kural her zaman doğru olmalı" denir ve araç binlerce rastgele senaryo üretip kuralı kırmaya çalışır. `proptest` bunu yapar.

I1–I4 çalıştırılabilir kontroller haline getirilip 5000 seed üzerinde koşturuluyor. I5 ve I6 çalıştırılabilir kontrol değil — **yapısal** olarak sağlanıyor (`crates/swarm-core/src/policy.rs`): I5, `SafetyCriticalAction`'ın `Action` trait'ini hiç implemente etmemesiyle — derleyici, sertifikasız bir safety-critical effect üretimini reddediyor, bunu kanıtlayan bir `compile_fail` doctest var. I6, `commit()`'in effect üretebilen tek fonksiyon olmasıyla. İkisi de gerçek argümanlar, ama `swarm-verify`'de koşan bir kontrol değiller — bu ayrım burada net olsun diye yazılıyor.

**Bitti sayılır:** `cargo test` → 5000 seed, sıfır ihlal. Ve bilerek bozulmuş bir versiyonda (I3: winner tie-break'i, entry setini değil gözlemleyen node'u kayıracak şekilde bozuluyor) test **kırılıyor**. Testin bir şeyi gerçekten yakaladığı kanıtlanmalı, yoksa yeşil ışık anlamsız — kanıt: `PHASE1-REMEDIATION.md` A1–A4, `crates/swarm-sim/tests/m6_property.rs::mutant_i3_detection`.

---

**Toplam:** ~600-800 satır. Akşamları çalışılıyorsa gerçekçi tahmin 4-6 hafta. Ama M0-M2'den sonra ortada zaten gösterilebilir bir şey oluyor.

---

## `Entry` struct'ı ile nasıl çalışmalı

Bu projenin temeli. Ama tam da bu yüzden **şimdi dondurulmamalı**. Yaklaşım şu olmalı:

### 1. Alanları bugünden aç, doldurmayı ertele
`mission_id` ve `epoch` gibi alanlar başta konulmalı, Faz 1'de sabit değer verilmeli. Sonradan alan eklemek, o ana kadarki bütün imzaları geçersiz kılar ve tüm test fixture'larını kırar. Şimdi 4 byte ayırmak, 3 ay sonra bir günlük acıyı önler.

### 2. İmzalanan byte'ları açık bir fonksiyon yap

```rust
fn signing_bytes(&self) -> Vec<u8>
```

`serde`'nin ne ürettiğine **güvenilmemeli**. Serde'nin çıktısı kütüphane sürümüyle, alan sırasıyla, derleyici ayarıyla değişebilir. İmzalanan byte dizisi, açıkça yazılmış, tek ve deterministik bir kural olmalı. Başına domain etiketi:

```
b"SWARM_ENTRY_V1" || sabit_sıralı_alanlar
```

> *Domain separation (alan ayrımı):* Aynı anahtarla farklı amaçlar için imza atarken, bir bağlamdaki imzanın başka bir bağlamda geçerli sayılmasını engelleyen önek. Sertifika imzası ile normal mesaj imzası birbirinin yerine kullanılamasın diye.

### 3. `Body`'yi TEK varyantla başlat
Bir enum yazılmalı ama içine sadece `TaskClaim` konulmalı. Yeni varyant ancak bir test onu talep ettiğinde eklenmeli. "İleride lazım olur" diye eklenen her varyant, henüz hiç kullanılmamışken tasarım borcu üretir.

### 4. Ham ve doğrulanmış entry'yi tip seviyesinde ayır

```rust
struct Entry { ... }           // ağdan gelen, güvenilmez
struct VerifiedEntry(Entry);   // imzası ve roster üyeliği kontrol edilmiş
```

Ve state'e giren fonksiyonlar **sadece** `VerifiedEntry` kabul etmeli. Böylece "doğrulamayı unutmak" bir runtime hatası değil, **derleme hatası** olur. Derleyiciyi güvenlik denetçisi olarak kullanmak, Faz 1'de bedava gelen en büyük kazanç.

### 5. "Altın vektör" (golden vector) dosyası tut
Bilinen bir `Entry`'nin byte-byte encoding'i ve imzası bir test dosyasına yazılmalı. Format değiştiğinde bu test kırılır — ve **kırılması iyidir**: farkında olunmadan wire formatının değiştiği anlaşılır.

### 6. Yanına `docs/spec.md` yaz, kodla birlikte güncelle
3-5 sayfa: `Entry` tanımı, teslimat kuralı, invariant listesi, tehdit modeli. İki sebepten:

- Kod yazarken kafanın karıştığı her an, spec'e bakılıp karar hatırlanır.
- Bu doküman, teknik yazı serisinin ve savunma tarafındaki ilk sunumun **hazır iskeleti** olur.

### 7. Sıralamayı ters kur: önce invariant, sonra kod
I1–I6 `invariants.rs` içine, henüz hiçbiri çalışmıyorken yazılmalı. Sonra kod bu testleri yeşile çevirmek için yazılmalı. Bu, "önce implementasyon sonra test" alışkanlığının tersi ama dağıtık sistemlerde tek işleyen yöntem — çünkü buradaki hatalar gözle görülmez, sadece invariant ihlali olarak ortaya çıkar.

---

## Faz 1 çıkış kriteri

Şu üçü doğruysa Faz 2'ye geçme hakkı var, öncesinde yok:

1. `cargo test` binlerce seed'de sıfır invariant ihlali veriyor ve **bilerek bozulmuş** bir versiyonda kırılıyor.
2. Terminalde 90 saniyede anlatılabilen bir demo var: 5 node, bölünme, çalışmaya devam, birleşme, yakınsama, hilecinin ifşası.
3. `spec.md` başkasının okuyup anlayabileceği durumda.

Faz 2'de ne olacağı **şimdi düşünülmemeli**. Ama merak edilirse: Faz 2, `swarm-core`'u değiştirmeden altına gerçek bir transport (Zenoh) ve üstüne basit bir fizik (gym-pybullet-drones) takmaktan ibaret. M0'daki sans-I/O kararı, Faz 2'yi bir haftalık iş haline getirir — o kararın asıl bedeli bugün ödenir, karşılığı sonra alınır.

---

## 10. Terim sözlüğü

| Terim | Açıklama |
|---|---|
| **Anti-entropy** | Node'ların periyodik olarak "sende ne var, bende ne var" karşılaştırıp farkı kapatması. Kayıp mesajları er ya da geç yakalar |
| **BFT** (Byzantine Fault Tolerance) | Bazı düğümlerin bozuk/kötü niyetli olabileceği varsayımı altında çalışan konsensüs. Genelde 2f+1 quorum ister |
| **Causal broadcast** | Mesajların nedensel sırada teslim edilmesi: "B, A'ya cevapsa, A'dan önce teslim edilemez" |
| **CRDT** | Conflict-free Replicated Data Type. Merkezi koordinasyon olmadan, çakışmaları deterministik olarak çözerek aynı sonuca yakınsayan veri yapısı |
| **Determinizm** | Aynı seed ile aynı çalışmanın birebir tekrarlanması. Hataların yeniden üretilebilmesini sağlar |
| **Domain separation** | İmzanın önüne konan sabit etiket; bir bağlamdaki imzanın başka bağlamda geçerli sayılmasını engeller |
| **Ed25519** | Hızlı, küçük (64 byte imza) modern dijital imza algoritması |
| **Equivocation** | Aynı sıra numarasıyla iki farklı imzalı mesaj yayınlamak. İki imzayı yan yana koymak kanıt olarak yeter |
| **Escrow** | Her node'a önceden tahsis edilmiş, koordinasyonsuz harcanabilen bütçe. Partition altında bile toplam sınırı korur |
| **Invariant** | Sistemin her durumunda doğru kalması gereken kural |
| **LoRa** | Uzun menzilli, çok düşük bant genişlikli kablosuz radyo. Protokolün "şişmanlığı" burada ciddi kısıt olur |
| **LWW** | Last-Write-Wins. En basit çakışma çözümü: zaman damgası yeni olan kazanır |
| **MMR** | Merkle Mountain Range. Sürekli büyüyen loglar için Merkle ağacı varyantı; O(log n) inclusion ve consistency proof |
| **O(log n)** | Veri 2 katına çıktığında işlemin sadece 1 adım uzaması. 1M kayıtlık log'da ispat ~20 hash |
| **OR-set** | Observed-Remove Set. Her eklemeye benzersiz etiket verir; silme sadece görülen etiketleri siler → add-wins |
| **PoE** | Proof of Equivocation — çelişkili beyan kanıtı (iki çakışan imza) |
| **Quorum certificate** | Yeterli sayıda node'un aynı şeyi imzaladığının kanıtı |
| **ROE** | Rules of Engagement — angajman kuralları, yetki zarfı |
| **Roster** | Görev başında sabitlenen sürü üye listesi (ve public key'leri) |
| **Sans-I/O** | Ağ/dosya/saat erişimi olmayan saf durum makinesi. Test ve replay edilebilirliğin ön koşulu |
| **TEE** | Trusted Execution Environment. İşlemcinin izole bölgesi; "hash'i X olan onaylı binary koştu" ispatı verir |
| **Version vector (VV)** | Her node için "kimden kaç mesaj gördüm" sayacı. Boyutu sürü büyüklüğü N ile lineer artar |
| **zkVM** | Bir programın doğru çalıştırıldığını, çalıştırmayı tekrarlamadan doğrulatan sıfır-bilgi sanal makinesi (SP1, RISC Zero) |

---

## 11. Bu projede çalışırken kurallar

Ajan/geliştirici bu dokümanla çalışırken:

1. **`swarm-core` içinde `std::net`, `std::time`, `tokio`, `rand::thread_rng` YOK.** Zaman, ağ ve rastgelelik daima parametre olarak girer. Bu kural tartışmaya açık değil.
2. **Yeni kod yazmadan önce invariantı yaz.** Hangi I'yi koruduğu belli olmayan kod yazılmaz.
3. **Kapsam dışı listesindeki bir şeyi eklemek gerekiyorsa, önce sor.** "Küçük bir ekleme" diye başlayan şey Faz 1'i öldürür.
4. **Her `Body` varyantı bir testle gelir.** Kullanılmayan varyant eklenmez.
5. **Wire formatı değişiyorsa golden vector testi güncellenir ve nedeni commit mesajına yazılır.**
6. **Tie-break, tombstone GC, buffer sınırı gibi kararlar `spec.md`'ye yazılmadan koda girmez.**
7. **Terimler açıklanarak kullanılır.** Bu proje kripto/dağıtık-sistem jargonuna aşina olmayan bir okuyucu da varsayar; yeni bir terim geçtiğinde kısa bir açıklama eklenir.
