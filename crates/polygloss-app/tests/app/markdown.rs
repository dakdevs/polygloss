//! GPUI tests of T3.9: comment markdown (design §8.5, §8.7, §19 "Agent
//! markdown"): raw HTML renders as text, images become links and never
//! load, only `http`, `https` and `mailto` links open, fenced code is
//! highlighted with lumis, and ` ```suggestion ` blocks render as a mini-diff
//! of the anchored new-side lines.

use std::sync::Arc;

use gpui_kit::base::{TextView, TextViewState};
use gpui_kit::{
    AnyWindowHandle, Context, Entity, ImageSource, IntoElement, ParentElement as _, Render,
    SharedUri, Styled as _, VisualContext as _, VisualTestContext, Window, div,
};
use polygloss_app::markdown::suggestion::SuggestionContext;
use polygloss_app::markdown::{self, code_blocks, sanitize, suggestion};
use polygloss_app::space::TextStyleExt as _;
use polygloss_viewport::ViewportTheme;

use crate::shell::{Shell, draw, start};
use crate::support::Sandbox;

/// A window showing one comment body the way thread blocks do.
struct Probe {
    state: Entity<TextViewState>,
    suggestion: Option<SuggestionContext>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = markdown::configure(TextView::new(&self.state), cx);
        let view = suggestion::with_suggestions(view, self.suggestion.clone());
        div()
            .w(gpui_kit::px(600.))
            .text_style(polygloss_app::space::text::BODY)
            .child(view)
    }
}

/// Renders `body` (sanitized) in its own window and returns its state
/// once parsed and drawn, and the window.
fn probe(
    shell: &mut Shell,
    body: &str,
    suggestion: Option<SuggestionContext>,
) -> (Entity<TextViewState>, AnyWindowHandle) {
    let body = body.to_owned();
    let (view, cx) = shell.cx.add_window_view(move |_, cx| Probe {
        state: markdown::markdown_state(&body, cx),
        suggestion,
    });
    draw(cx);
    let handle = cx.window_handle();
    (view.read_with(cx, |p, _| p.state.clone()), handle)
}

/// Whether the last frame of `window` painted an element with `selector`.
fn painted(shell: &mut Shell, window: AnyWindowHandle, selector: &'static str) -> bool {
    VisualTestContext::from_window(window, shell.cx)
        .debug_bounds(selector)
        .is_some()
}

fn rendered(shell: &mut Shell, state: &Entity<TextViewState>) -> String {
    state.read_with(shell.cx, |s, _| s.rendered_text().as_str().to_owned())
}

/// Every node of `md` the way gpui-kit's text view parses it, depth
/// first: GFM with math, then every inline math span re-parsed as prose
/// without math (its `flatten_unclaimed_math`, since no plugin claims
/// math).
fn nodes(md: &str) -> Vec<::markdown::mdast::Node> {
    use ::markdown::ParseOptions;
    use ::markdown::mdast::Node;
    fn options(math: bool) -> ParseOptions {
        let mut o = ParseOptions::gfm();
        o.constructs.math_text = math;
        o.constructs.math_flow = math;
        o
    }
    fn walk(n: &Node, src: &str, out: &mut Vec<Node>) {
        out.push(n.clone());
        if let Node::InlineMath(_) = n {
            let p = n.position().expect("position");
            let literal = &src[p.start.offset..p.end.offset];
            let prose = ::markdown::to_mdast(literal, &options(false)).expect("parse");
            walk(&prose, literal, out);
        }
        for c in n.children().into_iter().flatten() {
            walk(c, src, out);
        }
    }
    let root = ::markdown::to_mdast(md, &options(true)).expect("parse");
    let mut out = Vec::new();
    walk(&root, md, &mut out);
    out
}

