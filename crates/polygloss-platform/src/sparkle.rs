//! The Sparkle 2 updater, loaded at runtime through our own objc2 FFI
//! (design §21, library-choices §14, OQ-16).
//!
//! Nothing links against Sparkle: [`Updater::start`] loads
//! `Contents/Frameworks/Sparkle.framework` with `NSBundle` and creates an
//! `SPUStandardUpdaterController` by class name, on the main thread. It
//! returns `None` (and [`Updater::try_start`] says why) when the process is
//! not an app bundle, the bundle's `Info.plist` lacks `SUFeedURL` or
//! `SUPublicEDKey` (`scripts/package-release.sh` writes them only when
//! `POLYGLOSS_APPCAST_URL` and `SPARKLE_PUBLIC_ED_KEY` are set, plan OQ-P10),
//! the framework is not embedded, it fails to load, or the caller is off the
//! main thread. Dev builds, tests and releases built without an appcast
//! therefore never start an updater.
//!
//! Sparkle checks are Polygloss's only network access, and they are opt-in:
//! the bundle leaves `SUEnableAutomaticChecks` unset, so Sparkle asks the
//! user on the second launch before it checks automatically (its standard
//! prompt), and [`Updater::set_automatically_checks`] applies the
//! `updates.automatic_checks` setting on top.
//!
//! [`StartMode::Idle`] creates the controller without starting the updater:
//! no update check, no permission prompt and no `NSUserDefaults` writes. The
//! app uses it in test mode (`POLYGLOSS_TEST=1`), so the bundle smoke test
//! proves the framework loads and the FFI holds without any egress.
#![allow(unsafe_code)]

use std::ffi::CStr;
use std::fmt;
use std::path::{Path, PathBuf};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{MainThreadMarker, msg_send};
use objc2_foundation::{NSBundle, NSString};

/// The `Info.plist` key of the appcast URL.
pub const FEED_URL_KEY: &str = "SUFeedURL";

/// The `Info.plist` key of the EdDSA public key that signs updates.
pub const PUBLIC_ED_KEY_KEY: &str = "SUPublicEDKey";

/// Sparkle's standard controller: the updater plus its standard UI.
pub const CONTROLLER_CLASS: &CStr = c"SPUStandardUpdaterController";

/// Whether [`Updater::try_start`] starts the updater.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// Start it: scheduled checks (once the user allowed them) and the
    /// "Check for Updates…" command work.
    Start,
    /// Create the controller without starting the updater: nothing is
    /// checked, prompted or written (test mode).
    Idle,
}

/// Why there is no updater.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// Not an app bundle (a `cargo run` or test binary).
    NotBundled,
    /// The bundle has no (or a blank) `SUFeedURL`.
    NoFeedUrl,
    /// The bundle has no (or a blank) `SUPublicEDKey`.
    NoPublicKey,
    /// `Contents/Frameworks/Sparkle.framework` is not there.
    FrameworkMissing(PathBuf),
    /// Called off the main thread.
    NotMainThread,
    /// `NSBundle` could not load the framework (e.g. library validation).
    LoadFailed(String),
    /// The framework has no `SPUStandardUpdaterController` class.
    ClassMissing,
    /// The controller's initializer returned nil.
    InitFailed,
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unavailable::NotBundled => f.write_str("not running from an app bundle"),
            Unavailable::NoFeedUrl => write!(f, "the bundle has no {FEED_URL_KEY}"),
            Unavailable::NoPublicKey => write!(f, "the bundle has no {PUBLIC_ED_KEY_KEY}"),
            Unavailable::FrameworkMissing(path) => {
                write!(f, "Sparkle.framework is not embedded ({})", path.display())
            }
            Unavailable::NotMainThread => f.write_str("not on the main thread"),
            Unavailable::LoadFailed(why) => write!(f, "Sparkle.framework did not load: {why}"),
            Unavailable::ClassMissing => write!(
                f,
                "Sparkle.framework has no {} class",
                CONTROLLER_CLASS.to_string_lossy()
            ),
            Unavailable::InitFailed => write!(
                f,
                "{} failed to initialize",
                CONTROLLER_CLASS.to_string_lossy()
            ),
        }
    }
}

impl std::error::Error for Unavailable {}

/// What the updater needs from the main bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleInfo {
    /// The `.app` directory.
    pub path: PathBuf,
    /// `SUFeedURL`.
    pub feed_url: Option<String>,
    /// `SUPublicEDKey`.
    pub public_ed_key: Option<String>,
}

impl BundleInfo {
    /// The running app bundle's, or `None` outside one (no bundle
    /// identifier, as in [`crate::bundle::is_bundled`]).
    pub fn main() -> Option<BundleInfo> {
        let bundle = NSBundle::mainBundle();
        bundle.bundleIdentifier()?;
        let string = |key: &str| -> Option<String> {
            let value = bundle.objectForInfoDictionaryKey(&NSString::from_str(key))?;
            let value = value.downcast::<NSString>().ok()?;
            Some(value.to_string())
        };
        Some(BundleInfo {
            path: PathBuf::from(bundle.bundlePath().to_string()),
            feed_url: string(FEED_URL_KEY),
            public_ed_key: string(PUBLIC_ED_KEY_KEY),
        })
    }
}

/// `<app>/Contents/Frameworks/Sparkle.framework`.
pub fn framework_path(app: &Path) -> PathBuf {
    app.join("Contents")
        .join("Frameworks")
        .join("Sparkle.framework")
}

