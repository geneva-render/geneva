//! A strict parser for the HTML subset geneva draws.
//!
//! It is not an HTML5 parser: there is no tag inference, no error
//! recovery and no quirks mode. Every element that is not void must be
//! closed, and a mismatch is a diagnostic with a line and column rather
//! than a guess. Documents written for this parser open in a browser and
//! look the same; documents a browser forgives may not parse here, which
//! is the trade the rest of the project makes everywhere else.

use std::collections::BTreeMap;
use std::fmt;

/// Index of a node in a [`Document`].
pub type NodeId = usize;

/// Elements that never have children or a closing tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// A parsed element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Element {
    /// Lowercased tag name.
    pub tag: String,
    /// The `id` attribute.
    pub id: Option<String>,
    /// The `class` attribute, split on whitespace.
    pub classes: Vec<String>,
    /// The `style` attribute, as written.
    pub style: Option<String>,
    /// Every other attribute, lowercased names.
    pub attrs: BTreeMap<String, String>,
}

impl Element {
    /// Whether this element never takes children.
    pub fn is_void(&self) -> bool {
        VOID.contains(&self.tag.as_str())
    }
}

/// An element or a run of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// An element with a tag and attributes.
    Element(Element),
    /// Character data, with entities already resolved.
    Text(String),
}

/// One node of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// What the node is.
    pub kind: NodeKind,
    /// Parent, or `None` for the root.
    pub parent: Option<NodeId>,
    /// Children in document order.
    pub children: Vec<NodeId>,
}

impl Node {
    /// The element, if the node is one.
    pub fn element(&self) -> Option<&Element> {
        match &self.kind {
            NodeKind::Element(e) => Some(e),
            NodeKind::Text(_) => None,
        }
    }

    /// The text, if the node is a text run.
    pub fn text(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Element(_) => None,
            NodeKind::Text(t) => Some(t),
        }
    }
}

/// A parsed document: a flat arena of nodes with one synthetic root, plus
/// the contents of every `<style>` element in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Every node; index with a [`NodeId`].
    pub nodes: Vec<Node>,
    /// The root, which is an element named `:root` wrapping the markup.
    pub root: NodeId,
    /// The text of every `<style>` element, joined in document order.
    pub style: String,
}

impl Document {
    /// The children of a node.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    /// The node's ancestors, nearest first, not including itself.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = self.nodes[id].parent;
        while let Some(p) = cur {
            out.push(p);
            cur = self.nodes[p].parent;
        }
        out
    }
}

/// Why a document did not parse, with where it went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlError {
    /// What is wrong, in one line.
    pub message: String,
    /// One-based line in the source.
    pub line: usize,
    /// One-based column in the source.
    pub column: usize,
}

impl fmt::Display for HtmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (line {}, column {})",
            self.message, self.line, self.column
        )
    }
}

/// Parses the HTML subset into a tree.
pub fn parse(source: &str) -> Result<Document, HtmlError> {
    Parser {
        src: source.as_bytes(),
        text: source,
        pos: 0,
        nodes: Vec::new(),
        style: String::new(),
        self_closing: false,
    }
    .run()
}

struct Parser<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    nodes: Vec<Node>,
    style: String,
    /// Set by `take_element` and read straight after it, because every
    /// other caller of it ignores whether the tag closed itself.
    self_closing: bool,
}

