//! The SQLite store: bootstrap, WAL, `BEGIN IMMEDIATE` writes and reads (T1.10, design §7.1).

pub mod events;
pub mod migrations;
