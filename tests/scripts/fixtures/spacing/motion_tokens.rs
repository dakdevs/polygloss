// path: crates/polygloss-viewport/src/motion.rs
//
// Motion's own constants live in its `tokens` module, which the lint skips;
// the rest of the motion module is scanned.

// accept: motion tokens
pub mod tokens {
    pub const NUDGE: f32 = 4.0;
    pub const SHIFT: f32 = 8.0;
    pub const CHEVRON_CLOSED_DEG: f32 = -90.0;
    pub fn slide(t: f32) -> f32 {
        cubic_bezier(0.25, 1.0, 0.5, 1.0, t)
    }
}
// reject 6: motion's rendering code is a viewport file
pub fn displacement(offset: f32) -> f32 {
    offset * 0.75
}
