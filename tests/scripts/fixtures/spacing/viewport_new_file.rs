// path: crates/polygloss-viewport/src/never_listed.rs
//
// A viewport file no list names: rule 6 still applies.

// reject 6: a ratio
fn shrink(h: f32) -> f32 {
    0.75 * h
}
