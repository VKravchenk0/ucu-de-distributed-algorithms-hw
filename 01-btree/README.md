# HW1: Copy-on-write B+tree index

## 1. Загальний опис рішення

Персистентний key-value store побудований на Copy-On-Write B+Tree.

`Copy-on-write`: `put` ніколи не модифікує існуючі сторінки. Натомість, він копіює весь шлях від root-ноди до leaf-ноди в нові сторінки, при потребі сплітить ноди, які перестали поміщатись в page_size, і публікує нове дерево одним атомарним записом root page id. Стара версія дерева при цьому лишається валідною й доступною - саме на цьому побудована ізоляція читачів (див. пункт 5) і повторне використання простору (пункт 6).

`B+Tree`: значення лежать тільки в листках. Внутрішні вузли зберігають N сепаратор-ключів і N+1 дочірніх page id - класичний B-tree layout, а не пара "ключ-child" 1:1.

Стор працює або поверх файлу (підключеного через mmap), або поверх in-memory бекенду (для тестів) - бекенд підмінюється через трейт `PageIo` (R3.5), а вся B+Tree/COW/reclamation-логіка написана один раз і однаково працює над обома.

Публічний API має два методи: 
- `StorageEngine::put(key, value)`
- `StorageEngine::get(key) -> Option<Vec<u8>>`

## 2. Опис файлів

| Файл | Що робить |
|---|---|
| `src/lib.rs` | Підключає модулі, реекспортує публічний API (`StorageEngine`, `OpenError`, `Error`). (чи треба він нам??????????????) |
| `src/main.rs` | Демо: відкриває файловий стор, кладе пару ключів, читає їх назад. |
| `src/page/leaf.rs` | Leaf-сторінка (R1.1: значення тільки тут): серіалізація/десеріалізація (`LeafNode`), пошук (`leaf_search`), `leaf_insert`/`leaf_update`, `leaf_split_by_size`. |
| `src/page/internal.rs` | Internal-сторінка (N сепаратор-ключів, N+1 дочірніх page id): `InternalNode`, пошук дочірнього індексу (`internal_child_index`), заміна одного child (`internal_replace_child`), вставка спліт-child (`internal_insert_split_child`), спліт з підняттям медіанного ключа (`internal_split_with_promotion`). |
| `src/page/free_list.rs` | Формат free-list сторінки (page id, next page id, count, вільні page id) - encode/decode + розрахунок місткості сторінки. |
| `src/page/header.rs` | Header page файлу (тег типу, magic, версія, page_size, root_id, стан free-list-ланцюга). |
| `src/page/mod.rs` | Спільні теги типу сторінки (`0x2A`/`0x01`/`0x02`/`0x03`), enum `Page` (`Leaf`/`Internal`) з decode/serialize-диспетчеризацією між `leaf`/`internal`, `Error`, перевірка ліміту розміру запису (`max_kv_size`/`check_limits`). |
| `src/storage/tree.rs` | Сам алгоритм дерева: бінарний пошук у листку (`leaf_search`) і вибір дочірнього вузла в internal (`internal_child_index`), рекурсивний `put` (path copying + спліт з підняттям медіанного ключа для internal-вузлів), `get`. |
| `src/storage/store.rs` | `Store`/`WriteTxn`/`ReadGuard` - COW commit-протокол, атомарна публікація root, лічильник активних читачів, `Mutex` для серіалізації паралельних записів, персист free-list ланцюга на диск. |
| `src/storage/engine.rs` | `StorageEngine` - публічний фасад над `Store` + `tree`: `open`/`in_memory`, `put`/`get`, дебаг-принти. Тут же лежать unit-тести дерева (R7.1). |
| `src/storage/mod.rs` | Ре-експорт `engine`/`store`/`tree`. |
| `src/io/mod.rs` | Трейт `PageIo` (R3.5) - точка підміни бекенду, і тип `PageId`. |
| `src/io/in_memory_backend.rs` | `MemoryPageIo` - in-memory бекенд поверх `HashMap` |
| `src/io/file_backend.rs` | `MmapPageIo` - файловий бекенд поверх `memmap2::MmapRaw` (крейт, не std - обмеження task.md стосується тільки B-tree/storage/KV-бібліотек, а не загальних mmap-обгорток), з фіксованою адресою мапінгу на весь час життя стора. Весь `unsafe` для реального бекенду - це два невеликих `copy_nonoverlapping`-виклики (`read_at`/`write_at`), більше ніде. |
| `src/debug/mod.rs` | Вивід сторінок в текстовому форматі - у вигляді pretty-printed або сирих байт |
| `tests/mmap_integration.rs` | R7.2: інтеграційні тести з використанням файлового бек-енду - durability після reopen, split-heavy датасет, mismatch page_size. |
| `tests/concurrency.rs` | R7.3: конкурентні тести - багато читачів під час постійних записів + тест на обмежений розмір файлу при повторних перезаписах. |
| `tests/common/mod.rs` | `TempPath` - хелпер для тимчасового файлу стора в тестах (видаляється при `Drop`). |
| `DESIGN.md` | Короткий дизайн-нот (R7.4): формат сторінок, COW commit-протокол, схема reclamation з доказом коректності. |
| `summary.md` | Чернетка-лог по фазах розробки. Написана до фінального рефакторингу структури файлів (там ще старі назви типу `src/btree.rs`, `src/node.rs` тощо) - читати як історію рішень, а не як опис поточної структури. |