/// Whether `md`, as gpui-kit parses it, holds an inline formatting tag
/// (which gpui-kit would pair into formatting).
fn has_paired_tag(md: &str) -> bool {
    nodes(md).iter().any(|n| match n {
        ::markdown::mdast::Node::Html(h) => {
            let v = h.value.to_ascii_lowercase();
            ["b", "strong", "em", "i", "u", "s", "del", "strike"]
                .iter()
                .any(|t| v.starts_with(&format!("<{t}>")) || v.starts_with(&format!("</{t}>")))
        }
        _ => false,
    })
}

#[gpui_kit::test]
fn sanitizer_renders_raw_html_as_text(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let body = "Plain <b>bold</b>, <span class=\"x\">span</span> and \
                <img src=\"https://example.com/t.png\"> inline.\n\n\
                <div align=\"center\">\n<script>alert(1)</script>\n</div>\n\n\
                <!-- a comment -->\n\n`<b>code</b>` stays code.\n";
    let (state, _) = probe(&mut shell, body, None);
    let text = rendered(&mut shell, &state);
    for literal in [
        "<b>bold</b>",
        "<span class=\"x\">span</span>",
        "<img src=\"https://example.com/t.png\">",
        "<div align=\"center\">",
        "<script>alert(1)</script>",
        "<!-- a comment -->",
        "<b>code</b> stays code.",
    ] {
        assert!(text.contains(literal), "{literal:?} not in {text:?}");
    }
    // The formatting tags gpui-kit would pair before any plugin sees them
    // are escaped up front; code spans are left alone.
    assert_eq!(
        sanitize::prepare("a <b>x</b> `<b>` <em>y</em>"),
        "a \\<b>x\\</b> `<b>` \\<em>y\\</em>"
    );
    assert_eq!(sanitize::prepare("no html here"), "no html here");

    // Text between two dollar signs parses as inline math, which gpui-kit
    // re-parses as prose when no plugin claims it: the tags inside are
    // escaped too (review fix).
    for body in [
        "costs $5 and <b>x</b> for $10",
        "$1 <em>y</em> and <s>z</s> $2 and <strong>w</strong>",
        "a [link $ <b>x</b> $](https://example.com)",
        "- item $5 <i>x</i> $6\n\n> quote $1 <u>x</u> $2",
    ] {
        assert!(has_paired_tag(body), "the case is live: {body:?}");
        let prepared = sanitize::prepare(body);
        assert!(!has_paired_tag(&prepared), "{prepared:?} still pairs tags");
    }
    let body = "costs $5 and <b>x</b> for $10";
    let (state, _) = probe(&mut shell, body, None);
    let text = rendered(&mut shell, &state);
    assert!(text.contains("costs $5 and <b>x</b> for $10"), "{text:?}");
    assert!(!text.contains('\\'), "no escape shows: {text:?}");
    // Math holding tags becomes prose (its `$`s escaped, so an escape
    // never shows literally inside it); code spans in it stay code, and
    // plain math is left alone.
    assert_eq!(
        sanitize::prepare("$1 `<b>` <b>x</b> $2"),
        "\\$1 `<b>` \\<b>x\\</b> \\$2"
    );
    assert_eq!(
        sanitize::prepare("<b>a</b> costs $5 and $10"),
        "\\<b>a\\</b> costs $5 and $10"
    );

    // A GFM autolink literal (`https://…`, `www.…`) runs until whitespace
    // or `<`, so it would absorb the backslash escaping a tag right after
    // it and leave the tag live; it becomes an explicit link to the same
    // URL instead, and `prepare` settles in a pass or two (review fix,
    // round 2).
    use ::markdown::mdast::Node;
    let urls = |md: &str| -> Vec<String> {
        nodes(md)
            .iter()
            .filter_map(|n| match n {
                Node::Link(l) => Some(l.url.clone()),
                _ => None,
            })
            .collect()
    };
    for (body, want) in [
        (
            "Docs: <b>https://example.com</b>",
            &["https://example.com"][..],
        ),
        ("<b>http://example.com</b>", &["http://example.com"]),
        // `www.` right after `>` is not a literal (it needs a space or
        // punctuation before it): nothing to absorb the escape.
        ("<em>www.example.com</em>", &[]),
        (
            "see http://a.com<b>bold http://b.com</b>",
            &["http://a.com", "http://b.com"],
        ),
        (
            "www.a.com<em>x www.b.com</em>",
            &["http://www.a.com", "http://www.b.com"],
        ),
        (
            "https://a.com/x?y=1</s> and <strong>https://b.com/p_q</strong>",
            &["https://a.com/x?y=1", "https://b.com/p_q"],
        ),
        // A literal running into inline math's closing `$` holds it (as in
        // gpui-kit's prose re-parse of the math).
        (
            "costs $5 <b>x</b> see https://a.com$ ok",
            &["https://a.com$"],
        ),
    ] {
        assert!(has_paired_tag(body), "the case is live: {body:?}");
        let prepared = sanitize::prepare(body);
        assert!(!has_paired_tag(&prepared), "{prepared:?} still pairs tags");
        let passes = sanitize::prepare_passes(body);
        assert!(
            matches!(passes, Some(1..=2)),
            "{body:?} settles in {passes:?} passes"
        );
        assert_eq!(urls(&prepared), want, "{prepared:?}");
        assert_eq!(sanitize::prepare(&prepared), prepared, "idempotent");
    }
    let body = "Docs: <b>https://example.com</b>";
    let (state, _) = probe(&mut shell, body, None);
    let text = rendered(&mut shell, &state);
    assert!(text.contains(body), "{text:?}");
    assert!(!text.contains('\\'), "no escape shows: {text:?}");

    // The last resort, should `prepare` not settle, leaves no markup: an
    // escaped backslash never precedes a live tag.
    let escaped = sanitize::escape_all("\\<b>x</b> $1$ ![a](u) `c` https://a.com<i>y</i>");
    assert!(!has_paired_tag(&escaped), "{escaped:?}");
    assert!(
        !nodes(&escaped).iter().any(|n| matches!(
            n,
            Node::Html(_)
                | Node::InlineMath(_)
                | Node::Image(_)
                | Node::InlineCode(_)
                | Node::Link(_)
        )),
        "{escaped:?}"
    );

    // markdown-rs panics on some list and math-block mixes (gpui-kit would
    // too, on the main thread): such a body shows as one plain code block
    // (review fix, round 2).
    for body in [
        "1. $$\n- x",
        "- $$\n1. x",
        "Look:\n\n1. $$ <b>x</b>\n- ![i](u)",
    ] {
        assert!(
            std::panic::catch_unwind(|| nodes(body)).is_err(),
            "the case is live: {body:?}"
        );
        let prepared = sanitize::prepare(body);
        let parsed = nodes(&prepared);
        assert!(
            matches!(&parsed[1], Node::Code(c) if c.value == body && c.lang.is_none()),
            "{prepared:?}"
        );
        let (state, _) = probe(&mut shell, body, None);
        let text = rendered(&mut shell, &state);
        for line in body.lines() {
            assert!(text.contains(line), "{line:?} not in {text:?}");
        }
    }
}

