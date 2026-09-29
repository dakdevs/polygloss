//! The comment sanitizer (design §8.7, §19 "Agent markdown";
//! library-choices §5). Every body, human or agent, is untrusted:
//!
//! - **Raw HTML is text.** [`HtmlBlockAsText`] and [`HtmlInlineAsText`] claim
//!   every `mdast::Node::Html` (block and inline) and show its source
//!   literally. gpui-kit pairs a few inline formatting tags (`<b>…</b>`,
//!   `<em>`, `<s>`, …) among a paragraph's children *before* any plugin sees
//!   them, so [`prepare`] backslash-escapes those tags up front.
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

/// Rewrites `body` before gpui-kit parses it: images become links, and the
/// inline formatting tags gpui-kit would pair (`b`, `strong`, `i`, `em`,
/// `u`, `s`, `del`, `strike`) are backslash-escaped, so they show as text.
/// Code, and everything else, is left as written.
pub fn prepare(body: &str) -> String {
    if !body.contains('<') && !body.contains('!') {
        return body.to_owned();
    }
    let Ok(root) = markdown::to_mdast(body, &parse_options()) else {
        return body.to_owned();
    };
    let mut edits = Vec::new();
    collect(&root, body, Context::Block, &mut edits);
    if edits.is_empty() {
        return body.to_owned();
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
    out
}

/// Where a node sits: among blocks, in a paragraph's inline content, or
/// inside a link's text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Block,
    Inline,
    Link,
}

fn span(node: &Node) -> Option<Range<usize>> {
    let p = node.position()?;
    Some(p.start.offset..p.end.offset)
}

fn collect(node: &Node, src: &str, cx: Context, edits: &mut Vec<(Range<usize>, String)>) {
    match node {
        Node::Image(img) => {
            let Some(range) = span(node) else { return };
            let label = label(&img.alt, &img.url);
            let text = if cx == Context::Link {
                escape(&label)
            } else {
                format!("[{}](<{}>)", escape(&label), angle_url(&img.url))
            };
            edits.push((range, text));
        }
        Node::ImageReference(img) => {
            let Some(range) = span(node) else { return };
            if cx == Context::Link {
                edits.push((range, escape(&label(&img.alt, ""))));
            } else if src[range.clone()].starts_with('!') {
                // `![alt][ref]` → `[alt][ref]`: the same definition.
                edits.push((range.start..range.start + 1, String::new()));
            }
        }
        Node::Html(html) if cx != Context::Block => {
            if is_paired_tag(&html.value)
                && let Some(range) = span(node)
            {
                edits.push((range.start..range.start, "\\".to_owned()));
            }
        }
        Node::Link(_) | Node::LinkReference(_) => {
            for child in node.children().into_iter().flatten() {
                collect(child, src, Context::Link, edits);
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
                collect(child, src, Context::Block, edits);
            }
        }
        _ => {
            let inner = if cx == Context::Link {
                Context::Link
            } else {
                Context::Inline
            };
            for child in node.children().into_iter().flatten() {
                collect(child, src, inner, edits);
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