## 3. Структура дерева і page

Це B+Tree (R1.1): значення - тільки в листках, внутрішні вузли зберігають N сепаратор-ключів і N+1 дочірніх page id. Один вузол = одна сторінка (page), page_size задається при відкритті стора.

Кожен тип сторінки - header, leaf, internal, free-list - починається зі спільного 1-байтного тегу типу (offset 0): `0x2A` header, `0x01` leaf, `0x02` internal, `0x03` free-list.

**Leaf-сторінка** (`src/page/leaf.rs`):

```
| tag(1B)=0x01 | page id(8B) | key count(2B) | entries... | unused |
```

Кожен запис: `key_len(2B) | key | val_len(2B) | val`, підряд, без окремої offsets/pointers таблиці-індексу - формат навмисно плаский і послідовний (task.md явно каже, що "slotted-page architecture" не потрібна). O(1)-доступ і бінарний пошук (R1.5) відновлюються в пам'яті: при декодуванні сторінки будується легкий offset-кеш (один прохід по записах), і всі подальші звернення йдуть через нього - формат на диску лишається простим, доступ - швидким.

**Internal-сторінка** (`src/page/internal.rs`):

```
| tag(1B)=0x02 | page id(8B) | key count N(2B) | keys... | children... | unused |
```

Ключі: `key_len(2B) | key`, N разів - тільки сепаратори, без значень і без "власного" child-покажчика. Діти: `child page id(8B)`, **N+1** разів, фіксованої ширини, одразу після блоку ключів. `child[0]` покриває ключі `< key[0]`; `child[i]` (`0<i<N`) - `[key[i-1], key[i])`; `child[N]` - `>= key[N-1]`.

**Free-list сторінка** (`src/page/free_list.rs`):

```
| tag(1B)=0x03 | page id(8B) | next page id(8B) | count(2B) | free page ids... | unused |
```

`next page id == 0` - кінець ланцюга. Записи фіксованої ширини (u64), окремий offset-кеш не потрібен.

**Header-сторінка** (`src/page/header.rs`, завжди page `0`):

```
| tag(1B)=0x2A | magic(8B) | version(4B) | page_size(4B) | root_id(8B) | next_page_id(8B) | free_list_head(8B) | free_count(8B) |
```

Оновлюється in-place на кожному коміті (R2.3, метадані не COW). `free_list_head` вказує на перший page id у on-disk ланцюгу free-list сторінок (`0` = ланцюга ще нема) - див. пункт 6.

Всередині вузла ключі відсортовані, пошук - бінарний (R1.5): `leaf_search` у листку (точний збіг / точка вставки), `internal_child_index` у internal-вузлі (кількість сепараторів `<=` ключа - інша формула, ніж у листку, навмисно окрема функція).

Спліти (R1.6): якщо після вставки серіалізований вузол перевищує `page_size`, він ділиться на 2. Для internal-вузла це справжнє підняття медіанного ключа (median-key promotion): ключ-розділювач вилучається з обох половин і повертається на рівень вище, щоб бути вставленим у батьківський вузол (на відміну від листка, де перший ключ правої половини лише *копіюється* нагору - листок нічого не віддає, він і так власник усіх своїх даних). Якщо спліттиться корінь - дерево росте на один рівень під новим, завжди мінімальним коренем (1 ключ, 2 діти).