/// The checks before anything is loaded: an app bundle with a feed URL, a
/// public key and the embedded framework. Returns the framework's path.
pub fn preflight(bundle: Option<&BundleInfo>) -> Result<PathBuf, Unavailable> {
    let bundle = bundle.ok_or(Unavailable::NotBundled)?;
    let present = |v: &Option<String>| v.as_deref().is_some_and(|v| !v.trim().is_empty());
    if !present(&bundle.feed_url) {
        return Err(Unavailable::NoFeedUrl);
    }
    if !present(&bundle.public_ed_key) {
        return Err(Unavailable::NoPublicKey);
    }
    let framework = framework_path(&bundle.path);
    if !framework.is_dir() {
        return Err(Unavailable::FrameworkMissing(framework));
    }
    Ok(framework)
}

/// A running (or, in [`StartMode::Idle`], loaded) Sparkle updater. Keep it
/// for the app's lifetime. Main thread only (it is neither `Send` nor
/// `Sync`).
pub struct Updater {
    /// The `SPUStandardUpdaterController`.
    controller: Retained<AnyObject>,
    mode: StartMode,
}

impl fmt::Debug for Updater {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Updater")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl Updater {
    /// Starts Sparkle, or `None` when it is unavailable (module docs).
    pub fn start() -> Option<Updater> {
        Updater::try_start(StartMode::Start).ok()
    }

    /// [`Updater::start`], saying why there is no updater, and able to load
    /// without starting ([`StartMode::Idle`]).
    pub fn try_start(mode: StartMode) -> Result<Updater, Unavailable> {
        // The bundle checks first: they touch no AppKit state and hold in
        // any thread (a test binary is never bundled).
        let framework = preflight(BundleInfo::main().as_ref())?;
        MainThreadMarker::new().ok_or(Unavailable::NotMainThread)?;
        load_framework(&framework)?;
        let class = AnyClass::get(CONTROLLER_CLASS).ok_or(Unavailable::ClassMissing)?;
        let nil: Option<&AnyObject> = None;
        let starting = mode == StartMode::Start;
        // SAFETY: `SPUStandardUpdaterController` declares
        // `-initWithStartingUpdater:(BOOL) updaterDelegate:(id) userDriverDelegate:(id)`,
        // both delegates nullable; we are on the main thread (checked above),
        // as Sparkle requires.
        let controller: Option<Retained<AnyObject>> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![
                alloc,
                initWithStartingUpdater: starting,
                updaterDelegate: nil,
                userDriverDelegate: nil
            ]
        };
        let controller = controller.ok_or(Unavailable::InitFailed)?;
        Ok(Updater { controller, mode })
    }

    /// How the updater was started.
    pub fn mode(&self) -> StartMode {
        self.mode
    }

    /// "Check for Updates…": Sparkle's user-initiated check, with its own
    /// window. Does nothing in [`StartMode::Idle`].
    pub fn check_for_updates(&self) {
        if self.mode == StartMode::Idle {
            return;
        }
        let sender: Option<&AnyObject> = None;
        // SAFETY: `-checkForUpdates:(id)sender` is the controller's
        // IBAction; the sender may be nil. Main thread: `Updater` is !Send.
        unsafe {
            let _: () = msg_send![&*self.controller, checkForUpdates: sender];
        }
    }

    /// Whether Sparkle checks automatically (the user's answer to its
    /// prompt, or [`Updater::set_automatically_checks`]).
    pub fn automatically_checks(&self) -> bool {
        // SAFETY: `-updater` returns the controller's `SPUUpdater` (never
        // nil once initialized); `automaticallyChecksForUpdates` is a BOOL
        // property.
        unsafe {
            let updater: Retained<AnyObject> = msg_send![&*self.controller, updater];
            msg_send![&*updater, automaticallyChecksForUpdates]
        }
    }

    /// Turns automatic checks on or off (Sparkle keeps it in the app's
    /// user defaults). Does nothing in [`StartMode::Idle`], which writes no
    /// defaults.
    pub fn set_automatically_checks(&self, on: bool) {
        if self.mode == StartMode::Idle {
            return;
        }
        // SAFETY: as in `automatically_checks`; the setter takes a BOOL.
        unsafe {
            let updater: Retained<AnyObject> = msg_send![&*self.controller, updater];
            let _: () = msg_send![&*updater, setAutomaticallyChecksForUpdates: on];
        }
    }
}

/// Loads the framework's code, with the loader's reason on failure.
fn load_framework(framework: &Path) -> Result<(), Unavailable> {
    let path = NSString::from_str(&framework.to_string_lossy());
    let bundle = NSBundle::bundleWithPath(&path).ok_or_else(|| {
        Unavailable::LoadFailed(format!("{} is not a bundle", framework.display()))
    })?;
    // SAFETY: `-loadAndReturnError:` loads the bundle's executable; its
    // `NSError **` out-parameter is handled by `msg_send!`'s `_`.
    let loaded: Result<(), Retained<NSObject>> =
        unsafe { msg_send![&*bundle, loadAndReturnError: _] };
    loaded.map_err(|err| {
        // SAFETY: the error is an `NSError`, whose `localizedDescription`
        // is a non-nil `NSString`.
        let why: Retained<NSString> = unsafe { msg_send![&*err, localizedDescription] };
        Unavailable::LoadFailed(why.to_string())
    })
}
