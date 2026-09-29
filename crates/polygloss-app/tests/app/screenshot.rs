//! The screenshot baseline runner (`support::screenshot`): naming, pixel
//! comparison and what it writes. The captures themselves run in the `e2e`
//! binary (they need the main thread and Metal).

use image::{Rgba, RgbaImage};

use crate::support::Sandbox;
use crate::support::screenshot::{
    CHANNEL_TOLERANCE, DIFF_MARK, Verdict, baseline_name, baselines_dir, check, compare,
};

fn solid(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_pixel(width, height, Rgba([200, 180, 160, 255]))
}

#[test]
fn baseline_names_are_kebab_case_test_names() {
    assert_eq!(
        baseline_name("e2e_viewport_split_pierre_light"),
        "e2e-viewport-split-pierre-light"
    );
    assert!(baselines_dir().ends_with("crates/polygloss-app/tests/baselines"));
}

#[test]
fn compare_tolerates_two_levels_per_channel() {
    let expected = solid(10, 10);
    let mut actual = expected.clone();
    actual.put_pixel(0, 0, Rgba([202, 178, 160, 253]));
    let same = compare(&actual, &expected).unwrap();
    assert_eq!((same.differing, same.total), (0, 100));
    assert_eq!(CHANNEL_TOLERANCE, 2);

    actual.put_pixel(3, 4, Rgba([200, 180, 163, 255]));
    let one = compare(&actual, &expected).unwrap();
    assert_eq!(one.differing, 1);
}

#[test]
fn compare_marks_differing_pixels_in_the_diff_image() {
    let expected = solid(4, 2);
    let mut actual = expected.clone();
    actual.put_pixel(1, 1, Rgba([0, 0, 0, 255]));
    let c = compare(&actual, &expected).unwrap();
    assert_eq!(c.diff.dimensions(), (4, 2));
    assert_eq!(*c.diff.get_pixel(1, 1), DIFF_MARK);
    assert_ne!(*c.diff.get_pixel(0, 0), DIFF_MARK);
}

#[test]
fn compare_rejects_images_of_another_size() {
    assert!(compare(&solid(4, 4), &solid(4, 5)).is_none());
}

#[test]
fn at_most_one_pixel_in_a_thousand_may_differ() {
    let expected = solid(100, 100);
    let mut actual = expected.clone();
    for x in 0..10 {
        actual.put_pixel(x, 0, Rgba([0, 0, 0, 255]));
    }
    assert!(compare(&actual, &expected).unwrap().within_budget());
    actual.put_pixel(10, 0, Rgba([0, 0, 0, 255]));
    let over = compare(&actual, &expected).unwrap();
    assert_eq!(over.differing, 11);
    assert!(!over.within_budget());
}

#[test]
fn check_matches_within_budget_and_removes_stale_failure_output() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("baselines");
    std::fs::create_dir_all(&dir).unwrap();
    solid(8, 8).save(dir.join("e2e-demo.png")).unwrap();
    std::fs::write(dir.join("e2e-demo.actual.png"), b"stale").unwrap();
    std::fs::write(dir.join("e2e-demo.diff.png"), b"stale").unwrap();

    let verdict = check(&dir, "e2e_demo", &solid(8, 8), false).unwrap();
    assert_eq!(verdict, Verdict::Matched { differing: 0 });
    assert!(!dir.join("e2e-demo.actual.png").exists());
    assert!(!dir.join("e2e-demo.diff.png").exists());
}

#[test]
fn check_mismatch_writes_actual_and_diff_pngs() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("baselines");
    std::fs::create_dir_all(&dir).unwrap();
    solid(8, 8).save(dir.join("e2e-demo.png")).unwrap();
    let mut actual = solid(8, 8);
    actual.put_pixel(2, 2, Rgba([0, 0, 0, 255]));

    let verdict = check(&dir, "e2e_demo", &actual, false).unwrap();
    assert_eq!(
        verdict,
        Verdict::Mismatch {
            differing: 1,
            total: 64
        }
    );
    let written = image::open(dir.join("e2e-demo.actual.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(written, actual);
    let diff = image::open(dir.join("e2e-demo.diff.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(*diff.get_pixel(2, 2), DIFF_MARK);
    // The baseline is never touched without UPDATE_BASELINE.
    assert_eq!(
        image::open(dir.join("e2e-demo.png")).unwrap().to_rgba8(),
        solid(8, 8)
    );
}

#[test]
fn check_size_mismatch_writes_actual_only() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("baselines");
    std::fs::create_dir_all(&dir).unwrap();
    solid(8, 8).save(dir.join("e2e-demo.png")).unwrap();

    let verdict = check(&dir, "e2e_demo", &solid(8, 9), false).unwrap();
    assert_eq!(
        verdict,
        Verdict::SizeMismatch {
            actual: (8, 9),
            expected: (8, 8)
        }
    );
    assert!(dir.join("e2e-demo.actual.png").exists());
    assert!(!dir.join("e2e-demo.diff.png").exists());
}

#[test]
fn check_without_baseline_fails_and_writes_actual() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("baselines");
    std::fs::create_dir_all(&dir).unwrap();

    let verdict = check(&dir, "e2e_demo", &solid(8, 8), false).unwrap();
    assert_eq!(verdict, Verdict::Missing);
    assert!(!dir.join("e2e-demo.png").exists());
    assert!(dir.join("e2e-demo.actual.png").exists());
}

#[test]
fn check_with_update_rewrites_the_baseline() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("baselines");
    std::fs::create_dir_all(&dir).unwrap();
    solid(8, 8).save(dir.join("e2e-demo.png")).unwrap();
    std::fs::write(dir.join("e2e-demo.actual.png"), b"stale").unwrap();
    let mut actual = solid(8, 8);
    actual.put_pixel(2, 2, Rgba([0, 0, 0, 255]));

    let verdict = check(&dir, "e2e_demo", &actual, true).unwrap();
    assert_eq!(verdict, Verdict::Updated);
    assert_eq!(
        image::open(dir.join("e2e-demo.png")).unwrap().to_rgba8(),
        actual
    );
    assert!(!dir.join("e2e-demo.actual.png").exists());
    // A new test's first baseline is written the same way.
    assert_eq!(
        check(&dir, "e2e_new", &actual, true).unwrap(),
        Verdict::Updated
    );
    assert!(dir.join("e2e-new.png").exists());
}
