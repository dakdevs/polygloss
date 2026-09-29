//! The comment sanitizer (design §8.7, §19 "Agent markdown";
//! library-choices §5). Every body, human or agent, is untrusted:
//!
//! - **Raw HTML is text.** [`HtmlBlockAsText`] and [`HtmlInlineAsText`] claim
//!   every `mdast::Node::Html` (block and inline) and show its source
//!   literally. gpui-kit pairs a few inline formatting tags (`<b>…</b>`,
//!   `<em>`, `<s>`, …) among a paragraph's children *before* any plugin sees
//!   them, so [`prepare`] backslash-escapes those tags up front. Text
//!   between two dollar signs is inline math, which gpui-kit re-parses as
//!   prose when no plugin claims it (its `flatten_unclaimed_math`), tags and
//!   images included, while an escape inside math would show literally; so
//!   [`prepare`] escapes the dollar signs of any inline math holding such
//!   markup, making it prose, and then escapes what is in it.
//! - **Images are links.** [`prepare`] rewrites every image (`![alt](url)`,
//!   `![alt][ref]`) into a link to the same URL, labelled with its alt text
//!   (the URL when there is none); an image inside a link becomes that link's
//!   text. [`image_source`] answers every image request with a bundled
//!   placeholder all the same, so nothing is ever fetched.
//! - **Links:** [`open_link`] opens only `http:`, `https:` and `mailto:`
//!   URLs (NSWorkspace via `App::open_url`); gpui-kit's default would open
//!   any URL, `file:` and custom schemes included.

use std::ops::Range;
use std::sync::{Arc, LazyLock};

use gpui_kit::base::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    markdown_ast as mdast,
};
use gpui_kit::{
    App, Image, ImageFormat, ImageSource, IntoElement, ParentElement as _, SharedUri, Styled as _,
    Window, div,
};
use markdown::ParseOptions;
use mdast::Node;

/// gpui-kit's markdown parse options (GFM with math, no MDX, no
/// frontmatter), so [`prepare`] sees the nodes the text view will.
fn parse_options() -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    options
}

/// gpui-kit's options for re-parsing unclaimed inline math as prose (its
/// `flatten_unclaimed_math`): the same, without math.
fn prose_options() -> ParseOptions {
    let mut options = parse_options();
    options.constructs.math_text = false;
    options.constructs.math_flow = false;
    options
}

/// Rewrites `body` before gpui-kit parses it: images become links, and the
/// inline formatting tags gpui-kit would pair (`b`, `strong`, `i`, `em`,
/// `u`, `s`, `del`, `strike`) are backslash-escaped, so they show as text.
/// Inline math holding either gets its dollar signs escaped (it renders as
/// the same prose gpui-kit would make of it). Code, and everything else, is
/// left as written.
pub fn prepare(body: &str) -> String {
    if !body.contains('<') && !body.contains('!') {
        return body.to_owned();
    }
    // Each pass rewrites what the parse shows; escaping inline math exposes
    // its content to the next pass. Every pass that changes something
    // escapes at least one tag, image or dollar sign for good, so this
    // ends; the cap is a guard.
    let mut out = body.to_owned();
    for _ in 0..PREPARE_PASSES {
        match prepare_once(&out) {
            Some(next) => out = next,
            None => return out,
        }
    }
    escape_all(&out)
}

const PREPARE_PASSES: usize = 32;

