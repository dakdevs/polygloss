//! The app's asset source (plan M6 "Icons"): the Lucide icons the redesign
//! uses that gpui-kit's default bundle lacks, in front of that bundle.
//!
//! An icon registered nowhere draws nothing, silently, so every `IconName`
//! variant the app names and every icon path literal in the app and the
//! viewport must be in [`AppIcons`] or in gpui-kit-assets'
//! `default-icons.txt`; `tests/scripts/app-icons.test.ts` checks that.

use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

gpui_kit::assets::icon_assets!(
    pub AppIcons,
    [
        ListTree,
        RotateCcwClock,
        PanelLeft,
        PanelLeftOpen,
        GitBranch,
        GitCommitHorizontal,
        GitCompare,
        CircleDot,
        MessageSquare,
        MessageSquareDot,
        Columns2,
        Rows2,
        SlidersHorizontal,
        SquareArrowOutUpRight,
        Square,
        SquareCheck,
        Circle,
        CircleCheck,
        CircleMinus,
        Camera,
        FlaskConical,
        FileCog,
        Package,
        BookOpen,
        Wrench,
        Layers,
        Languages,
        Tag,
        FileSymlink,
        Binary,
        HardDrive,
        FolderGit,
        Dot,
    ]
);

/// [`AppIcons`] first, then gpui-kit's default bundle
/// (`gpui_kit::assets::Assets`, the icons its components draw).
#[derive(Clone, Copy, Debug, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        match AppIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(AppIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
