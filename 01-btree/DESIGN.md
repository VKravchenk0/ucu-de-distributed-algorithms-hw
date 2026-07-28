# Design note

Covers, per task.md R7.4: the on-disk page format, the copy-on-write
commit protocol, and the space-reclamation scheme.

## On-disk page format

The file is divided into fixed-size pages (`page_size`, configurable at
open time). Page id `0` is always the **header page**; it also doubles as
a "null" sentinel elsewhere (e.g. "no free-list chain yet"), since no real
page is ever assigned id `0`.

Every page kind — header, leaf, internal, free-list — starts with a
shared 1-byte type tag at offset 0, so any page's kind is identifiable
from its first byte alone: `0x2A` header, `0x01` leaf, `0x02` internal,
`0x03` free-list.

### Header page (`src/page/header.rs`)

Updated in place on every commit (R2.3 — metadata isn't copy-on-write):

| offset | size | field |
|---|---|---|
| 0 | 1B | type tag (`0x2A`) |
| 1 | 8B | magic (`b"HWBTREE1"`) |
| 9 | 4B | format version |
| 13 | 4B | `page_size` |
| 17 | 8B | `root_id` |
| 25 | 8B | `next_page_id` (bump-allocator high-water mark) |
| 33 | 8B | `free_list_head` (id of the first page in the on-disk free-list chain, `0` = none) |
| 41 | 8B | `free_count` (diagnostic: total entries across the whole chain) |

A file whose page 0 doesn't start with the tag+magic (a brand-new or
zero-length file) is treated as **fresh**: `Store::open_or_create`
bootstraps it with an empty leaf root at page 1 (R3.4). A file with a
*valid* header page whose `page_size` disagrees with the caller's request,
or whose format `version` this build doesn't understand, is a distinct
case — an **error** (`PageSizeMismatch` / `VersionMismatch`), not a fresh
backend, so reopening with the wrong page size or an incompatible old
file can never silently clobber real data.

### Leaf page (`src/page/leaf.rs`)

One B+Tree leaf per page (R1.1: values live only in leaves; R3.1, R3.3):

```
| tag(1B)=0x01 | page id(8B) | key count(2B) | entries... | unused |
```