/// The last resort when [`prepare`] does not settle: every `<`, `!` and `$`
/// backslash-escaped (code shows the backslashes; nothing renders).
fn escape_all(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + body.len() / 8);
    for c in body.chars() {
        if matches!(c, '<' | '!' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// One rewrite of `body`, or `None` when there is nothing to rewrite.
fn prepare_once(body: &str) -> Option<String> {
    let root = markdown::to_mdast(body, &parse_options()).ok()?;
    let mut edits = Vec::new();
    collect(&root, body, 0, Context::Block, &mut edits);
    if edits.is_empty() {
        return None;
    }
    // Edits never overlap (images are not descended into); apply them back
    // to front so earlier offsets stay valid.
    edits.sort_by_key(|(r, _): &(Range<usize>, String)| std::cmp::Reverse((r.start, r.end)));
    let mut out = body.to_owned();
    for (range, text) in edits {
        if range.end <= out.len()
            && out.is_char_boundary(range.start)
            && out.is_char_boundary(range.end)
        {
            out.replace_range(range, &text);
        }
    }
    (out != body).then_some(out)
}

/// Where a node sits: among blocks, in a paragraph's inline content, or
/// inside a link's text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Block,
    Inline,
    Link,
}

/// `node`'s byte range in the body, for a node parsed from the body at
/// `base`.
fn span(node: &Node, base: usize) -> Option<Range<usize>> {
    let p = node.position()?;
    Some(base + p.start.offset..base + p.end.offset)
}

/// Collects the edits for `node`, parsed from `src[base..]`'s start (`base`
/// is non-zero inside re-parsed inline math); `src` is the whole body.
fn collect(
    node: &Node,
    src: &str,
    base: usize,
    cx: Context,
    edits: &mut Vec<(Range<usize>, String)>,
) {
    match node {
        Node::Image(img) => {
            let Some(range) = span(node, base) else {
                return;
            };
            let label = label(&img.alt, &img.url);
            let text = if cx == Context::Link {
                escape(&label)
            } else {
                format!("[{}](<{}>)", escape(&label), angle_url(&img.url))
            };
            edits.push((range, text));
        }
        Node::ImageReference(img) => {
            let Some(range) = span(node, base) else {
                return;
            };
            if cx == Context::Link {
                edits.push((range, escape(&label(&img.alt, ""))));
            } else if src[range.clone()].starts_with('!') {
                // `![alt][ref]` → `[alt][ref]`: the same definition.
                edits.push((range.start..range.start + 1, String::new()));
            }
        }
        Node::Html(html) if cx != Context::Block => {
            if is_paired_tag(&html.value)
                && let Some(range) = span(node, base)
            {
                edits.push((range.start..range.start, "\\".to_owned()));
            }
        }
        Node::InlineMath(_) => {
            // gpui-kit re-parses unclaimed inline math (`$5 and <b>x</b> for
            // $10`) as prose, `$`s included, and renders what it finds; an
            // escape added inside would show literally when the rest is
            // plain. So math holding something to rewrite loses its `$`s
            // (escaped: the text reads the same) and the next pass rewrites
            // its content as prose.
            let Some(range) = span(node, base) else {
                return;
            };
            let Some(literal) = src.get(range.clone()) else {
                return;
            };
            let Ok(Node::Root(root)) = markdown::to_mdast(literal, &prose_options()) else {
                return;
            };
            let mut inner = Vec::new();
            for child in root
                .children
                .iter()
                .flat_map(|c| c.children().into_iter().flatten())
            {
                collect(child, src, range.start, cx, &mut inner);
            }
            if inner.is_empty() {
                return;
            }
            let open = literal.len() - literal.trim_start_matches('$').len();
            let close = literal.len() - literal.trim_end_matches('$').len();
            for i in (0..open).chain(literal.len() - close..literal.len()) {
                edits.push((range.start + i..range.start + i, "\\".to_owned()));
            }
        }
        Node::Link(_) | Node::LinkReference(_) => {
            for child in node.children().into_iter().flatten() {
                collect(child, src, base, Context::Link, edits);
            }
        }
        Node::Root(_)
        | Node::Blockquote(_)
        | Node::List(_)
        | Node::ListItem(_)
        | Node::FootnoteDefinition(_)
        | Node::Table(_)
        | Node::TableRow(_) => {
            for child in node.children().into_iter().flatten() {
                collect(child, src, base, Context::Block, edits);
            }
        }
        _ => {
            let inner = if cx == Context::Link {
                Context::Link
            } else {
                Context::Inline
            };
            for child in node.children().into_iter().flatten() {
                collect(child, src, base, inner, edits);
            }
        }
    }
}

/// An image's link text: its alt text, else its URL, else "image".
fn label(alt: &str, url: &str) -> String {
    let alt = alt.split_whitespace().collect::<Vec<_>>().join(" ");
    if !alt.is_empty() {
        alt
    } else if !url.trim().is_empty() {
        url.trim().to_owned()
    } else {
        "image".to_owned()
    }
}

/// `s` with every ASCII punctuation character backslash-escaped, so it is
/// plain link text.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `url` for a `<…>` link destination, which may not hold `<`, `>` or line
/// endings (and where `\` escapes).
fn angle_url(url: &str) -> String {
    url.trim()
        .replace('\\', "%5C")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('\n', "%0A")
        .replace('\r', "%0D")
}

/// An inline tag gpui-kit turns into formatting when it finds the closing
/// tag among the siblings (its `inline_html_mark`), open or close.
fn is_paired_tag(html: &str) -> bool {
    let Some(inner) = html
        .trim()
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
    else {
        return false;
    };
    if inner.ends_with('/') {
        return false;
    }
    let rest = inner.strip_prefix('/').unwrap_or(inner);
    let name: String = rest
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "strong" | "b" | "em" | "i" | "u" | "s" | "del" | "strike"
    )
}