#[gpui_kit::test]
fn images_become_links_and_never_load(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let body = "![logo](https://example.com/logo.png \"Logo\") and ![](https://example.com/x.png) \
                and ![ref img][r] and [![badge](https://example.com/b.svg)](https://example.com)\n\n\
                [r]: https://example.com/r.png\n";
    let prepared = sanitize::prepare(body);
    let parsed = nodes(&prepared);
    use ::markdown::mdast::Node;
    assert!(
        !parsed
            .iter()
            .any(|n| matches!(n, Node::Image(_) | Node::ImageReference(_))),
        "{prepared:?} still has images"
    );
    let links: Vec<String> = parsed
        .iter()
        .filter_map(|n| match n {
            Node::Link(l) => Some(l.url.clone()),
            _ => None,
        })
        .collect();
    // An image inside a link becomes that link's text.
    for url in [
        "https://example.com/logo.png",
        "https://example.com/x.png",
        "https://example.com",
    ] {
        assert!(
            links.iter().any(|l| l == url),
            "{url} not linked: {links:?}"
        );
    }
    assert!(
        parsed.iter().any(|n| matches!(n, Node::LinkReference(_))),
        "the image reference is a link reference"
    );

    // Images inside inline math (gpui-kit re-parses it as prose) become
    // links too (review fix).
    for math in [
        "$1 ![logo](https://example.com/t.png) $2",
        "costs $5 [![b](https://example.com/b.svg)](https://example.com) $6",
    ] {
        let has_image = |md: &str| {
            nodes(md)
                .iter()
                .any(|n| matches!(n, Node::Image(_) | Node::ImageReference(_)))
        };
        assert!(has_image(math), "the case is live: {math:?}");
        let prepared = sanitize::prepare(math);
        assert!(!has_image(&prepared), "{prepared:?} still has images");
        assert!(
            nodes(&prepared).iter().any(|n| matches!(n, Node::Link(_))),
            "{prepared:?} has no link"
        );
    }

    let (state, _) = probe(&mut shell, body, None);
    let text = rendered(&mut shell, &state);
    for label in ["logo", "https://example.com/x.png", "ref img", "badge"] {
        assert!(text.contains(label), "{label:?} not in {text:?}");
    }
    // Belt and braces: whatever asks for an image gets the bundled
    // placeholder, never a load from its URL.
    for uri in [
        "https://example.com/logo.png",
        "http://example.com/a.gif",
        "data:image/png;base64,iVBORw0KGgo=",
        "file:///etc/hosts",
    ] {
        match sanitize::image_source(&SharedUri::from(uri.to_owned())) {
            ImageSource::Image(img) => {
                assert!(Arc::ptr_eq(&img, &sanitize::placeholder_image()), "{uri}")
            }
            _ => panic!("{uri} would load"),
        }
    }
}

