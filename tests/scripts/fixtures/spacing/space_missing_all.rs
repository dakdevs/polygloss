// A group module with a const its `ALL` leaves out, and one without `ALL`.

pub mod gap {
    pub const INLINE: f32 = 4.0;
    pub const ORPHAN: f32 = 8.0;
    pub const ALL: &[(&str, f32)] = &[("INLINE", INLINE)];
}

pub mod stroke {
    pub const BORDER: f32 = 1.0;
}