Each entry is `key_len(2B) | key | val_len(2B) | val`, back to back, no
separate offsets/pointers index table — this on-disk layout is
intentionally flat and sequential, since task.md's constraints
explicitly call out that "slotted-page architecture... is not required."
Random access and binary search (R1.5) are recovered in memory instead:
decoding a page builds a small offset cache once (one linear pass over
the page's own entries), and every `get_key`/`get_val`/search call after
that is served from the cache — the *disk* format stays simple, the
*access pattern* stays O(1)/O(log n).

The page id is embedded redundantly (it's always known from the caller's
context too) purely as an integrity check: decoding asserts the stored
id matches the id the page was read from.

### Internal page (`src/page/internal.rs`)

```
| tag(1B)=0x02 | page id(8B) | key count N(2B) | keys... | children... | unused |
```

Keys: `key_len(2B) | key`, repeated N times — separators only, no values,
no per-key pointer. Children: `child page id(8B)`, repeated **N+1**
times, fixed-width and contiguous right after the keys block.

This is the classic B-tree layout task.md's R1.1 describes ("internal
nodes hold separator keys and child pointers"): `child[0]` covers keys
`< key[0]`; `child[i]` (`0<i<N`) covers `[key[i-1], key[i])`; `child[N]`
covers `>= key[N-1]`. A node's *N* separator keys sit *between* its
*N+1* children, rather than each key being paired 1:1 with "its own"
child.

### Free-list page (`src/page/free_list.rs`)

```
| tag(1B)=0x03 | page id(8B) | next page id(8B) | count(2B) | free page ids... | unused |
```

`next page id == 0` marks the end of the chain. Entries are fixed-width
`u64`s (no offset cache needed). See "Free-list persistence" below for
how this chain is written and recovered.

### Entry size limit

`check_limits`/`max_kv_size` (`src/page/mod.rs`) enforce R1.3 ("every
key/value pair fits within a single page"). The bound comes from the
internal page: any valid internal node must be able to hold 2 max-size
separator keys plus 3 children simultaneously, so that when a child
split forces one more key into it (3 keys total), a valid 2-way split
point is always guaranteed to exist. This is a *simplification* over an
earlier design that paired each key 1:1 with its own child pointer: that
scheme's root growth could produce a brand-new root holding up to 3
separators copied from a 3-way split, which was never itself re-checked
afterward — forcing a tighter `/3` bound. With true median-key
promotion (see below), an overflowing node always produces exactly 2
outputs plus 1 promoted key, never 3, and a freshly grown root is always
minimal (1 key, 2 children) — so the bound loosens from
`(page_size - 46) / 3` to `(page_size - 39) / 2`. At `page_size=4096`
that's `2028` bytes of usable key+value space instead of `1350`.

## Copy-on-write commit protocol

`put` never mutates an existing page (R2.1). Inserting a key recurses to
the target leaf, then — on the way back up — either replaces one child
pointer in a copy of the parent (if the recursive result fit in one
page), or splices a new `(separator key, child pointer)` pair into a
copy of the parent and, if *that* overflows, splits the parent itself
via true median-key promotion: the split key is removed from both
halves and returned to be inserted one level up (a leaf split instead
*copies* its first key up, since leaves own their data and don't give
any of it away). If the root itself splits, the tree grows by one level
under a brand-new, always-minimal root (R1.6). The old version of every
copied/split page is marked *orphaned*, not reused yet (see reclamation,
below).

A `WriteTxn` (`src/storage/store.rs`) buffers all of this — page writes
and frees — and only makes it visible on `commit(new_root)`:

1. Apply the buffered page writes to the backend.
2. Fold this transaction's orphaned pages into `pending_free`.
3. **Reclamation gate** (see below): promote `pending_free` into the
   reusable free list only if no reader is currently active.
4. **Persist the free list** to its on-disk chain (see "Free-list
   persistence" below).
5. Persist the header page (root id, allocator/free-list state) — R2.3,
   updated in place.
6. `sync()` (durability), then publish the new root via a **single
   atomic store** to an `AtomicU64` (R2.2). This is the one moment a
   new tree version becomes visible to readers.

Writers are serialized by a single `Mutex<WriterState>` (R5.3 — a
single-writer model is explicitly acceptable). Readers never touch that
lock: a `ReadGuard` only ever does a wait-free atomic load of the
current root plus plain page reads, so a writer can never block or
corrupt an in-flight reader (R5.2). Because pages are never mutated in
place, a reader that captured root `R` sees a complete, unmodified
snapshot of everything reachable from `R`, no matter what the writer
does afterward.

## Reclamation scheme (R4)

Freed pages aren't reused immediately — a reader could still be
traversing a tree version that references them. `Store` tracks a
global `AtomicUsize` reader count, incremented when `enter_read()`
creates a `ReadGuard` and decremented (via `Drop`) when it's released.
A commit only promotes `pending_free` into the allocatable free list
if it observes the reader count at exactly zero at that moment;
otherwise the pages just accumulate in `pending_free` until some later
commit finds the count at zero. `alloc()` (R4.3) prefers popping from
the free list before extending the file.

**Why this is correct, not just "usually fine":** a page `P` orphaned
by the transition `V_i → V_{i+1}` is, by construction, unreachable from
`V_{i+1}` and every later root. The only way any reader could ever
dereference `P` is by having observed root `V_i` or earlier and not yet
finished its walk. The gate only promotes `P` at a commit where the
reader count is zero — at that instant, by definition, no reader is
mid-walk, so every reader that *could* have seen `V_i` has already
fully exited. Any reader that starts afterward only ever reads the
(monotonically advancing) current root, `V_{i+1}` or later — it
structurally cannot reach `P`. The check can never fire too early
(reclaiming while a relevant reader is still active), only later than
strictly necessary.

This is deliberately the simpler of two viable designs — the
alternative is per-version epoch tracking (tag each reader with the
version it started under, tag each freed batch with the version that
orphaned it, reclaim a batch once the oldest active reader has advanced
past it), which reclaims sooner under sustained concurrent read load.
The quiescence gate was chosen because R5.3 explicitly invites a
single-writer model, and task.md's own test requirements (R7.3) test
bounded growth and concurrent reads as *separate* tests rather than
requiring both to hold simultaneously under one combined stress test —
so the simpler scheme's only real cost (reclamation can be delayed
under back-to-back concurrent reads with no gap) isn't something the
required tests exercise, and "a straightforward and correct
implementation is preferred" per the task's own constraints.

### Free-list persistence

The free list (both the reusable pool and anything still waiting on the
quiescence gate) is persisted to its own dedicated chain of free-list
pages on every commit, not kept in memory only. `WriterState` tracks a
small, permanent chain of storage-page ids for this (`free_list_page_ids`)
that's allocated purely via the bump allocator — never by popping from
`free_list` itself, which would be a fixed-point problem (popping
changes the list's length, which changes how many storage pages are
needed...). Once a storage page is allocated this way it's reused
(overwritten in place, like the header page — R2.3) forever after, even
if the free list later shrinks: a small, bounded, permanent cost for a
design simple enough to obviously be correct.

What gets written each commit is the **union** of `free_list` and
`pending_free`, not just the promoted `free_list` — this is safe because
a fresh process start always begins with zero readers (R4.2's "a reader
that could have observed it" cannot survive a process boundary), so
anything still sitting in `pending_free` at the moment of the last
shutdown is unconditionally safe to reclaim by whoever reopens next. The
in-memory quiescence gate itself is untouched by this — it still governs
what's usable as `free_list` *within this process's lifetime* — only
what's persisted to disk is widened, so a process that exits mid-batch
no longer leaks those pages.

`Store::open_or_create` walks the chain from `header.free_list_head`
(following each page's `next page id` until `0`) to rebuild both the
in-memory `free_list` and the set of reusable storage-page ids on
reopen.

## The mmap backend (`src/io/file_backend.rs`)

`std` has no built-in mmap wrapper, so this backend uses the
[`memmap2`](https://docs.rs/memmap2) crate — task.md's constraint is
"no external **B-tree, storage, or KV** libraries," which a
general-purpose OS-facing mmap wrapper isn't, so pulling one in is in
bounds (unlike hand-declaring raw `extern "C"` `mmap`/`munmap`/`msync`
bindings by hand, which only reproduces what an established crate
already does, correctly, across platforms).

Specifically, `MmapPageIo` uses `memmap2::MmapRaw`: `PageIo::read_page`
/ `write_page` / `sync` all take `&self`, not `&mut self` (single
writer, many concurrent readers, all through shared references), so
mutation has to happen via raw pointer writes rather than an exclusive
`&mut [u8]`. `MmapRaw` is built for exactly that — unlike
`Mmap`/`MmapMut`, it exposes `as_ptr`/`as_mut_ptr` through `&self`
rather than through `Deref`/`DerefMut`, so `read_at`/`write_at` copy
bytes in and out through the raw pointer without ever materializing a
`&[u8]`/`&mut [u8]` over memory the OS considers externally shared.

To avoid a remap-during-concurrent-read hazard (a reader holding a raw
pointer into a mapping the writer replaces via `munmap`+`mmap`), the
mapping reserves one large, fixed virtual address range once at open
time (`DEFAULT_MAX_SIZE`, 1 GiB, via `MmapOptions::len`) and is never
remapped for the store's lifetime. Growth only ever extends the
backing file (`ftruncate`) within that reserved ceiling — cheap on
64-bit Linux, since unused reserved virtual space costs nothing
physically until the file is actually grown into it.