#[gpui_kit::test]
fn link_click_allows_only_http_https_mailto(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    for (url, allowed) in [
        ("https://example.com/a?b=c", true),
        ("http://example.com", true),
        ("HTTPS://EXAMPLE.COM", true),
        ("mailto:someone@example.com", true),
        ("file:///etc/passwd", false),
        ("javascript:alert(1)", false),
        ("polygloss://diff/abc", false),
        ("ftp://example.com", false),
        ("vscode://file/x", false),
        ("relative/path.md", false),
        ("#anchor", false),
        ("", false),
    ] {
        assert_eq!(sanitize::link_allowed(url), allowed, "{url:?}");
    }
    let cx = shell.cx;
    cx.update(|_, cx| markdown::open_link("file:///etc/passwd", cx));
    assert_eq!(cx.opened_url(), None);
    cx.update(|_, cx| markdown::open_link("javascript:alert(1)", cx));
    assert_eq!(cx.opened_url(), None);
    cx.update(|_, cx| markdown::open_link("https://example.com/docs", cx));
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/docs"));
    cx.update(|_, cx| markdown::open_link("mailto:someone@example.com", cx));
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("mailto:someone@example.com")
    );
}

#[gpui_kit::test]
fn code_blocks_are_highlighted_with_lumis(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let _shell = start(cx);
    let theme = Arc::new(ViewportTheme::pierre(
        polygloss_highlight::Appearance::Light,
    ));
    let code = "fn main() {\n    let x = 1;\n}\n";
    let spans = code_blocks::highlight(code, Some("rust"), &theme);
    assert!(!spans.is_empty(), "rust is highlighted");
    for (range, style) in &spans {
        assert!(range.end <= code.len() && range.start < range.end);
        assert!(code.is_char_boundary(range.start) && code.is_char_boundary(range.end));
        assert!(style.color.is_some());
    }
    let keyword = spans
        .iter()
        .find(|(r, _)| &code[r.clone()] == "fn")
        .expect("`fn` is styled");
    assert!(keyword.1.color.is_some());
    // Unknown or missing languages stay plain.
    assert!(code_blocks::highlight(code, Some("no-such-lang"), &theme).is_empty());
    assert!(code_blocks::highlight(code, None, &theme).is_empty());
    assert!(code_blocks::highlight(code, Some("suggestion"), &theme).is_empty());
}