## 4. Як переключитись між різними бек-ендами (файл/RAM)

Все крутиться навколо трейта `PageIo` (`src/io/mod.rs`) - read/write сторінки по id + sync. `Store<IO>` написаний generic над ним, тож вся COW/tree/reclamation-логіка спільна для обох бекендів.

Підміна відбувається на рівні конструктора `StorageEngine`:

```rust
// реальний файл, mmap-бекенд
let store = StorageEngine::open("/path/to/file.db", 4096)?;

// in-memory бекенд (без диску, для тестів/ефемерного використання)
let store = StorageEngine::in_memory(4096);
```

`open` створює `MmapPageIo` (файл + мапінг), `in_memory` - `MemoryPageIo` (обгортка над `HashMap` під м'ютексом). Обидва ховаються за `Box<dyn PageIo>` всередині `StorageEngine`, тож зовні різниці немає - той самий `put`/`get`. Якщо треба ще один бекенд (умовно, для мережі), досить реалізувати `PageIo` для нього і додати ще один конструктор.

## 5. Як реалізована ізоляція читання

Читачі (R5.2) - lock-free: `Store::enter_read()` просто інкрементить атомарний лічильник читачів і повертає `ReadGuard`, який при `read()` бере поточний root атомарним `load` і далі просто читає сторінки. Ніякого мʼютекса читачі не чіпають взагалі - писач їх заблокувати не може в принципі.

Ізоляція випливає прямо з COW: раз існуюча сторінка ніколи не змінюється in-place, то читач, що зафіксував собі `root = R` на старті, бачить повне й незмінне дерево, яке було актуальним у момент захоплення `R` - незалежно від того, скільки `put()` встигне відбутися паралельно. Нові версії просто ростуть "поруч", зі своїми новими сторінками, поки хтось не почне читати вже новий root.

Публікація нової версії (R2.2) - один атомарний `store` в `AtomicU64` з новим root id, у самому кінці `commit()` (після того, як усі сторінки нової версії вже записані і засинхронені). Тобто читач або бачить повністю стару версію, або повністю нову - проміжного/розваленого стану не існує.

Писачі при цьому серіалізуються звичайним `Mutex<WriterState>` (R5.3 - задача явно дозволяє single-writer модель), але цей мʼютекс читачів не стосується.

## 6. Як реалізовано space reuse

Ідея: коли `put` копіює шлях від кореня до листка, старі версії скопійованих сторінок стають "сирітками" (orphaned) - вони більше не досяжні з нового root, але фізично ще лежать у файлі. Їх можна безпечно перевикористати, коли жоден читач вже не може на них наткнутися.

Механіка (`src/storage/store.rs`):

1. Кожна `WriteTxn` збирає список звільнених у цій транзакції сторінок (`free()`), і при `commit()` вони йдуть у `pending_free` (ще не готові до перевикористання).
2. Перед комітом перевіряється лічильник активних читачів (той самий, що інкрементиться в `enter_read`). Якщо він рівно 0 - значить, у цей момент точно ніхто не мандрує деревом, і всі, хто міг би бачити попередню версію, вже вийшли. Тоді `pending_free` переливається у справжній `free_list` - вже придатний до перевикористання.
3. Якщо читачі є - сторінки просто чекають у `pending_free` до наступного коміту, де перевірка повториться.
4. Алокація нової сторінки (`alloc()`) спочатку дивиться у `free_list` (R4.3) і тільки якщо він порожній - розширює файл (`next_page_id` як bump-allocator).
5. **Персист на диск**: на кожному коміті об'єднання `free_list ∪ pending_free` записується у власний ланцюг free-list сторінок (`src/page/free_list.rs`), а `header.free_list_head` оновлюється на голову цього ланцюга. Сторінки під сам ланцюг виділяються тільки через bump-allocator (ніколи не через `pop()` з `free_list`, щоб уникнути замкненої залежності "щоб знати скільки сторінок треба - треба вже знати, скільки з free_list піде на них самих"), і, раз виділені, перевикористовуються in-place назавжди. При `open_or_create` ланцюг вичитується назад у пам'ять, тож free-list переживає закриття/відкриття стора - раніше (Phase 1) це було відоме обмеження, тепер закрито.

Чому це коректно, а не "здебільшого працює": сторінка, осиротіла переходом `V_i → V_{i+1}`, недосяжна з `V_{i+1}` і всіх пізніших root. Побачити її міг тільки читач, який зафіксував root `V_i` чи раніше - і ще не закінчив обхід. А reclamation-гейт перекладає її у `free_list` тільки в момент, коли лічильник читачів == 0, тобто саме тоді, коли жоден такий читач вже фізично не може бути "в середині" обходу. Ціна такого спрощення - reclaim може трохи затриматись під безперервним потоком читачів без пауз (документовано в `DESIGN.md` як усвідомлений компроміс, а не забутий баг). Персист на диск пише об'єднання `free_list ∪ pending_free` (а не тільки вже промотований `free_list`) - це безпечно, бо новий процес завжди стартує з нульовою кількістю читачів, тож усе, що лишилось у `pending_free` на момент попереднього завершення роботи, гарантовано можна перевикористати після reopen.

Файл при цьому не пухне безмежно навіть при постійних перезаписах одних і тих самих ключів - це прямо перевіряється тестами (`repeated_overwrites_reuse_pages_via_free_list` в unit-тестах і `repeated_overwrites_keep_file_size_bounded` в `tests/concurrency.rs`), а виживання free-list через reopen - тестами `free_list_survives_reopen` і `free_list_spans_multiple_chained_pages` (`src/storage/store.rs`).

## 7. Виконання вимог

1. **R1.1** (B+Tree, значення тільки в листках) - формат сторінок в `src/page/leaf.rs`/`src/page/internal.rs`: internal-вузол зберігає N сепаратор-ключів і N+1 дочірніх page id (класичний B-tree layout), значення пише тільки листок.
2. **R1.2** (`put`/`get`) - публічний API `StorageEngine` (`src/storage/engine.rs`).
3. **R1.3** (variable-length byte[], лексикографічний порядок, запис в одній сторінці) - ключі/значення - просто `&[u8]`/`Vec<u8>`, порівняння через `Ord` на байтах; `check_limits`/`max_kv_size` (`src/page/mod.rs`) гарантують, що запис завжди влазить у сторінку.
4. **R1.4** (upsert / `get` на відсутньому ключі → `None`) - `tree::put` перевіряє, чи ключ на знайденому індексі співпадає (`leaf_update` замість `leaf_insert`); `tree::get` повертає `None`, якщо ключ не знайдено або дерево порожнє.
5. **R1.5** (бінарний пошук, O(log n)) - `leaf_search`/`internal_child_index` в `src/page/leaf.rs`/`src/page/internal.rs`, ручний бінарний пошук по індексах вузла (не лінійний скан).
6. **R1.6** (спліти + ріст висоти дерева) - `leaf_split_by_size`/`internal_split_with_promotion`, і в `tree::put` - якщо спліттиться корінь, будується новий мінімальний internal-рут над результатами спліту.
7. **R2.1** (COW, копіювання шляху) - `insert_leaf`/`insert_internal` в `tree.rs` завжди читають старий вузол і пишуть результат у нову сторінку (`txn.write` на новий `alloc()`-нутий id), стара позначається `txn.free()`.
8. **R2.2** (атомарна публікація root) - `Store::root: AtomicU64`, єдиний `store()` у кінці `WriteTxn::commit`.
9. **R2.3** (метадані - не COW) - header page (`src/page/header.rs`) і free-list сторінки (`src/page/free_list.rs`) переписуються in-place на кожному коміті.
10. **R3.1** (фіксовані сторінки, page_size конфігурований) - `page_size` передається при `StorageEngine::open`/`in_memory`, один вузол = одна сторінка скрізь у коді.
11. **R3.2** (mmap як кеш) - `src/io/file_backend.rs`: `MmapPageIo` поверх `memmap2::MmapRaw`.
12. **R3.3** (явний формат сторінки, детермінований serialize/deserialize) - `LeafNode`/`InternalNode::decode`/`to_bytes` в `src/page/leaf.rs`/`internal.rs`, з спільним 1-байтним тегом типу для всіх чотирьох видів сторінок (header/leaf/internal/free-list).
13. **R3.4** (persist + recover root id, open-or-create) - `Store::open_or_create`: валідний header → продовжуємо з нього; порожній/новий файл → бутстрап порожнього листка-рута; header з іншим page_size або несумісною version → явна помилка, а не тихе перезатирання.
14. **R3.5** (підмінний бекенд) - трейт `PageIo` (`src/io/mod.rs`), дві реалізації - `MemoryPageIo` і `MmapPageIo`.
15. **R4.1** (reclaim осиротілих сторінок, обмежений розмір файлу) - free-list механіка в `store.rs` (див. пункт 6), перевірено тестом на bounded growth.
16. **R4.2** (перевикористання тільки після виходу всіх релевантних читачів) - quiescence-гейт: `pending_free` → `free_list` тільки коли `readers == 0` (`WriteTxn::commit`).
17. **R4.3** (free list перед розширенням файлу) - `WriteTxn::alloc()` спочатку `pop()` з `free_list`, і тільки тоді інкрементить `next_page_id`; сам free list тепер ще й персистується на диск (`src/page/free_list.rs`), тож переживає reopen.
18. **R5.1** (коректність під конкурентним доступом) - покрито `tests/concurrency.rs`.
19. **R5.2** (читачі lock-free, бачать консистентний снепшот) - `ReadGuard` ніколи не бере `write_lock`, тільки atomic-load root + читання сторінок (пункт 5).
20. **R5.3** (писачі можуть серіалізуватись) - `Mutex<WriterState>` в `Store`, `begin_write()` бере лок на весь час транзакції.
21. **R6.1** (O(log n) lookup/insert, write amplification O(height)) - бінарний пошук всередині вузла + COW копіює рівно шлях від рута до листка, без зайвого.
22. **R6.2** (спліт по реальному серіалізованому розміру, сторінка ніколи не перевищує page_size) - `leaf_split_by_size`/`internal_split_with_promotion` рахують реальний `nbytes()` вузла, а не оцінку/ліміт кількості ключів.
23. **R6.3** (модульність: алгоритм / серіалізація / page management окремо) - `storage/tree.rs` (алгоритм) vs `page/leaf.rs`+`page/internal.rs`+`page/free_list.rs`+`page/header.rs` (серіалізація) vs `io/*` + `storage/store.rs` (page management/backend).
24. **R7.1-R7.4** - див. пункт 8 нижче.

## 8. Де знайти артефакти з розділу 7 (Deliverables and testing)

- **R7.1** (unit-тести дерева проти in-memory бекенду) - в `src/storage/engine.rs` (модуль `tests` внизу файлу): round-trip, upsert, missing/empty lookup, bulk-insert зі спліттом листків, малий page_size з ростом внутрішніх вузлів на кілька рівнів, межовий розмір запису. Плюс тести формату сторінок в `src/page/leaf.rs`, `src/page/internal.rs`, `src/page/free_list.rs`, `src/page/mod.rs` (round-trip, межові випадки child-індексації, коректність підняття медіанного ключа), і тести рівня `Store` в `src/storage/store.rs` (bootstrap порожнього рута, reopen на тому ж in-memory "диску", перевикористання сторінок, виживання free-list через reopen і через кілька зчеплених free-list сторінок).
- **R7.2** (інтеграційні тести проти реального mmap-файлу, durability після reopen) - `tests/mmap_integration.rs`.
- **R7.3** (конкурентні тести: читачі під навантаженням записів + bounded file growth) - `tests/concurrency.rs`.
- **R7.4** (дизайн-нота) - `DESIGN.md` в корені репозиторію.

Запустити все разом: `cargo test`. На момент написання цього README - 29 unit + 5 integration + 2 concurrency тестів, всі зелені (плюс один допоміжний `tests/sandbox.rs`, який не рахується як deliverable-тест, а просто ручний прогін з принтами для дебагу).


## Deliverables and testing
- **R7.1** - Unit tests for tree correctness against an in-memory backend: insert, upsert, leaf split, internal split, multi-level growth
юніт-тести знаходяться в `src/storage/engine.rs`, модуль `tests`
- **R7.2** Integration tests against the real mmap file backend, including reopening the file and verifying the durability of previously inserted data.
Інтеграційні тести знаходяться в `tests/mmap_integration.rs`
- **R7.3** Concurrency tests: many concurrent readers during ongoing writes (no lost or torn reads), plus a test that repeatedly overwrites keys and asserts the file size stays bounded (proving space reuse).
`tests/concurrency.rs`