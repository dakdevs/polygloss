use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use polygloss_highlight::{Budget, Highlighter, Language, ThemeId, TokenCache, Tokens};

use crate::support::{minimal_syntax, rust_source};

const BLOB_A: &str = "1111111111111111111111111111111111111111";
const BLOB_B: &str = "2222222222222222222222222222222222222222";
const BLOB_C: &str = "3333333333333333333333333333333333333333";
const BLOB_D: &str = "4444444444444444444444444444444444444444";

fn rust() -> Language {
    Language::from_name("rust").unwrap()
}

fn tokens(lines: usize) -> Arc<Tokens> {
    let budget = Budget {
        time: Duration::from_secs(60),
        max_lines: u32::MAX,
    };
    Arc::new(
        Highlighter::new(minimal_syntax())
            .highlight(
                rust_source(lines).as_bytes(),
                &rust(),
                &AtomicUsize::new(0),
                budget,
            )
            .unwrap(),
    )
}

/// What one entry of `t` under a 40-char blob id costs in the cache.
fn entry_cost(t: &Arc<Tokens>) -> usize {
    let mut probe = TokenCache::new(usize::MAX);
    probe.insert(BLOB_A, rust(), ThemeId(1), t.clone());
    probe.heap_bytes()
}

#[test]
fn token_cache_evicts_by_bytes() {
    let t = tokens(200);
    let cost = entry_cost(&t);
    assert!(cost >= t.heap_bytes());
    let mut cache = TokenCache::new(3 * cost);
    let theme = ThemeId(1);
    for blob in [BLOB_A, BLOB_B, BLOB_C] {
        cache.insert(blob, rust(), theme, t.clone());
    }
    assert_eq!(cache.len(), 3);
    assert_eq!(cache.heap_bytes(), 3 * cost);
    // Touch A so B is the least recently used, then overflow by one entry.
    assert!(cache.get(BLOB_A, rust(), theme).is_some());
    cache.insert(BLOB_D, rust(), theme, t.clone());
    assert_eq!(cache.len(), 3);
    assert!(cache.heap_bytes() <= 3 * cost);
    assert!(
        cache.get(BLOB_B, rust(), theme).is_none(),
        "LRU entry evicted"
    );
    for blob in [BLOB_A, BLOB_C, BLOB_D] {
        assert!(
            Arc::ptr_eq(&cache.get(blob, rust(), theme).unwrap(), &t),
            "{blob}"
        );
    }

    // A bigger entry evicts as many old ones as it needs.
    let big = tokens(500);
    let big_cost = entry_cost(&big);
    assert!(big_cost > cost && big_cost <= 3 * cost);
    cache.insert(BLOB_B, rust(), theme, big);
    assert!(cache.heap_bytes() <= 3 * cost);
    assert!(cache.get(BLOB_B, rust(), theme).is_some());
    assert!(
        cache.get(BLOB_A, rust(), theme).is_none(),
        "oldest goes first"
    );
}

#[test]
fn token_cache_replaces_same_key_and_skips_oversized_entries() {
    let small = tokens(50);
    let cost = entry_cost(&small);
    let mut cache = TokenCache::new(2 * cost);
    cache.insert(BLOB_A, rust(), ThemeId(1), small.clone());
    cache.insert(BLOB_A, rust(), ThemeId(1), small.clone());
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.heap_bytes(), cost);
    // An entry larger than the whole budget is not kept and evicts nothing.
    cache.insert(BLOB_B, rust(), ThemeId(1), tokens(2_000));
    assert_eq!(cache.len(), 1);
    assert!(cache.get(BLOB_B, rust(), ThemeId(1)).is_none());
    assert!(cache.get(BLOB_A, rust(), ThemeId(1)).is_some());
}

#[test]
fn token_cache_keys_include_language_and_theme() {
    let t = tokens(20);
    let mut cache = TokenCache::new(usize::MAX);
    cache.insert(BLOB_A, rust(), ThemeId(1), t.clone());
    assert!(cache.get(BLOB_A, rust(), ThemeId(1)).is_some());
    assert!(cache.get(BLOB_A, rust(), ThemeId(2)).is_none());
    let go = Language::from_name("go").unwrap();
    assert!(cache.get(BLOB_A, go, ThemeId(1)).is_none());
    assert!(cache.get(BLOB_B, rust(), ThemeId(1)).is_none());
    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.heap_bytes(), 0);
}