#[gpui_kit::test]
fn suggestion_renders_mini_diff_on_new_side(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    // Lines 12–13 of the new side, from a snippet of lines 9–16.
    let snippet = "l9\nl10\nl11\nlet a = 1;\nlet b = 2;\nl14\nl15\nl16";
    assert_eq!(
        suggestion::anchored_lines(snippet, 12, 13),
        Some(vec!["let a = 1;".to_owned(), "let b = 2;".to_owned()])
    );
    // Near the top of the file the snippet has fewer lines above.
    assert_eq!(
        suggestion::anchored_lines("a\nb\nc\nd", 1, 1),
        Some(vec!["a".to_owned()])
    );
    assert_eq!(suggestion::anchored_lines("a", 3, 4), None);
    let ctx = SuggestionContext {
        start_line: 12,
        lines: vec!["let a = 1;".to_owned(), "let b = 2;".to_owned()].into(),
    };
    let rows = suggestion::mini_diff(&ctx, "let a = 10;\nlet b = 2;\nlet c = 3;");
    use suggestion::MiniRow;
    assert_eq!(
        rows,
        vec![
            MiniRow::Removed {
                line: 12,
                text: "let a = 1;".into()
            },
            MiniRow::Removed {
                line: 13,
                text: "let b = 2;".into()
            },
            MiniRow::Added {
                line: 12,
                text: "let a = 10;".into()
            },
            MiniRow::Added {
                line: 13,
                text: "let b = 2;".into()
            },
            MiniRow::Added {
                line: 14,
                text: "let c = 3;".into()
            },
        ]
    );
    // An empty suggestion deletes the lines.
    assert_eq!(
        suggestion::mini_diff(&ctx, "")
            .iter()
            .filter(|r| matches!(r, MiniRow::Added { .. }))
            .count(),
        0
    );

    let body = "Use a larger value:\n\n```suggestion\nlet a = 10;\nlet b = 2;\n```\n";
    let (_, window) = probe(&mut shell, body, Some(ctx));
    for selector in [
        "suggestion-diff",
        "suggestion-removed-0",
        "suggestion-removed-1",
        "suggestion-added-0",
        "suggestion-added-1",
    ] {
        assert!(painted(&mut shell, window, selector), "{selector}");
    }
    assert!(!painted(&mut shell, window, "suggestion-added-2"));

    // Only top-level blocks are suggestions, as for MCP's
    // `parse_suggestions`: one quoted or in a list item is plain code
    // (review fix).
    for nested in [
        "> ```suggestion\n> let a = 10;\n> ```\n",
        "- item\n\n  ```suggestion\n  let a = 10;\n  ```\n",
    ] {
        assert!(
            polygloss_core::review::parse_suggestions(nested).is_empty(),
            "{nested:?}"
        );
        assert!(suggestion::top_level_suggestions(nested).is_empty());
        let (state, window) = probe(&mut shell, nested, Some(ctx_for_nested()));
        assert!(
            !painted(&mut shell, window, "suggestion-diff"),
            "{nested:?}"
        );
        let text = rendered(&mut shell, &state);
        assert!(text.contains("let a = 10;"), "{text:?}");
    }
    let top = "Intro\n\n```suggestion\nx\n```\n";
    assert_eq!(suggestion::top_level_suggestions(top), [7]);
}

fn ctx_for_nested() -> SuggestionContext {
    SuggestionContext {
        start_line: 1,
        lines: vec!["let a = 1;".to_owned()].into(),
    }
}