/// A literal-text node for raw HTML `html`.
fn html_node(name: &'static str, html: &str) -> MarkdownNode {
    MarkdownNode::new(name, ())
        .text(html.to_owned())
        .markdown(html.to_owned())
}

/// Raw HTML blocks (`<div>…</div>`, `<script>`, comments) as literal text.
pub struct HtmlBlockAsText;

impl MarkdownPlugin for HtmlBlockAsText {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "polygloss-html-block"
    }

    fn parse(&self, node: &Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        match node {
            Node::Html(html) => Some(html_node("polygloss-html-block", &html.value)),
            _ => None,
        }
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .children(node.as_text().lines().map(|l| {
                // A blank line keeps its height.
                div().min_h(gpui_kit::rems(1.2)).child(l.to_owned())
            }))
    }
}

/// Raw inline HTML (`<span>`, `<img …>`, `<br>`) as literal text, in the
/// paragraph's own style.
pub struct HtmlInlineAsText;

impl MarkdownPlugin for HtmlInlineAsText {
    fn name(&self) -> &str {
        "polygloss-html-inline"
    }

    fn parse(&self, node: &Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        match node {
            Node::Html(html) => Some(html_node("polygloss-html-inline", &html.value)),
            _ => None,
        }
    }

    /// `None`: gpui-kit shows the node's text inline (atomic text), styled
    /// like the text around it.
    fn render_inline(
        &self,
        _: &MarkdownNode,
        _: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        None
    }
}

/// The bundled image every image request gets (design §19: no remote
/// images; nothing in a comment is ever loaded).
pub fn placeholder_image() -> Arc<Image> {
    static PLACEHOLDER: LazyLock<Arc<Image>> = LazyLock::new(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            include_bytes!("../../../../assets/icons/image-placeholder.svg").to_vec(),
        ))
    });
    PLACEHOLDER.clone()
}

/// The text views' image source: always [`placeholder_image`].
pub fn image_source(_uri: &SharedUri) -> ImageSource {
    ImageSource::Image(placeholder_image())
}

/// Whether a link to `url` may open: `http://`, `https://` or `mailto:`
/// (any case). Everything else (`file:`, `javascript:`, app schemes,
/// relative paths, anchors) is refused.
pub fn link_allowed(url: &str) -> bool {
    let url = url.trim();
    let lower = |n: usize| url.get(..n).map(str::to_ascii_lowercase);
    lower(7).as_deref() == Some("http://")
        || lower(8).as_deref() == Some("https://")
        || lower(7).as_deref() == Some("mailto:")
}

/// Opens `url` in the default app when [`link_allowed`]; logs and ignores
/// anything else.
pub fn open_link(url: &str, cx: &mut App) {
    if link_allowed(url) {
        cx.open_url(url.trim());
    } else {
        tracing::info!("not opening a comment link with a disallowed scheme: {url:?}");
    }
}
