// path: crates/polygloss-viewport/src/paint_rows.rs
//
// Rule 6 in a viewport painter: float literals other than 0.0, 1.0, 2.0 as
// a divisor and 0.5 as a factor, except in colors and opacities.

// accept: colors and opacities
let ink = hsla(0., 0., 0., 0.06);
let tint = rgba(0x00000033).alpha(0.3);
let faded = theme.foreground.opacity(0.4);
// accept: half an advance
let x = 0.5 * advance;
let y = advance * 0.5;
// accept: centring
let top = px(h / 2.0);
// accept: zero and one
let t = t.clamp(0.0, 1.0);
// reject 6: a literal clamp
let inset = inset.clamp(0.0, 4.0);
// reject 6: 2.0 as a factor
let w = 2.0 * advance;