#[gpui_kit::test]
fn suggestion_on_old_side_renders_plain_code(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let body = "Try:\n\n```suggestion\nlet a = 10;\n```\n";
    let (state, window) = probe(&mut shell, body, None);
    assert!(!painted(&mut shell, window, "suggestion-diff"));
    let text = rendered(&mut shell, &state);
    assert!(text.contains("let a = 10;"), "{text:?}");
    // Old-side and file anchors have no suggestion context.
    use polygloss_core::review::{Subject, ThreadAnchor};
    use polygloss_diff::Side;
    let anchor = |side| ThreadAnchor {
        subject: Subject::Line {
            path: "a.rs".into(),
            side,
            start_line: 2,
            line: 2,
        },
        anchor_blob: None,
        anchor_snippet: Some("a\nb\nc".into()),
    };
    assert!(suggestion::context_for(&anchor(Side::Old), None).is_none());
    let new = suggestion::context_for(&anchor(Side::New), None).expect("new side has a context");
    assert_eq!(new.start_line, 2);
    assert_eq!(&*new.lines, ["b".to_owned()]);
    // A moved thread's mini-diff is numbered where the lines are now; an
    // outdated one's where they were written (review fix).
    use polygloss_core::review::{Position, PositionState};
    let at = |state, line| Position {
        state,
        path: Some("a.rs".into()),
        side: Some(Side::New),
        start_line: Some(line),
        line: Some(line),
    };
    let moved = suggestion::context_for(&anchor(Side::New), Some(&at(PositionState::Moved, 7)))
        .expect("context");
    assert_eq!(moved.start_line, 7);
    assert_eq!(&*moved.lines, ["b".to_owned()]);
    let outdated =
        suggestion::context_for(&anchor(Side::New), Some(&at(PositionState::Outdated, 9)))
            .expect("context");
    assert_eq!(outdated.start_line, 2);
    let file = ThreadAnchor {
        subject: Subject::File {
            path: "a.rs".into(),
        },
        anchor_blob: None,
        anchor_snippet: None,
    };
    assert!(suggestion::context_for(&file, None).is_none());
}

/// The painted bounds of `selector` in `window`.
fn bounds_in(
    shell: &mut Shell,
    window: AnyWindowHandle,
    selector: &'static str,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    VisualTestContext::from_window(window, shell.cx)
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} was not painted"))
}

/// T7.7, ADR-0031 C4: a suggestion uses the card's column formula from its
/// frame's inner edge, as a card in the `+-` indicator mode (its `-` and
/// `+` in a two-advance cell after the gutter). Hand-computed at 13 pt
/// (advance 7.8): 2-digit numbers, 4 + 4 + 15.6 + 8 = 31.6, at least 40,
/// code at 40 + 15.6 = 55.6; 4-digit numbers, 4 + 4 + 31.2 + 8 = 47.2 → 48,
/// code at 63.6, laid out on the window's device pixels (within a quarter
/// point at 2×). Rows are code rows: round(1.5 × 13) = 20.
#[gpui_kit::test]
fn suggestion_uses_the_card_column_formula(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    for (start_line, code_x) in [(12, 55.6), (1_012, 63.6)] {
        let ctx = SuggestionContext {
            start_line,
            lines: vec!["let a = 1;".to_owned()].into(),
        };
        let body = "```suggestion\nlet a = 10;\n```\n";
        let (_, window) = probe(&mut shell, body, Some(ctx));
        let frame = bounds_in(&mut shell, window, "suggestion-diff");
        let code = bounds_in(&mut shell, window, "suggestion-code-removed-0");
        let x = (code.left() - frame.left()).as_f32() - 1.0;
        assert!((x - code_x).abs() <= 0.25, "line {start_line}: code at {x}");
        assert_eq!(code.size.height, gpui_kit::px(20.), "line {start_line}");
    }
}

/// T7.7: a blank line of raw HTML keeps a comment line's height
/// (`text::BODY`, 18 pt), not a rem-based one.
#[gpui_kit::test]
fn a_blank_html_line_keeps_the_body_line_height(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let (_, window) = probe(&mut shell, "<pre>\na\n\nb\n</pre>\n", None);
    let blank = bounds_in(&mut shell, window, "html-blank-line");
    assert_eq!(blank.size.height, gpui_kit::px(18.));
}
