//! The Sparkle loader (T5.3, library-choices §14, OQ-16). A test binary is
//! not an app bundle, so [`Updater::start`] must return `None` without
//! touching AppKit, Sparkle or the network; the bundle checks that decide
//! whether the updater may start at all are pure and run against temp
//! bundles here. The real framework is exercised by the bundle smoke test
//! (`scripts/smoke-bundle.sh`, `POLYGLOSS_BUNDLE_E2E=1`).

use std::fs;
use std::path::Path;

use polygloss_platform::sparkle::{
    self, BundleInfo, CONTROLLER_CLASS, FEED_URL_KEY, PUBLIC_ED_KEY_KEY, StartMode, Unavailable,
    Updater,
};

/// A bundle at `path` with the given Info.plist values.
fn info(path: &Path, feed: Option<&str>, key: Option<&str>) -> BundleInfo {
    BundleInfo {
        path: path.to_path_buf(),
        feed_url: feed.map(str::to_owned),
        public_ed_key: key.map(str::to_owned),
    }
}

/// `<app>/Contents/Frameworks/Sparkle.framework` as a directory.
fn embed_framework(app: &Path) {
    fs::create_dir_all(app.join("Contents/Frameworks/Sparkle.framework/Versions/B")).unwrap();
}

const FEED: &str = "https://example.invalid/appcast.xml";
const KEY: &str = "cHVibGljLWtleS1wdWJsaWMta2V5LXB1YmxpYy1rZXk=";

#[test]
fn updater_none_when_not_bundled() {
    // libtest runs tests off the main thread and outside any bundle: both
    // are refused before AppKit or Sparkle is touched.
    assert!(Updater::start().is_none());
    assert_eq!(
        Updater::try_start(StartMode::Start).err(),
        Some(Unavailable::NotBundled)
    );
    assert_eq!(
        Updater::try_start(StartMode::Idle).err(),
        Some(Unavailable::NotBundled)
    );
    assert!(BundleInfo::main().is_none());
    assert_eq!(sparkle::preflight(None), Err(Unavailable::NotBundled));
}

#[test]
fn preflight_needs_the_feed_url_and_public_key() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("Polygloss.app");
    embed_framework(&app);
    assert_eq!(
        sparkle::preflight(Some(&info(&app, None, Some(KEY)))),
        Err(Unavailable::NoFeedUrl)
    );
    assert_eq!(
        sparkle::preflight(Some(&info(&app, Some(FEED), None))),
        Err(Unavailable::NoPublicKey)
    );
    // Blank values count as missing (a release built without the variables).
    assert_eq!(
        sparkle::preflight(Some(&info(&app, Some("  "), Some(KEY)))),
        Err(Unavailable::NoFeedUrl)
    );
    assert_eq!(
        sparkle::preflight(Some(&info(&app, Some(FEED), Some("")))),
        Err(Unavailable::NoPublicKey)
    );
}

#[test]
fn preflight_needs_the_embedded_framework() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("Polygloss.app");
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    let want = app.join("Contents/Frameworks/Sparkle.framework");
    assert_eq!(
        sparkle::preflight(Some(&info(&app, Some(FEED), Some(KEY)))),
        Err(Unavailable::FrameworkMissing(want.clone()))
    );
    embed_framework(&app);
    assert_eq!(
        sparkle::preflight(Some(&info(&app, Some(FEED), Some(KEY)))),
        Ok(want)
    );
}

#[test]
fn framework_path_is_inside_contents_frameworks() {
    assert_eq!(
        sparkle::framework_path(Path::new("/x/Polygloss.app")),
        Path::new("/x/Polygloss.app/Contents/Frameworks/Sparkle.framework")
    );
}

#[test]
fn sparkle_names_match_the_framework_and_packaging() {
    // package-release.sh writes these keys; Sparkle reads them.
    assert_eq!(FEED_URL_KEY, "SUFeedURL");
    assert_eq!(PUBLIC_ED_KEY_KEY, "SUPublicEDKey");
    assert_eq!(CONTROLLER_CLASS, c"SPUStandardUpdaterController");
}

#[test]
fn unavailable_reasons_read_as_log_lines() {
    let missing = Unavailable::FrameworkMissing("/x/Sparkle.framework".into());
    assert_eq!(
        missing.to_string(),
        "Sparkle.framework is not embedded (/x/Sparkle.framework)"
    );
    assert_eq!(
        Unavailable::NotBundled.to_string(),
        "not running from an app bundle"
    );
    assert_eq!(
        Unavailable::NoFeedUrl.to_string(),
        "the bundle has no SUFeedURL"
    );
    assert_eq!(
        Unavailable::NoPublicKey.to_string(),
        "the bundle has no SUPublicEDKey"
    );
    assert_eq!(
        Unavailable::LoadFailed("boom".into()).to_string(),
        "Sparkle.framework did not load: boom"
    );
}
