# HW1: Copy-on-write B+tree index

Персистентний key-value store побудований на Copy-On-Write B+Tree.

## Зміст
1. [Quickstart](#1-quickstart)
2. [Загальний опис рішення](#2-загальний-опис-рішення)
3. [Структура дерева і page](#3-структура-дерева-і-page)
   - [3.1 Header-сторінка](#31-header-сторінка-srcpageheaderrs)
   - [3.2 Internal-сторінка](#32-internal-сторінка-srcpageinternalrs)
   - [3.3 Leaf-сторінка](#33-leaf-сторінка-srcpageleafrs)
   - [3.4 Free-list сторінка](#34-free-list-сторінка-srcpagefree_listrs)
4. [Copy-on-write commit](#4-copy-on-write-commit)
   - [4.1 Вставка нових значень](#41-вставка-нових-значень)
   - [4.2 Ізоляція запису](#42-ізоляція-запису)
   - [4.3 Ізоляція читання](#43-ізоляція-читання)
5. [Перевикористання простору](#5-перевикористання-простору)
6. [Переключення між різними бек-ендами (файл/RAM)](#6-переключення-між-різними-бек-ендами-файлram)
7. [Основні інтерфейси](#7-основні-інтерфейси)
   - [7.1 PageIO](#71-pageio)
   - [7.2 Store](#72-store)
   - [7.3 StorageEngine](#73-storageengine)
8. [Тести](#8-тести)

## 1. Quickstart
Проєкт доступний до запуску в [dev-контейнері](https://containers.dev/).

Запуск тестів:
```bash
cargo test
```

## 2. Загальний опис рішення

`Copy-on-write`: вставка значень ніколи не модифікує існуючі сторінки. Натомість, вставка копіює весь шлях від root-ноди до leaf-ноди в нові сторінки, при потребі сплітить ноди, які перестали поміщатись в `page_size`, і публікує нове дерево атомарною зміною `root page id` в header page. Стара версія дерева при цьому лишається валідною й доступною, що дозволяє імплементувати ізоляцію читачів і повторне використання простору.

`B+Tree`: значення лежать тільки в leaf-нодах. Внутрішні вузли зберігають тільки ключі-сепаратори і посилання на child-ноди.

Стор працює або поверх файлу (підключеного через mmap), або поверх in-memory бекенду - бекенд підмінюється через трейт `PageIo`, а вся B+Tree/COW/reclamation-логіка написана один раз.

Публічний API має два методи: 
- `StorageEngine::put(key, value)`
- `StorageEngine::get(key) -> Option<Vec<u8>>`

## 3. Структура дерева і page

Значення зберігаються тільки в листках. Внутрішні ноди зберігають `N` сепаратор-ключів і `N+1` дочірніх page id. Одна нода - одна сторінка (page), `page_size` задається при відкритті стора.

Кожен тип сторінки - header, leaf, internal, free-list - починається із спільного 1-байтного type-тегу: 
- `0x2A` header
- `0x01` leaf
- `0x02` internal
- `0x03` free-list

### 3.1 Header-сторінка (`src/page/header.rs`):

```
| tag(1B)=0x2A | format_signature(8B) | version(4B) | page_size(4B) | root_id(8B) | next_page_id(8B) | free_list_head(8B) | free_count(8B) |
```

- `format_signature` - указання формату файлу (завжди `HWBTREE1`)
- `version` - версія структури файлу, для майбутніх змін в форматі
- `page_size` - розмір пейджі, вказаний при створенні файлу
- `root_id` - id поточної root-сторінки
- `next_page_id` - курсор для page allocation. Вказує на наступну сторінку, яку можна використати для алокації.
- `free_list_head` - вказує на перший page id у ланцюгу free-list сторінок (`0` = ланцюга ще нема)
- `free_count` - кількість вільних сторінок в ланцюгу free-list

Header-сторінка завжди має `page id == 0` та оновлюється in-place на кожному коміті.

### 3.2 Internal-сторінка (`src/page/internal.rs`):

```
| tag(1B)=0x02 | page id(8B) | key count N(2B) | keys... | children... | unused |
```

- `keys`: `key_len(2B) | key`, N разів - містить тільки сепаратори. 
- `children`: `child page id(8B)`, N+1 разів

### 3.3 Leaf-сторінка (`src/page/leaf.rs`):

```
| tag(1B)=0x01 | page id(8B) | key count(2B) | entries... | unused |
```

- `entries`: `key_len(2B) | key | val_len(2B) | val`

### 3.4 Free-list сторінка (`src/page/free_list.rs`):

```
| tag(1B)=0x03 | page id(8B) | next page id(8B) | count(2B) | free page ids... | unused |
```

## 4. Copy-on-write commit
### 4.1 Вставка нових значень

`put` ніколи не виконує in-place update існуючої сторінки. Алгоритм рекурсивно спускається до потрібного листа і вставляє нове значення. Далі, на шляху назад, в залежності від `InsertResult`, відбувається наступне:
- `InsertResult.Single` - замінює child-вказівник у копії батьківської ноди
- `InsertResult.Split` - вставляє нову пару `(separator key, child pointer)` у копію батьківської ноди. Якщо ця вставка спричиняє переповнення розміру ноди - сплітить саму parent-ноду.  

Оскільки ідея сopy-on-write полягає в тому, що кожна зміна (включно зі зміною ключів в parent-нодах) створює нову копію ноди - це значить, що кожен апдейт пропагується до root-ноди. Якщо це призводить до спліту root-ноди - дерево зростає на додатковий рівень.

### 4.2 Ізоляція запису

Ізоляція запису реалізована через `WriteTxn` (`src/storage/store.rs`). `WriteTxn` буферизує записи і звільнення сторінок, і публікує їх лише при `commit(new_root)` за наступним алгоритмом:

1. Застосувати буферизовані записи сторінок до бекенду.
2. Додати orphaned сторінки цієї транзакції до `pending_free`.
3. Reclamation gate: просунути `pending_free` у придатний для повторного використання `free list` лише якщо на цей момент немає активних читачів.
4. Зберегти `free list` на диску.
5. Зберегти header-сторінку (root id + стан вільного списку) через in-place update.
6. Виконати `sync()`, після чого опублікувати новий root через атомарний запис в `AtomicU64`. В цей момент нова версія дерева стає видимою для читачів.

Клієнти серіалізуються на запис через `Mutex<WriterState>`.

### 4.3 Ізоляція читання 
Ізоляція читання виконується через `ReadGuard`, який виконує атомарне зчитування поточного кореня плюс звичайні читання сторінок. Оскільки сторінки ніколи не змінюються in-place, то читач, "захопивши" root-ноду, бачить незмінний снепшот дерева, яке досяжне з цієї root-ноди.


## 5. Перевикористання простору
Перевикористання простору реалізовано через free list. Header-сторінка (`header.rs`) містить вказівник на free_list_head, яка в свою чергу містить посилання на наступну вільну сторінку (`next page id`). `next page id == 0` - означає кінець ланцюжка вільних сторінок.  

Сторінки в free-list оновлюються через in-place update.

Алгоритм звільнення сторінок (`src/storage/store.rs`):

1. Кожна транзакція запису (`WriteTxn`) збирає список звільнених у цій транзакції сторінок (`free()`), і при `commit()` вони йдуть у `pending_free` (ще не готові до перевикористання).
2. Перед комітом перевіряється лічильник активних читачів (інкрементується в `Store::enter_read`). Якщо він дорівнює `0` - значить, всі, хто міг би бачити попередню версію, вже закінчили читання. Тоді `pending_free` переміщується у `free_list`, вже придатний до перевикористання.
3. Якщо читачі є - сторінки чекають у `pending_free` до наступного коміту, де перевірка повториться.
4. Алокація нової сторінки (`alloc()`) спочатку дивиться у `free_list` і тільки якщо він порожній - розширює файл.


## 6. Переключення між різними бек-ендами (файл/RAM)

Реалізовано через трейт `PageIo` (`src/io/mod.rs`), який містить функції для читання/запису сторінки по id, а також sync. Імплементації - `MmapPageIo` та `MemoryPageIo`.  
`Store<IO>` написаний поверх `PageIo`, тож вся COW/tree/reclamation-логіка спільна для обох бекендів.

Підміна відбувається на рівні конструктора `StorageEngine`:

```rust
// MmapPageIo - файл, mmap-бекенд
let store = StorageEngine::open("/path/to/file.db", 4096)?;

// MemoryPageIo - in-memory бекенд
let store = StorageEngine::in_memory(4096);
```

## 7. Основні інтерфейси
### 7.1 PageIO
```rust
pub trait PageIo: Send + Sync {
    fn page_size(&self) -> usize;
    fn read_page(&self, id: PageId) -> Vec<u8>;
    fn write_page(&self, id: PageId, bytes: &[u8]);
    fn sync(&self);
}
```

### 7.2 Store
```rust
impl<IO: PageIo> Store<IO> {
    pub fn open_or_create(io: IO) -> Result<Self, OpenError> { ... }
    pub fn enter_read(&self) -> ReadGuard<'_, IO> { ... }
    pub fn begin_write(&self) -> WriteTxn<'_, IO> { ... }
}
```

### 7.3 StorageEngine 
```rust
impl StorageEngine {
    pub fn in_memory(page_size: usize) -> Self { ... }
    pub fn open(path: impl AsRef<Path>, page_size: usize) -> Result<Self, OpenError> { ... }

    pub fn put(&self, key: &[u8], val: &[u8]) -> Result<(), Error> { ... }
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> { ... }
}
```


## 8. Тести

- **R7.1** (unit-тести дерева з in-memory бекендом) - в `src/storage/engine.rs` (модуль `tests` внизу файлу)
- **R7.2** (інтеграційні тести на nmap-файлі) - `tests/mmap_integration.rs`.
- **R7.3** (конкурентні тести + bounded file growth) - `tests/concurrency.rs`.