impl Parser<'_> {
    fn run(mut self) -> Result<Document, HtmlError> {
        let root = self.push(
            NodeKind::Element(Element {
                tag: ":root".to_owned(),
                ..Element::default()
            }),
            None,
        );
        let mut open = vec![root];
        loop {
            self.take_text(*open.last().unwrap_or(&root));
            if self.pos >= self.src.len() {
                break;
            }
            if self.eat("<!--") {
                self.skip_to("-->", "an unclosed comment")?;
                continue;
            }
            if self.eat("<!") || self.eat("<?") {
                self.skip_to(">", "an unclosed declaration")?;
                continue;
            }
            if self.eat("</") {
                let name = self.take_name()?;
                self.skip_space();
                if !self.eat(">") {
                    return Err(self.error(format!("expected \">\" after </{name}")));
                }
                match open.pop() {
                    Some(id) if id != root && self.tag_of(id) == name => {}
                    Some(id) => {
                        let what = if id == root {
                            format!("</{name}> closes nothing")
                        } else {
                            format!("</{name}> closes <{}>", self.tag_of(id))
                        };
                        return Err(self.error(what));
                    }
                    None => return Err(self.error(format!("</{name}> closes nothing"))),
                }
                continue;
            }
            if !self.eat("<") {
                // A stray "<" that starts nothing is text.
                let parent = *open.last().unwrap_or(&root);
                self.pos += 1;
                self.push(NodeKind::Text("<".to_owned()), Some(parent));
                continue;
            }
            let start = self.pos;
            let element = self.take_element()?;
            if element.tag == "style" {
                let body = self.take_raw_text("style")?;
                self.style.push_str(&body);
                self.style.push('\n');
                continue;
            }
            if element.tag == "script" {
                self.pos = start;
                return Err(self.error("<script> is not run or drawn; remove it".to_owned()));
            }
            let void = element.is_void();
            let closed = self.self_closing;
            let parent = *open.last().unwrap_or(&root);
            let id = self.push(NodeKind::Element(element), Some(parent));
            if !void && !closed {
                open.push(id);
            }
        }
        if let Some(id) = open.into_iter().rev().find(|id| *id != root) {
            return Err(self.error(format!("<{}> is never closed", self.tag_of(id))));
        }
        Ok(Document {
            nodes: self.nodes,
            root,
            style: self.style,
        })
    }

    fn push(&mut self, kind: NodeKind, parent: Option<NodeId>) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            kind,
            parent,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(id);
        }
        id
    }

    fn tag_of(&self, id: NodeId) -> String {
        self.nodes[id]
            .element()
            .map_or_else(String::new, |e| e.tag.clone())
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.src[self.pos..].starts_with(s.as_bytes()) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn skip_to(&mut self, end: &str, what: &str) -> Result<(), HtmlError> {
        match self.text[self.pos..].find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(())
            }
            None => Err(self.error(what.to_owned())),
        }
    }

    /// Text up to the next `<`, pushed as a node when it is not blank.
    fn take_text(&mut self, parent: NodeId) {
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos] != b'<' {
            self.pos += 1;
        }
        if self.pos > start {
            let raw = &self.text[start..self.pos];
            if !raw.trim().is_empty() {
                let text = entities(raw);
                self.push(NodeKind::Text(text), Some(parent));
            }
        }
    }

    /// The body of a raw-text element such as `<style>`.
    fn take_raw_text(&mut self, tag: &str) -> Result<String, HtmlError> {
        let close = format!("</{tag}");
        let rest = &self.text[self.pos..];
        let Some(i) = rest.to_ascii_lowercase().find(&close) else {
            return Err(self.error(format!("<{tag}> is never closed")));
        };
        let body = rest[..i].to_owned();
        self.pos += i;
        self.eat(&close);
        self.skip_space();
        if !self.eat(">") {
            return Err(self.error(format!("expected \">\" after </{tag}")));
        }
        Ok(body)
    }

    fn take_name(&mut self) -> Result<String, HtmlError> {
        let start = self.pos;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b':' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.error("expected a tag or attribute name".to_owned()));
        }
        Ok(self.text[start..self.pos].to_ascii_lowercase())
    }

    fn take_element(&mut self) -> Result<Element, HtmlError> {
        let tag = self.take_name()?;
        let mut el = Element {
            tag,
            ..Element::default()
        };
        self.self_closing = false;
        loop {
            self.skip_space();
            if self.eat("/>") {
                self.self_closing = true;
                return Ok(el);
            }
            if self.eat(">") {
                return Ok(el);
            }
            if self.pos >= self.src.len() {
                return Err(self.error(format!("<{}> is never finished", el.tag)));
            }
            let name = self.take_name()?;
            self.skip_space();
            let value = if self.eat("=") {
                self.skip_space();
                Some(self.take_value()?)
            } else {
                None
            };
            match name.as_str() {
                "id" => el.id = value,
                "class" => {
                    el.classes = value
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                }
                "style" => el.style = value,
                _ => {
                    el.attrs.insert(name, value.unwrap_or_default());
                }
            }
        }
    }

    fn take_value(&mut self) -> Result<String, HtmlError> {
        let quote = self.src.get(self.pos).copied();
        if quote == Some(b'"') || quote == Some(b'\'') {
            let q = quote.unwrap() as char;
            self.pos += 1;
            let start = self.pos;
            match self.text[start..].find(q) {
                Some(i) => {
                    let v = entities(&self.text[start..start + i]);
                    self.pos = start + i + 1;
                    Ok(v)
                }
                None => Err(self.error("an attribute value is never closed".to_owned())),
            }
        } else {
            let start = self.pos;
            while self.pos < self.src.len() {
                let c = self.src[self.pos];
                if c.is_ascii_whitespace() || c == b'>' || c == b'/' {
                    break;
                }
                self.pos += 1;
            }
            if self.pos == start {
                return Err(self.error("expected an attribute value".to_owned()));
            }
            Ok(entities(&self.text[start..self.pos]))
        }
    }

    fn error(&self, message: String) -> HtmlError {
        let before = &self.text[..self.pos.min(self.text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before
            .rfind('\n')
            .map_or(before.len(), |i| before.len() - i - 1)
            + 1;
        HtmlError {
            message,
            line,
            column,
        }
    }
}

/// Resolves the entities a static document actually uses.
fn entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[1..].find(';').map(|j| j + 1) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let replacement = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "middot" => Some('\u{b7}'),
            "bull" => Some('\u{2022}'),
            "hellip" => Some('\u{2026}'),
            "ndash" => Some('\u{2013}'),
            "mdash" => Some('\u{2014}'),
            "lsquo" => Some('\u{2018}'),
            "rsquo" => Some('\u{2019}'),
            "ldquo" => Some('\u{201c}'),
            "rdquo" => Some('\u{201d}'),
            "laquo" => Some('\u{ab}'),
            "raquo" => Some('\u{bb}'),
            "times" => Some('\u{d7}'),
            "divide" => Some('\u{f7}'),
            "plusmn" => Some('\u{b1}'),
            "deg" => Some('\u{b0}'),
            "copy" => Some('\u{a9}'),
            "reg" => Some('\u{ae}'),
            "trade" => Some('\u{2122}'),
            "sect" => Some('\u{a7}'),
            "para" => Some('\u{b6}'),
            "dagger" => Some('\u{2020}'),
            "euro" => Some('\u{20ac}'),
            "pound" => Some('\u{a3}'),
            "yen" => Some('\u{a5}'),
            "cent" => Some('\u{a2}'),
            "frac12" => Some('\u{bd}'),
            "frac14" => Some('\u{bc}'),
            "frac34" => Some('\u{be}'),
            "larr" => Some('\u{2190}'),
            "rarr" => Some('\u{2192}'),
            "uarr" => Some('\u{2191}'),
            "darr" => Some('\u{2193}'),
            "shy" => Some('\u{ad}'),
            "ensp" => Some('\u{2002}'),
            "emsp" => Some('\u{2003}'),
            "thinsp" => Some('\u{2009}'),
            n => n
                .strip_prefix('#')
                .and_then(|d| match d.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => d.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match replacement {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Document {
        parse(src).unwrap()
    }

    #[test]
    fn builds_a_tree() {
        let d = doc("<div class='card a'><h1 id=t>Hi</h1><img src=x.png></div>");
        let card = d.children(d.root)[0];
        let el = d.nodes[card].element().unwrap();
        assert_eq!(el.tag, "div");
        assert_eq!(el.classes, ["card", "a"]);
        let kids = d.children(card);
        assert_eq!(kids.len(), 2);
        let h1 = d.nodes[kids[0]].element().unwrap();
        assert_eq!(h1.id.as_deref(), Some("t"));
        assert_eq!(d.nodes[d.children(kids[0])[0]].text(), Some("Hi"));
        let img = d.nodes[kids[1]].element().unwrap();
        assert_eq!(img.attrs.get("src").map(String::as_str), Some("x.png"));
        assert!(d.children(kids[1]).is_empty());
    }

    #[test]
    fn collects_style_elements_and_skips_comments() {
        let d = doc("<!-- hi --><style>.a { color: red }</style><style>b{}</style><p>x</p>");
        assert!(d.style.contains(".a { color: red }"));
        assert!(d.style.contains("b{}"));
        assert_eq!(d.children(d.root).len(), 1);
    }

    #[test]
    fn resolves_entities() {
        let d = doc("<p>a &amp; b &lt;c&gt; &#39;d&#39; &nbsp; &#x41;</p>");
        let text = d.nodes[d.children(d.children(d.root)[0])[0]]
            .text()
            .unwrap();
        assert_eq!(text, "a & b <c> 'd' \u{a0} A");
    }

    #[test]
    fn self_closing_and_void_take_no_children() {
        let d = doc("<div><br><span/>tail</div>");
        let div = d.children(d.root)[0];
        assert_eq!(d.children(div).len(), 3);
    }

    #[test]
    fn mismatched_tags_point_at_the_problem() {
        let e = parse("<div>\n  <p>hi</div>\n</p>").unwrap_err();
        assert!(e.message.contains("</div> closes <p>"), "{e}");
        assert_eq!(e.line, 2);

        let e = parse("<div><p>hi</p>").unwrap_err();
        assert!(e.message.contains("<div> is never closed"), "{e}");

        let e = parse("</p>").unwrap_err();
        assert!(e.message.contains("closes nothing"), "{e}");

        assert!(parse("<script>alert(1)</script>").is_err());
        assert!(parse("<div class='x>hi</div>").is_err());
    }

    #[test]
    fn blank_text_between_tags_is_dropped() {
        let d = doc("<div>\n   <p>x</p>\n</div>");
        assert_eq!(d.children(d.children(d.root)[0]).len(), 1);
    }
}
