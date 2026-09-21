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
pub use layout::{Content, Group, Laid, Measure, Painted};
pub use style::{
    AnimationSpec, Background, Blend, Computed, Direction, Extent, Overrides, Paint, Shadow, Stop,
    Text, TextAlign, TextFill, declared_box, extent_of, overridden,
};

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
    /// Rules in this markup's own `<style>` that parsed and then matched
    /// no element, so their declarations reached nothing. Usually a
    /// misspelt class or a tag the markup does not use.
    pub unmatched: Vec<String>,
    /// Elements that set `z-index` where it does not apply. A browser
    /// ignores those too, so this says nothing is wrong with the drawing,
    /// only that the declaration is not doing what it looks like.
    pub inert: Vec<String>,
    /// `@keyframes` rules from the stylesheet, untouched.
    pub keyframes: std::collections::BTreeMap<String, KeyframesRule>,
    /// The `animation` on the outermost element, when the document has
    /// one: the root's single element child, which is the whole picture.
    /// Layout and paint do not play it; it is what the clip drawing this
    /// markup animates with, so a file that moves in a browser moves here
    /// too. `None` where the root has several children, since moving the
    /// picture for one of them would move the rest with it; their
    /// animations are in [`Prepared::inner_animations`] instead.
    pub animation: Option<AnimationSpec>,
    /// The element that carries that animation.
    pub animated_node: Option<NodeId>,
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
        layout::layout(
            &self.doc,
            &self.styles,
            width,
            height,
            measure,
            self.animated_node,
        )
    }

    /// Lays the document out with a frame's overrides applied to the
    /// styles first, which is how an element inside the markup is drawn
    /// where its animation puts it at that moment.
    pub fn layout_with<M: Measure>(
        &self,
        width: Option<f32>,
        height: Option<f32>,
        measure: &mut M,
        overrides: &std::collections::BTreeMap<NodeId, Overrides>,
    ) -> Result<Laid, String> {
        if overrides.is_empty() {
            return self.layout(width, height, measure);
        }
        let styles = style::overridden(&self.doc, &self.styles, overrides);
        layout::layout(
            &self.doc,
            &styles,
            width,
            height,
            measure,
            self.animated_node,
        )
    }

    /// Every element carrying an `animation` that the clip does not play,
    /// with what it says. These are the renderer's to play, each
    /// composited as a group of its own. That is every animated element
    /// but the outermost one, and every one of them where the document
    /// has no outermost element for the clip to move.
    #[must_use]
    pub fn inner_animations(&self) -> Vec<(NodeId, &AnimationSpec)> {
        self.styles
            .iter()
            .enumerate()
            .filter(|(id, _)| self.doc.nodes[*id].element().is_some())
            .filter(|(id, _)| self.animated_node != Some(*id))
            .filter_map(|(id, c)| c.animation.as_ref().map(|a| (id, a)))
            .collect()
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

/// Every font family the computed styles name, once each and in order.
///
/// A family here is either a family the machine has or the id of a font
/// asset the document carries, which the painter registers by both. The
/// caller is the one that knows which, so this only reports what was
/// asked for.
pub fn font_families(prepared: &Prepared) -> Vec<String> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for style in &prepared.styles {
        if let Some(family) = style.text.family.as_deref() {
            seen.insert(family);
        }
    }
    seen.into_iter().map(str::to_owned).collect()
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
    // Where this document's own <style> rules sit in the sheet, so that a
    // rule someone wrote here can be told apart from one in a stylesheet
    // shared with other markup.
    let own_from = sheet.rules.len();
    let own = css::parse_stylesheet(&doc.style).map_err(Error::Css)?;
    sheet.rules.extend(own.rules);
    sheet.keyframes.extend(own.keyframes);
    let own_to = sheet.rules.len();
    if !extra.trim().is_empty() {
        let more = css::parse_stylesheet(extra).map_err(Error::Css)?;
        sheet.rules.extend(more.rules);
        // A rule of the same name replaces the markup's, as the field is
        // applied after it.
        sheet.keyframes.extend(more.keyframes);
    }
    let (styles, problems, used) = style::cascade(&doc, &sheet);
    // CSS honours z-index on a positioned box or a flex item and ignores
    // it everywhere else. Matching that keeps a card looking the same in
    // a browser, but forgetting `position` is the usual way to get z-index
    // wrong, so the ones that do nothing are named.
    let mut inert = Vec::new();
    for (id, computed) in styles.iter().enumerate() {
        if computed.z_index.is_some() && !layout::takes_z_index(&doc, &styles, id) {
            let tag = doc.nodes[id]
                .element()
                .map_or("a box", |e| e.tag.as_str())
                .to_owned();
            inert.push(format!(
                "<{tag}>: \"z-index\" applies to a positioned box or a flex item, so it does \
nothing here; a browser ignores it too"
            ));
        }
    }
    // A rule that parses and then matches nothing styles nothing, and
    // until now said nothing either, which is how a misspelt class name
    // produced an unstyled box and a clean report. Only this document's
    // own rules are named: a linked stylesheet is written for more than
    // one piece of markup, so the rules it does not use here are not
    // mistakes.
    let mut unmatched = Vec::new();
    for (rule, hit) in sheet.rules[own_from..own_to]
        .iter()
        .zip(&used[own_from..own_to])
    {
        if !hit {
            let names: Vec<String> = rule.selectors.iter().map(ToString::to_string).collect();
            let n = rule.declarations.len();
            unmatched.push(format!(
                "`{}` matches nothing in this markup, so {}",
                names.join(", "),
                if n == 1 {
                    "its one declaration does nothing".to_owned()
                } else {
                    format!("its {n} declarations do nothing")
                },
            ));
        }
    }
    // The outermost element's animation is the clip's to play; one on an
    // element inside it is the renderer's, which composites that element
    // as a group of its own.
    //
    // The clip moves the whole picture, so the outermost element can only
    // be the clip's when it is the whole picture: the root's one element
    // child, with nothing drawn beside it. A document whose root has
    // several children has no outermost element in that sense, and the
    // first of them is a box among siblings: moving the picture for its
    // animation would move the others with it. Every animation there is
    // played inside, each element composited as its own group, which is
    // what a browser does with any of them.
    let kids = doc.children(doc.root);
    let outermost: Vec<NodeId> = kids
        .iter()
        .copied()
        .filter(|id| doc.nodes[*id].element().is_some())
        .collect();
    let loose_text = kids
        .iter()
        .any(|id| doc.nodes[*id].text().is_some_and(|t| !t.trim().is_empty()));
    let alone = outermost.len() == 1 && !loose_text;
    let mut animation = None;
    let mut animated = None;
    let mut animated_node = None;
    for (id, computed) in styles.iter().enumerate() {
        let Some(spec) = &computed.animation else {
            continue;
        };
        if alone && outermost.first() == Some(&id) {
            animation = Some(spec.clone());
            animated = Some(computed.clone());
            animated_node = Some(id);
        }
    }
    Ok(Prepared {
        doc,
        styles,
        problems,
        unmatched,
        inert,
        keyframes: sheet.keyframes,
        animation,
        animated_node,
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
        assert_eq!(
            p.animation.as_ref().and_then(|a| a.shorthand.as_deref()),
            Some("slide-in 0.5s ease-out")
        );
        assert_eq!(
            p.keyframes["slide-in"]["from"],
            "transform: translateX(-10px)"
        );
    }

    #[test]
    fn a_rule_that_matches_nothing_is_named() {
        let p = prepare(
            "<style>.crad { color: red } section > p { color: red; margin: 0 } \
             .card { color: blue }</style><div class=card></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(p.unmatched.len(), 2, "{:?}", p.unmatched);
        assert!(p.unmatched[0].contains("`.crad`"), "{:?}", p.unmatched);
        assert!(
            p.unmatched[0].contains("its one declaration does nothing"),
            "{:?}",
            p.unmatched
        );
        // The selector is written back out, combinator and all.
        assert!(
            p.unmatched[1].contains("`section > p`"),
            "{:?}",
            p.unmatched
        );
        assert!(
            p.unmatched[1].contains("its 2 declarations do nothing"),
            "{:?}",
            p.unmatched
        );
    }

    #[test]
    fn a_linked_stylesheet_is_not_blamed_for_what_this_file_leaves_alone() {
        // A house stylesheet covers more than one card, so the rules this
        // one does not use are not mistakes.
        let mut linked = BTreeMap::new();
        linked.insert(
            "house.css".to_owned(),
            ".title { color: red } .strap { color: grey }".to_owned(),
        );
        let p = prepare(
            "<link rel=stylesheet href=house.css><div class=title></div>",
            "",
            &linked,
        )
        .unwrap();
        assert!(p.unmatched.is_empty(), "{:?}", p.unmatched);
    }

    #[test]
    fn body_reaches_the_root_and_is_not_reported_as_unmatched() {
        let p = prepare(
            "<style>body { background: #2a6fb0 } html { padding: 4px }</style><div></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(p.unmatched.is_empty(), "{:?}", p.unmatched);
    }

    #[test]
    fn a_first_child_with_siblings_is_not_the_clips_to_play() {
        // The clip moves the whole picture, so it can only play the
        // animation of an element that is the whole picture. Here the
        // animated box is one of three, and playing it on the clip moved
        // the other two with it: the blob drifted and the scene behind it
        // drifted too.
        let p = prepare(
            "<style>@keyframes drift { from { transform: none } \
             to { transform: translate(10px) rotate(12deg) } }\
             .blob { position: absolute; animation: drift 5s }</style>\
             <div class=blob></div><div class=blob></div><div class=scene>hi</div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(p.animation.is_none(), "the clip plays nothing");
        assert!(p.animated_node.is_none());
        // Both blobs are the renderer's, each its own group.
        assert_eq!(p.inner_animations().len(), 2);
    }

    #[test]
    fn a_lone_first_child_is_still_the_clips_to_play() {
        // The wrapper a document puts round the same boxes makes the
        // outermost element the whole picture again, and the clip plays
        // what it carries.
        let p = prepare(
            "<style>@keyframes drift { from { transform: none } \
             to { transform: translate(10px) } }\
             .mesh { animation: drift 5s }</style>\
             <div class=mesh><div class=blob></div><div class=blob></div></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            p.animation.as_ref().and_then(|a| a.shorthand.as_deref()),
            Some("drift 5s")
        );
        assert_eq!(p.inner_animations().len(), 0);
    }

    #[test]
    fn text_beside_the_outermost_element_keeps_the_clip_out_of_it() {
        // Text straight under the root draws beside the element, so the
        // element is not the whole picture and moving the picture would
        // move the text with it.
        let p = prepare(
            "<style>@keyframes drift { from { transform: none } \
             to { transform: translate(10px) } }\
             .card { animation: drift 5s }</style>\
             loose text<div class=card></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(p.animation.is_none());
        assert_eq!(p.inner_animations().len(), 1);
        // Whitespace between the root and its one child is not text that
        // draws, so it does not keep the clip out.
        let q = prepare(
            "<style>@keyframes drift { from { transform: none } \
             to { transform: translate(10px) } }\
             .card { animation: drift 5s }</style>\
             \n  <div class=card></div>\n",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            q.animation.as_ref().and_then(|a| a.shorthand.as_deref()),
            Some("drift 5s")
        );
    }

    #[test]
    fn an_animation_further_in_is_the_renderers_to_play() {
        let p = prepare(
            "<style>@keyframes a { from { opacity: 0 } to { opacity: 1 } } \
             p { animation: a 1s; animation-delay: 0.2s }</style><div><p>hi</p></div>",
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(p.animation.is_none());
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        let inner = p.inner_animations();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0].1.shorthand.as_deref(), Some("a 1s"));
        assert_eq!(
            inner[0].1.longhands,
            vec![("animation-delay".to_owned(), "0.2s".to_owned())]
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
        assert_eq!(
            p.animation.as_ref().and_then(|a| a.shorthand.as_deref()),
            Some("fade 1s")
        );
        assert!(p.keyframes.contains_key("fade"));
    }
}
