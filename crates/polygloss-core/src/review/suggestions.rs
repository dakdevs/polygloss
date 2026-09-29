//! Structural parsing of suggestion blocks with `markdown` (T1.13, design §8.5).
//!
//! A suggestion is a fenced code block whose info string starts with the word
//! `suggestion` (case-sensitive, as on GitHub), at the top level of the comment:
//! a fence quoted in a blockquote or nested in a list item is someone quoting a
//! suggestion, not making one. Parsing uses the GFM mdast, so a ```` ```suggestion ````
//! line inside a longer fence, inline code or an indented code block never counts.

use markdown::ParseOptions;
use markdown::mdast::Node;

/// The replacement text of each top-level ```` ```suggestion ```` block, in order.
/// The text is the block's content without its final newline (`""` proposes
/// deleting the anchored lines). An unclosed fence runs to the end of the body.
pub fn parse_suggestions(body_md: &str) -> Vec<String> {
    // GFM parsing only fails for MDX constructs, which GFM options never enable.
    let Ok(root) = markdown::to_mdast(body_md, &ParseOptions::gfm()) else {
        return Vec::new();
    };
    let Some(children) = root.children() else {
        return Vec::new();
    };
    children
        .iter()
        .filter_map(|node| match node {
            Node::Code(code) if code.lang.as_deref() == Some("suggestion") => {
                Some(code.value.clone())
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_suggestions;

    #[test]
    fn crlf_bodies_and_meta_after_the_language() {
        assert_eq!(
            parse_suggestions("```suggestion\r\na\r\nb\r\n```\r\n"),
            ["a\r\nb"]
        );
        assert_eq!(parse_suggestions("```suggestion   title\nx\n```"), ["x"]);
        assert!(parse_suggestions("").is_empty());
        assert!(parse_suggestions("- ```suggestion\n  x\n  ```").is_empty());
    }
}
