//! gpui-kit's families as `polygloss_viewport::kit::init_kit` names them
//! (plan T2.10.2), checked against the real macOS text system (CoreText).
//!
//! The viewport's kit tests run on GPUI's no-op text system, where every
//! family counts as installed; these pin that the system families `init_kit`
//! names instead of gpui-kit's probes really resolve, so gpui-kit's widgets
//! never fall back to GPUI's fallback stack.

use gpui_kit::component::theme::Theme;
use polygloss_viewport::kit::{SYSTEM_MONO_FONT, SYSTEM_UI_FONT, init_kit, is_installed};

use crate::support::Sandbox;
use crate::support::harness::Test;
use crate::support::screenshot;

pub const TESTS: &[Test] = &crate::tests![
    e2e_kit_system_families_resolve_on_the_real_text_system,
    e2e_init_kit_names_resolvable_families_for_a_menlo_code_font,
];

fn e2e_kit_system_families_resolve_on_the_real_text_system() {
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app();
    cx.update(|cx| {
        let text_system = cx.text_system();
        assert!(
            is_installed(text_system, SYSTEM_UI_FONT),
            "{SYSTEM_UI_FONT}"
        );
        assert!(
            is_installed(text_system, SYSTEM_MONO_FONT),
            "{SYSTEM_MONO_FONT}"
        );
        // The control: a family nobody has resolves to the fallback stack.
        assert!(!is_installed(text_system, "Polygloss No Such Family"));
    });
}

fn e2e_init_kit_names_resolvable_families_for_a_menlo_code_font() {
    // A user whose code font is Menlo gets SF Mono in gpui-kit's monospace
    // widgets (naming Menlo would bring gpui-kit's font scan back); the
    // viewport itself still draws Menlo.
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app();
    cx.update(|cx| {
        assert!(is_installed(cx.text_system(), "Menlo"));
        init_kit("Menlo", cx);
        let theme = Theme::global(cx);
        assert_eq!(theme.font_family.as_ref(), SYSTEM_UI_FONT);
        assert_eq!(theme.mono_font_family.as_ref(), SYSTEM_MONO_FONT);
        let (ui, mono) = (theme.font_family.clone(), theme.mono_font_family.clone());
        assert!(is_installed(cx.text_system(), &ui), "{ui}");
        assert!(is_installed(cx.text_system(), &mono), "{mono}");
    });
}
