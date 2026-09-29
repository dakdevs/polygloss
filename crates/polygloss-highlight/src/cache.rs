//! LRU token cache keyed by `(blob, language, theme)` and capped by bytes
//! (design §11.11: "Results are cached by (blob, language, theme)"; blobs are
//! immutable, so a key never goes stale).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::language::Language;
use crate::scope_map::ThemeId;
use crate::tokens::Tokens;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    /// The blob's object id (hex).
    blob: Box<str>,
    language: Language,
    theme: ThemeId,
}

#[derive(Debug)]
struct Entry {
    tokens: Arc<Tokens>,
    bytes: usize,
    /// Last use; also this entry's key in `TokenCache::order`.
    tick: u64,
}

/// Least-recently-used cache of highlight results, evicting until its entries'
/// bytes (token heap bytes plus key and bookkeeping) fit the budget. Not
/// synchronized: wrap it in a `Mutex` to share it.
#[derive(Debug)]
pub struct TokenCache {
    budget_bytes: usize,
    bytes: usize,
    tick: u64,
    entries: HashMap<Key, Entry>,
    /// Entries by last use, oldest first.
    order: BTreeMap<u64, Key>,
}

impl TokenCache {
    pub fn new(budget_bytes: usize) -> TokenCache {
        TokenCache {
            budget_bytes,
            bytes: 0,
            tick: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    /// The cached tokens for `blob` (object id) highlighted as `language` with
    /// theme `theme`, marking them most recently used.
    pub fn get(&mut self, blob: &str, language: Language, theme: ThemeId) -> Option<Arc<Tokens>> {
        let key = Key {
            blob: blob.into(),
            language,
            theme,
        };
        let tick = self.next_tick();
        let entry = self.entries.get_mut(&key)?;
        self.order.remove(&entry.tick);
        entry.tick = tick;
        let tokens = entry.tokens.clone();
        self.order.insert(tick, key);
        Some(tokens)
    }

    /// Caches `tokens`, replacing an entry with the same key and evicting the
    /// least recently used entries until everything fits the budget. Tokens
    /// larger than the whole budget are not cached (and evict nothing).
    pub fn insert(&mut self, blob: &str, language: Language, theme: ThemeId, tokens: Arc<Tokens>) {
        let bytes = entry_bytes(blob, &tokens);
        if bytes > self.budget_bytes {
            return;
        }
        let key = Key {
            blob: blob.into(),
            language,
            theme,
        };
        if let Some(old) = self.entries.remove(&key) {
            self.order.remove(&old.tick);
            self.bytes -= old.bytes;
        }
        while self.bytes + bytes > self.budget_bytes {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= evicted.bytes;
            }
        }
        let tick = self.next_tick();
        self.order.insert(tick, key.clone());
        self.entries.insert(
            key,
            Entry {
                tokens,
                bytes,
                tick,
            },
        );
        self.bytes += bytes;
    }

    /// Bytes accounted to the cached entries (always within the budget).
    pub fn heap_bytes(&self) -> usize {
        self.bytes
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }
}

fn entry_bytes(blob: &str, tokens: &Tokens) -> usize {
    tokens.heap_bytes()
        + std::mem::size_of::<Tokens>()
        + 2 * (blob.len() + std::mem::size_of::<Key>())
        + std::mem::size_of::<Entry>()
}
