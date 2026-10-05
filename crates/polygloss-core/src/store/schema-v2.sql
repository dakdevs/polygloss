-- Schema v2: design §7.2's "Migration 2" block copied exactly (T6.1, ADR-0028).
-- Applied by store/migrations.rs as migration 2 (user_version 2). Never edit.

ALTER TABLE file_changes ADD COLUMN generated_attr INTEGER
  CHECK (generated_attr IN (0, 1, 2));           -- 0 unspecified, 1 set, 2 unset; NULL = row from before migration 2
