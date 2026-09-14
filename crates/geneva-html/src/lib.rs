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

pub use css::{CssError, Stylesheet};
pub use dom::{Document, Element, HtmlError, Node, NodeId, NodeKind};
pub use layout::{Content, Laid, Measure, Painted};
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
}

impl Prepared {
    /// Lays the document out in a box `width` wide, and `height` tall when
    /// one is given; without a height the box fits its content.
    pub fn layout<M: Measure>(
        &self,
        width: f32,
        height: Option<f32>,
        measure: &mut M,
    ) -> Result<Laid, String> {
        layout::layout(&self.doc, &self.styles, width, height, measure)
    }
}

/// Parses markup and styles and computes every node's style.
///
/// `extra` is a stylesheet from outside the markup; it is applied after
/// any `<style>` element, so it wins ties at equal specificity.
pub fn prepare(html: &str, extra: &str) -> Result<Prepared, Error> {
    let doc = dom::parse(html).map_err(Error::Html)?;
    let mut sheet = css::parse_stylesheet(&doc.style).map_err(Error::Css)?;
    if !extra.trim().is_empty() {
        sheet
            .rules
            .extend(css::parse_stylesheet(extra).map_err(Error::Css)?.rules);
    }
    let (styles, problems) = style::cascade(&doc, &sheet);
    Ok(Prepared {
        doc,
        styles,
        problems,
    })
}
