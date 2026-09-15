//! A small HTML and CSS subset with flexbox layout.
//!
//! This is not a browser. It parses a strict subset of HTML, a subset of
//! CSS with type, class and id selectors, computes styles with the usual
//! cascade and inheritance, and lays the result out with
//! [taffy](https://github.com/DioxusLabs/taffy), which gives it real
//! flexbox and block layout rather than an approximation of them. What
//! comes out is a display list of boxes, text runs and images with
//! absolute coordinates, which a renderer paints with the primitives it
//! already has.
//!
//! What it does not have is inline layout: an element's text is one
//! paragraph, and a child element is a box of its own. Everything else it
//! cannot do it refuses by name, so an unsupported property is a
//! diagnostic rather than a silent difference from the browser the
//! document was written in.

#![forbid(unsafe_code)]

pub mod css;
pub mod dom;
pub mod layout;
pub mod style;

use std::fmt;

pub use css::{CssError, KeyframesRule, Stylesheet};
pub use dom::{Document, Element, HtmlError, Node, NodeId, NodeKind};
pub use layout::{Content, Laid, Measure, Painted};
pub use style::declared_box;
pub use style::{Computed, Paint, Shadow, Text, TextAlign};

/// Why a document did not parse, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The markup.
    Html(HtmlError),
    /// The styles.
    Css(CssError),
}

impl Error {
    /// One-based line of the problem.
    pub fn line(&self) -> usize {
        match self {
            Self::Html(e) => e.line,
            Self::Css(e) => e.line,
        }
    }

    /// One-based column, where one is known.
    pub fn column(&self) -> Option<usize> {
        match self {
            Self::Html(e) => Some(e.column),
            Self::Css(_) => None,
        }
    }

    /// The problem without its position.
    pub fn message(&self) -> &str {
        match self {
            Self::Html(e) => &e.message,
            Self::Css(e) => &e.message,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Html(e) => e.fmt(f),
            Self::Css(e) => e.fmt(f),
        }
    }
}

/// A parsed document with a style for every node.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    /// The tree.
    pub doc: Document,
    /// One computed style per node; a text node carries its parent's.
    pub styles: Vec<Computed>,
    /// Declarations that named something geneva does not draw. These do
    /// not stop the document from being drawn; they say what was ignored.
    pub problems: Vec<String>,
    /// `@keyframes` rules from the stylesheet, untouched.
    pub keyframes: std::collections::BTreeMap<String, KeyframesRule>,
    /// The `animation` on the outermost element. Layout and paint do not
    /// play it; it is what the clip drawing this markup animates with, so
    /// a file that moves in a browser moves here too.
    pub animation: Option<String>,
    /// The style of the element that carries that animation, kept so that
    /// a percentage in it can resolve against that element's box rather
    /// than the surface's, which is what CSS does.
    animated: Option<Computed>,
}

impl Prepared {
    /// The border box of the element carrying the animation, against a
    /// surface of `width` by `height`. `None` on an axis its style leaves
    /// to the content.
    pub fn animated_box(&self, width: f32, height: f32) -> (Option<f32>, Option<f32>) {
        self.animated
            .as_ref()
            .map_or((None, None), |s| style::declared_box(s, (width, height)))
    }
}

impl Prepared {
    /// Lays the document out. A dimension that is `None` fits the content.
    pub fn layout<M: Measure>(
        &self,
        width: Option<f32>,
        height: Option<f32>,
        measure: &mut M,
    ) -> Result<Laid, String> {
        layout::layout(&self.doc, &self.styles, width, height, measure)
    }
}

/// Every `src` an `<img>` in a prepared document names, in document
/// order. Like a stylesheet link, this crate does not read them.
pub fn image_sources(prepared: &Prepared) -> Vec<String> {
    prepared
        .doc
        .nodes
        .iter()
        .filter_map(|n| n.element())
        .filter(|e| e.tag == "img")
        .filter_map(|e| e.attrs.get("src").cloned())
        .collect()
}

/// The `href` of every `<link rel="stylesheet">` in some markup, so that
/// a caller can read them before preparing it. This crate never touches
/// the filesystem.
pub fn stylesheet_links(html: &str) -> Result<Vec<String>, Error> {
    dom::parse(html).map(|d| d.links).map_err(Error::Html)
}

/// Parses markup and styles and computes every node's style.
///
/// `linked` holds the contents of the document's `<link
/// rel="stylesheet">` hrefs, which are applied in document order before
/// its `<style>` elements. `extra` is a stylesheet from outside the
/// markup, applied last, so it wins ties at equal specificity.
pub fn prepare(
    html: &str,
    extra: &str,
    linked: &std::collections::BTreeMap<String, String>,
) -> Result<Prepared, Error> {
    let doc = dom::parse(html).map_err(Error::Html)?;
    let mut sheet = css::Stylesheet::default();
    for href in &doc.links {
        if let Some(text) = linked.get(href) {
            let more = css::parse_stylesheet(text).map_err(Error::Css)?;
            sheet.rules.extend(more.rules);
            sheet.keyframes.extend(more.keyframes);
        }
    }
    let own = css::parse_stylesheet(&doc.style).map_err(Error::Css)?;
    sheet.rules.extend(own.rules);
    sheet.keyframes.extend(own.keyframes);
    if !extra.trim().is_empty() {
        let more = css::parse_stylesheet(extra).map_err(Error::Css)?;
        sheet.rules.extend(more.rules);
        // A rule of the same name replaces the markup's, as the field is
        // applied after it.
        sheet.keyframes.extend(more.keyframes);
    }
    let (styles, mut problems) = style::cascade(&doc, &sheet);
    // Only the outermost element's animation is the clip's; anything
    // deeper would have to move inside a picture that is drawn once.
    let outermost: Vec<NodeId> = doc
        .children(doc.root)
        .iter()
        .copied()
        .filter(|id| doc.nodes[*id].element().is_some())
        .collect();
    let mut animation = None;
    let mut animated = None;
    for (id, computed) in styles.iter().enumerate() {
        let Some(spec) = &computed.animation else {
            continue;
        };
        if outermost.first() == Some(&id) {
            animation = Some(spec.clone());
            animated = Some(computed.clone());
        } else if doc.nodes[id].element().is_some() {
            let tag = doc.nodes[id].element().map_or("", |e| e.tag.as_str());
            problems.push(format!(
                "<{tag}>: an animation is played on the outermost element only, \
because the markup is drawn once and the clip moves the picture"
            ));
        }
    }
    Ok(Prepared {
        doc,
        styles,
        problems,
        keyframes: sheet.keyframes,
        animation,
        animated,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const CARD: &str = "<style>\
        @keyframes slide-in { from { transform: translateX(-10px) } to { transform: none } }\
        .card { animation: slide-in 0.5s ease-out; background: #222 }\
        </style><div class='card'><p>hi</p></div>";

    #[test]
    fn the_outermost_animation_is_hoisted_with_its_rules() {
        let p = prepare(CARD, "", &BTreeMap::new()).unwrap();
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert_eq!(p.animation.as_deref(), Some("slide-in 0.5s ease-out"));
        assert_eq!(
            p.keyframes["slide-in"]["from"],
            "transform: translateX(-10px)"
        );
    }

    #[test]
    fn an_animation_further_in_is_named_rather_than_dropped() {
        let p = prepare(
            "<style>@keyframes a { from { opacity: 0 } to { opacity: 1 } } \
             p { animation: a 1s }</style><div><p>hi</p></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(p.animation.is_none());
        assert_eq!(p.problems.len(), 1);
        assert!(
            p.problems[0].contains("outermost element only"),
            "{:?}",
            p.problems
        );
    }

    #[test]
    fn the_animated_elements_box_is_reported_for_percentages() {
        // A percentage width is of the surface; padding and border widen a
        // content-box element and not a border-box one.
        let p = prepare(
            "<style>@keyframes a { from { translate: -100% } to { translate: 0 } }              .c { animation: a 1s; width: 50%; height: 40px; padding: 10px;              border-left: 6px solid red }</style><div class='c'></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(p.animated_box(1000.0, 500.0), (Some(526.0), Some(60.0)));

        let p = prepare(
            "<style>@keyframes a { from { opacity: 0 } to { opacity: 1 } }              .c { animation: a 1s; width: 50%; padding: 10px; box-sizing: border-box }             </style><div class='c'></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        // Border-box takes the width as written, and the height is left to
        // the content, which only laying it out could tell.
        assert_eq!(p.animated_box(1000.0, 500.0), (Some(500.0), None));
    }

    #[test]
    fn a_linked_stylesheet_is_read_when_its_text_is_supplied() {
        let html = "<link rel='stylesheet' href='house.css'><div class='card'>x</div>";
        assert_eq!(stylesheet_links(html).unwrap(), ["house.css"]);

        let mut linked = BTreeMap::new();
        linked.insert(
            "house.css".to_owned(),
            ".card { color: #ff8800; padding: 7px }".to_owned(),
        );
        let p = prepare(html, "", &linked).unwrap();
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        let card = p.doc.children(p.doc.root)[0];
        assert_eq!(p.styles[card].text.color.to_hex(), "#ff8800");

        // A <style> in the markup comes after the link, so it wins ties.
        let p = prepare(
            "<link rel='stylesheet' href='house.css'><style>.card { color: #00ff00 }</style>\
             <div class='card'>x</div>",
            "",
            &linked,
        )
        .unwrap();
        let card = p.doc.children(p.doc.root)[0];
        assert_eq!(p.styles[card].text.color.to_hex(), "#00ff00");
    }

    #[test]
    fn rules_can_come_from_the_css_field_too() {
        let p = prepare(
            "<div class='card'></div>",
            "@keyframes fade { from { opacity: 0 } to { opacity: 1 } } .card { animation: fade 1s }",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(p.animation.as_deref(), Some("fade 1s"));
        assert!(p.keyframes.contains_key("fade"));
    }
}
