//! Layout with taffy, and the display list that comes out of it.
//!
//! Every element becomes one taffy node, and every run of text becomes a
//! leaf whose size the renderer measures. What comes back is a flat list
//! of boxes with absolute coordinates, in the order they are painted.

use taffy::prelude::*;
use taffy::style::Overflow;
use taffy::{AvailableSpace, TaffyTree};

use crate::dom::{Document, NodeId as DomId};
use crate::style::{Computed, Paint, Text};

/// What the renderer has to answer while laying out.
pub trait Measure {
    /// The size a run of text takes, wrapped to `width` when there is one.
    fn text(&mut self, text: &str, style: &Text, width: Option<f32>) -> (f32, f32);

    /// The natural size of an image, if it can be opened.
    fn image(&mut self, src: &str) -> Option<(f32, f32)>;
}

/// What a box draws inside its border.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// Nothing; the background and border are the whole box.
    Empty,
    /// A run of text.
    Text {
        /// The text, with whitespace already collapsed unless `pre`.
        text: String,
        /// How to draw it.
        style: Text,
    },
    /// An image, drawn to fill the content box.
    Image {
        /// The `src` attribute, as written.
        src: String,
    },
}

/// A rectangle: x, y, width, height.
pub type Rectangle = [f32; 4];

/// One box of the display list, in paint order.
#[derive(Debug, Clone, PartialEq)]
pub struct Painted {
    /// The border box in the composition's pixels.
    pub rect: Rectangle,
    /// The content box, inside border and padding.
    pub content_rect: Rectangle,
    /// What it draws.
    pub content: Content,
    /// Background, border colours, radii and shadow.
    pub paint: Paint,
    /// Border widths: top, right, bottom, left.
    pub border: [f32; 4],
    /// The clip an ancestor with `overflow: hidden` imposes, with its radii.
    pub clip: Option<(Rectangle, [f64; 4])>,
    /// Opacity, with every ancestor's multiplied in up to the innermost
    /// group, whose own opacity is applied when the group is composited.
    pub opacity: f32,
    /// The node this came from, for diagnostics.
    pub node: DomId,
    /// The innermost group this box is painted inside, an index into
    /// [`Laid::groups`], or `None` for the surface itself.
    pub group: Option<usize>,
}

/// A subtree painted into a buffer of its own and then composited as one
/// picture: an element with an `opacity` below one, a `filter`, or an
/// `animation` the renderer plays. Its boxes are contiguous in paint
/// order, since such an element is a stacking context.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The element.
    pub node: DomId,
    /// The group this one is inside, if any.
    pub parent: Option<usize>,
    /// The element's border box, which a transform turns about.
    pub rect: Rectangle,
    /// The element's own opacity, applied to the composited picture.
    pub opacity: f32,
    /// `filter: blur()` in pixels, applied to the composited picture.
    pub blur: f64,
    /// The clip an ancestor outside the group imposes, applied to the
    /// composited picture rather than to the boxes inside.
    pub clip: Option<(Rectangle, [f64; 4])>,
    /// `clip-path: polygon()` in the surface's pixels, applied to the
    /// group's picture before its transform.
    pub clip_path: Option<Vec<(f64, f64)>>,
}

/// The laid-out document.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    /// Boxes in paint order.
    pub boxes: Vec<Painted>,
    /// The size the root settled on.
    pub size: (f32, f32),
    /// The groups the boxes belong to, outer ones before inner.
    pub groups: Vec<Group>,
}

/// What a taffy leaf stands for.
#[derive(Debug, Clone)]
enum Leaf {
    Text(DomId),
    Image(DomId),
}

/// Lays the document out. A dimension that is `None` is sized to the
/// content, which is how a card fits itself to the text inside it.
/// `played_by_clip` is the element whose animation the clip plays, so
/// it does not open a group of its own for it.
pub fn layout<M: Measure>(
    doc: &Document,
    styles: &[Computed],
    width: Option<f32>,
    height: Option<f32>,
    measure: &mut M,
    played_by_clip: Option<DomId>,
) -> Result<Laid, String> {
    let mut tree: TaffyTree<Leaf> = TaffyTree::new();
    let mut map: Vec<Option<NodeId>> = vec![None; doc.nodes.len()];
    let mut root_style = styles[doc.root].layout.clone();
    root_style.size = Size {
        width: width.map_or_else(Dimension::auto, Dimension::length),
        height: height.map_or_else(Dimension::auto, Dimension::length),
    };
    // The size given is the box the markup is drawn into, so padding on
    // the root goes inside it, the way it does on a page. Content-box
    // sizing would push the root past the box instead.
    root_style.box_sizing = BoxSizing::BorderBox;
    let mut styles = styles.to_vec();
    styles[doc.root].layout = root_style;
    let styles = &styles[..];
    let root = build(doc, styles, doc.root, &mut tree, &mut map)?;

    tree.compute_layout_with_measure(
        root,
        Size {
            width: width.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
            height: height.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
        },
        |known, available, _id, context, _style| {
            let Some(leaf) = context else {
                return Size::ZERO;
            };
            match leaf {
                Leaf::Text(dom) => {
                    let style = &styles[*dom].text;
                    let text = doc.nodes[*dom].text().unwrap_or_default();
                    let limit = known.width.or(match available.width {
                        AvailableSpace::Definite(w) => Some(w),
                        AvailableSpace::MinContent => Some(0.0),
                        AvailableSpace::MaxContent => None,
                    });
                    let (w, h) = measure.text(text, style, limit);
                    Size {
                        width: known.width.unwrap_or(w),
                        height: known.height.unwrap_or(h),
                    }
                }
                Leaf::Image(dom) => {
                    let src = doc.nodes[*dom]
                        .element()
                        .and_then(|e| e.attrs.get("src"))
                        .map_or("", String::as_str);
                    let (w, h) = measure.image(src).unwrap_or((0.0, 0.0));
                    match (known.width, known.height) {
                        (Some(w), Some(h)) => Size {
                            width: w,
                            height: h,
                        },
                        // One given dimension keeps the aspect ratio.
                        (Some(kw), None) if w > 0.0 => Size {
                            width: kw,
                            height: kw * h / w,
                        },
                        (None, Some(kh)) if h > 0.0 => Size {
                            width: kh * w / h,
                            height: kh,
                        },
                        _ => Size {
                            width: w,
                            height: h,
                        },
                    }
                }
            }
        },
    )
    .map_err(|e| format!("layout failed: {e}"))?;

    let mut boxes = Vec::new();
    let mut groups = Vec::new();
    let size = tree.layout(root).map_err(|e| e.to_string())?.size;
    let mut walk = Walk {
        doc,
        styles,
        tree: &tree,
        map: &map,
        played_by_clip,
        groups: &mut groups,
    };
    let root_group = walk.open_group(doc.root, (0.0, 0.0), None)?;
    walk.paint_order(doc.root, (0.0, 0.0), 1.0, None, root_group, &mut boxes)?;
    Ok(Laid {
        boxes,
        size: (size.width, size.height),
        groups,
    })
}

/// What the paint-order walk carries along.
struct Walk<'a> {
    doc: &'a Document,
    styles: &'a [Computed],
    tree: &'a TaffyTree<Leaf>,
    map: &'a [Option<NodeId>],
    played_by_clip: Option<DomId>,
    groups: &'a mut Vec<Group>,
}

impl Walk<'_> {
    /// Whether an element is composited as one picture.
    fn is_group(&self, dom: DomId) -> bool {
        let style = &self.styles[dom];
        self.doc.nodes[dom].element().is_some()
            && (style.paint.opacity < 1.0
                || style.paint.blur > 0.0
                || style.paint.clip_path.is_some()
                || (style.animation.is_some() && self.played_by_clip != Some(dom)))
    }

    /// Records a group for an element that opens one, with its box.
    fn open_group(
        &mut self,
        dom: DomId,
        origin: (f32, f32),
        parent: Option<usize>,
    ) -> Result<Option<usize>, String> {
        if !self.is_group(dom) {
            return Ok(None);
        }
        let Some(id) = self.map[dom] else {
            return Ok(None);
        };
        let l = self.tree.layout(id).map_err(|e| e.to_string())?;
        let style = &self.styles[dom];
        let rect = [
            origin.0 + l.location.x,
            origin.1 + l.location.y,
            l.size.width,
            l.size.height,
        ];
        let clip_path = style.paint.clip_path.as_ref().map(|points| {
            points
                .iter()
                .map(|(x, y)| {
                    (
                        f64::from(rect[0]) + x.size(f64::from(rect[2])),
                        f64::from(rect[1]) + y.size(f64::from(rect[3])),
                    )
                })
                .collect()
        });
        self.groups.push(Group {
            node: dom,
            parent,
            rect,
            opacity: style.paint.opacity as f32,
            blur: style.paint.blur,
            clip: None,
            clip_path,
        });
        Ok(Some(self.groups.len() - 1))
    }
}

/// Builds the taffy tree for one node and its children.
fn build(
    doc: &Document,
    styles: &[Computed],
    dom: DomId,
    tree: &mut TaffyTree<Leaf>,
    map: &mut [Option<NodeId>],
) -> Result<NodeId, String> {
    let node = &doc.nodes[dom];
    if node.text().is_some() {
        let id = tree
            .new_leaf_with_context(styles[dom].layout.clone(), Leaf::Text(dom))
            .map_err(|e| e.to_string())?;
        map[dom] = Some(id);
        return Ok(id);
    }
    let element = node
        .element()
        .ok_or("a node that is neither text nor an element")?;
    if element.tag == "img" {
        let id = tree
            .new_leaf_with_context(styles[dom].layout.clone(), Leaf::Image(dom))
            .map_err(|e| e.to_string())?;
        map[dom] = Some(id);
        return Ok(id);
    }
    let mut children = Vec::new();
    for child in doc.children(dom) {
        if styles[*child].layout.display == Display::None {
            continue;
        }
        children.push(build(doc, styles, *child, tree, map)?);
    }
    let id = tree
        .new_with_children(styles[dom].layout.clone(), &children)
        .map_err(|e| e.to_string())?;
    map[dom] = Some(id);
    Ok(id)
}

/// The three piles CSS paints a stacking context from, in this order:
/// what is in flow, then what is positioned without a z-index of its own,
/// then the contexts that do have one. Negative ones go under the flow
/// rather than over it.
#[derive(Default)]
struct Piles {
    flow: Vec<Painted>,
    positioned: Vec<Painted>,
    contexts: Vec<(i32, usize, Vec<Painted>)>,
}

impl Piles {
    /// Flattens the piles into paint order.
    fn flatten(mut self, own: Option<Painted>, out: &mut Vec<Painted>) {
        self.contexts.sort_by_key(|(z, order, _)| (*z, *order));
        let split = self.contexts.partition_point(|(z, _, _)| *z < 0);
        out.extend(own);
        for (_, _, boxes) in self.contexts.drain(..split) {
            out.extend(boxes);
        }
        out.append(&mut self.flow);
        out.append(&mut self.positioned);
        for (_, _, boxes) in self.contexts.drain(..) {
            out.extend(boxes);
        }
    }
}

/// Whether `z-index` applies to this box. CSS honours it on a positioned
/// box or a flex item and ignores it everywhere else, and a card that
/// looks right here has to look right in a browser.
pub(crate) fn takes_z_index(doc: &Document, styles: &[Computed], dom: DomId) -> bool {
    styles[dom].positioned
        || doc.nodes[dom]
            .parent
            .is_some_and(|p| styles[p].layout.display == Display::Flex)
}

impl Walk<'_> {
    /// What one node contributes: its own box, where its children start,
    /// and the clip they inherit. Laying this out is separate from
    /// deciding the order the boxes are painted in. A node that opens a
    /// group has `group` set to it, and its own opacity is left for the
    /// group's compositing rather than multiplied into its boxes.
    #[allow(clippy::type_complexity)]
    fn box_of(
        &self,
        dom: DomId,
        origin: (f32, f32),
        opacity: f32,
        clip: Option<(Rectangle, [f64; 4])>,
        group: Option<usize>,
        opens_group: bool,
    ) -> Result<
        Option<(
            Option<Painted>,
            (f32, f32),
            f32,
            Option<(Rectangle, [f64; 4])>,
        )>,
        String,
    > {
        let doc = self.doc;
        let Some(id) = self.map[dom] else {
            return Ok(None);
        };
        let l = self.tree.layout(id).map_err(|e| e.to_string())?;
        let x = origin.0 + l.location.x;
        let y = origin.1 + l.location.y;
        let style = &self.styles[dom];
        let opacity = if opens_group {
            opacity
        } else {
            opacity * style.paint.opacity as f32
        };
        let border = [l.border.top, l.border.right, l.border.bottom, l.border.left];
        let rect: Rectangle = [x, y, l.size.width, l.size.height];
        let content_rect: Rectangle = [
            x + border[3] + l.padding.left,
            y + border[0] + l.padding.top,
            (l.size.width - border[1] - border[3] - l.padding.left - l.padding.right).max(0.0),
            (l.size.height - border[0] - border[2] - l.padding.top - l.padding.bottom).max(0.0),
        ];

        let content = match &doc.nodes[dom].kind {
            crate::dom::NodeKind::Text(text) => Content::Text {
                text: collapse(text, style.text.pre),
                style: style.text.clone(),
            },
            crate::dom::NodeKind::Element(el) if el.tag == "img" => Content::Image {
                src: el.attrs.get("src").cloned().unwrap_or_default(),
            },
            crate::dom::NodeKind::Element(_) => Content::Empty,
        };

        // The root stands in for the page, so it draws only what someone
        // asked it to draw with a `body` or `html` rule. Left alone it marks
        // nothing, which is what keeps the painted area down to the content.
        let root_draws = dom == doc.root
            && (style.paint.background.is_some()
                || !style.paint.shadow.is_empty()
                || border.iter().any(|w| *w > 0.0));
        let own = (dom != doc.root || root_draws).then(|| Painted {
            rect,
            content_rect,
            content,
            paint: style.paint.clone(),
            border,
            clip,
            opacity,
            node: dom,
            group,
        });

        let child_clip = if style.layout.overflow.x == Overflow::Visible
            && style.layout.overflow.y == Overflow::Visible
        {
            clip
        } else {
            Some((rect, style.paint.radius))
        };
        Ok(Some((own, (x, y), opacity, child_clip)))
    }

    /// Paints a whole stacking context: the node's own box, then
    /// everything under it in the order CSS paints a context. `group` is
    /// the group the node is painted inside; when the node opened it,
    /// the group's own opacity and outside clip are kept for compositing.
    fn paint_order(
        &mut self,
        dom: DomId,
        origin: (f32, f32),
        opacity: f32,
        clip: Option<(Rectangle, [f64; 4])>,
        group: Option<usize>,
        out: &mut Vec<Painted>,
    ) -> Result<(), String> {
        let opens_group = group.is_some_and(|g| self.groups[g].node == dom);
        let (opacity, clip) = if opens_group {
            if let Some(g) = group {
                self.groups[g].clip = clip;
            }
            (1.0, None)
        } else {
            (opacity, clip)
        };
        let Some((own, at, opacity, child_clip)) =
            self.box_of(dom, origin, opacity, clip, group, opens_group)?
        else {
            return Ok(());
        };
        let mut piles = Piles::default();
        for (order, child) in self.doc.children(dom).to_vec().iter().enumerate() {
            self.sort_into(*child, at, opacity, child_clip, group, order, &mut piles)?;
        }
        piles.flatten(own, out);
        Ok(())
    }

    /// Puts one node into the piles of the context it belongs to. A node
    /// with a z-index of its own opens a context and is painted whole,
    /// where its z-index puts it. A group is painted whole too, in tree
    /// order with the positioned boxes, which is where CSS paints an
    /// element with opacity or a filter (its own z-index apart). A
    /// positioned node without one is also painted whole, so its children
    /// stay with it. Anything else joins the flow, and its children keep
    /// filling the same piles, which is how a z-index deeper in can still
    /// rise above an uncle.
    #[allow(clippy::too_many_arguments)]
    fn sort_into(
        &mut self,
        dom: DomId,
        origin: (f32, f32),
        opacity: f32,
        clip: Option<(Rectangle, [f64; 4])>,
        group: Option<usize>,
        order: usize,
        piles: &mut Piles,
    ) -> Result<(), String> {
        let z = self.styles[dom]
            .z_index
            .filter(|_| takes_z_index(self.doc, self.styles, dom));
        let opened = self.open_group(dom, origin, group)?;
        let inner = opened.or(group);
        if let Some(z) = z {
            let mut boxes = Vec::new();
            self.paint_order(dom, origin, opacity, clip, inner, &mut boxes)?;
            piles.contexts.push((z, order, boxes));
            return Ok(());
        }
        if opened.is_some() || self.styles[dom].positioned {
            let mut boxes = Vec::new();
            self.paint_order(dom, origin, opacity, clip, inner, &mut boxes)?;
            piles.positioned.append(&mut boxes);
            return Ok(());
        }
        let Some((own, at, opacity, child_clip)) =
            self.box_of(dom, origin, opacity, clip, group, false)?
        else {
            return Ok(());
        };
        piles.flow.extend(own);
        for (i, child) in self.doc.children(dom).to_vec().iter().enumerate() {
            self.sort_into(*child, at, opacity, child_clip, group, order + i, piles)?;
        }
        Ok(())
    }
}

/// CSS whitespace collapsing, unless the element asked to keep it.
fn collapse(text: &str, pre: bool) -> String {
    if pre {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parse_stylesheet;
    use crate::dom::parse;

    /// A measurer with a fixed character cell, so sizes are predictable.
    struct Cells;

    impl Measure for Cells {
        fn text(&mut self, text: &str, style: &Text, width: Option<f32>) -> (f32, f32) {
            let cell = style.size as f32 * 0.5;
            let line = style.size as f32 * style.line_height as f32;
            let chars = text.chars().count() as f32;
            match width {
                Some(w) if w > 0.0 && chars * cell > w => {
                    let per_line = (w / cell).floor().max(1.0);
                    (w, (chars / per_line).ceil() * line)
                }
                _ => (chars * cell, line),
            }
        }

        fn image(&mut self, _src: &str) -> Option<(f32, f32)> {
            Some((100.0, 50.0))
        }
    }

    fn lay(html: &str, w: f32, h: f32) -> (Document, Laid) {
        let doc = parse(html).unwrap();
        let sheet = parse_stylesheet(&doc.style).unwrap();
        let (styles, problems, _) = crate::style::cascade(&doc, &sheet);
        assert!(problems.is_empty(), "{problems:?}");
        let laid = layout(&doc, &styles, Some(w), Some(h), &mut Cells, None).unwrap();
        (doc, laid)
    }

    /// The order boxes come out in, by class, so paint order is readable.
    fn order(doc: &Document, laid: &Laid) -> Vec<String> {
        laid.boxes
            .iter()
            .filter_map(|b| doc.nodes[b.node].element())
            .map(|e| e.classes.join("."))
            .filter(|c| !c.is_empty())
            .collect()
    }

    #[test]
    fn z_index_lifts_a_box_above_one_written_after_it() {
        // Without a z-index the later box wins, as it does on a page.
        let (doc, laid) = lay(
            "<style>div { position: absolute; width: 50px; height: 50px }</style>\
             <div class=a></div><div class=b></div>",
            200.0,
            100.0,
        );
        assert_eq!(order(&doc, &laid), ["a", "b"]);

        // With one, the earlier box is painted last, so it sits on top.
        let (doc, laid) = lay(
            "<style>div { position: absolute; width: 50px; height: 50px }\
             .a { z-index: 5 }</style><div class=a></div><div class=b></div>",
            200.0,
            100.0,
        );
        assert_eq!(order(&doc, &laid), ["b", "a"]);
    }

    #[test]
    fn a_negative_z_index_goes_under_the_flow() {
        let (doc, laid) = lay(
            "<style>.back { position: absolute; z-index: -1; width: 50px; height: 50px }\
             .front { width: 50px; height: 50px }</style>\
             <div class=back></div><div class=front></div>",
            200.0,
            100.0,
        );
        // Written first and still painted first, because negative z-index
        // puts it under everything in flow rather than merely behind its
        // sibling.
        assert_eq!(order(&doc, &laid), ["back", "front"]);

        // Flip it: the flow box is written first, and the negative one
        // still goes under.
        let (doc, laid) = lay(
            "<style>.back { position: absolute; z-index: -1; width: 50px; height: 50px }\
             .front { width: 50px; height: 50px }</style>\
             <div class=front></div><div class=back></div>",
            200.0,
            100.0,
        );
        assert_eq!(order(&doc, &laid), ["back", "front"]);
    }

    #[test]
    fn a_z_index_deeper_in_still_rises_above_an_uncle() {
        // The plain wrapper opens no stacking context, so the badge's
        // z-index is measured against the uncle, not against its siblings.
        let (doc, laid) = lay(
            "<style>.badge { position: absolute; z-index: 9; width: 20px; height: 20px }\
             .uncle { position: absolute; width: 50px; height: 50px }</style>\
             <div class=wrap><div class=badge></div></div><div class=uncle></div>",
            200.0,
            100.0,
        );
        assert_eq!(order(&doc, &laid), ["wrap", "uncle", "badge"]);
    }

    #[test]
    fn body_styles_the_box_the_markup_is_drawn_into() {
        let (_, laid) = lay(
            "<style>body { padding: 20px; background: #2a6fb0; display: flex }\
             .box { width: 60px; height: 30px }</style><div class=box></div>",
            400.0,
            90.0,
        );
        // The size given is the box, so padding goes inside it rather than
        // pushing the root out to 440 by 130.
        assert_eq!(laid.size, (400.0, 90.0));
        // The root draws, because someone asked it to.
        let root = &laid.boxes[0];
        assert_eq!([root.rect[0], root.rect[1]], [0.0, 0.0]);
        assert_eq!([root.rect[2], root.rect[3]], [400.0, 90.0]);
        assert!(root.paint.background.is_some());
        // The child sits at the padding edge.
        let child = laid.boxes.iter().find(|b| b.rect[2] == 60.0).unwrap();
        assert_eq!([child.rect[0], child.rect[1]], [20.0, 20.0]);
    }

    #[test]
    fn an_unstyled_root_still_draws_nothing() {
        // Left alone the root marks no pixels, which is what keeps the
        // painted area down to the content.
        let (_, laid) = lay(
            "<style>.box { width: 60px; height: 30px }</style><div class=box></div>",
            400.0,
            90.0,
        );
        assert!(
            laid.boxes.iter().all(|b| b.rect[2] != 400.0),
            "the root should not be in the paint list"
        );
    }

    #[test]
    fn a_column_stacks_its_children() {
        let (_, laid) = lay(
            "<style>.card { display: flex; flex-direction: column; gap: 4px; padding: 10px }\
             </style><div class=card><p>ab</p><p>cd</p></div>",
            200.0,
            100.0,
        );
        // The card is a block child of the surface: it fills the width and
        // takes its height from its content, the way <body>'s child does.
        let card = &laid.boxes[0];
        assert_eq!(card.rect[2], 200.0);
        // The two text leaves sit under their paragraphs, 4px apart.
        let ps: Vec<&Painted> = laid
            .boxes
            .iter()
            .filter(|b| matches!(b.content, Content::Empty))
            .collect();
        assert_eq!(ps[1].rect[1], 10.0);
        assert!((ps[2].rect[1] - (10.0 + ps[1].rect[3] + 4.0)).abs() < 0.01);
    }

    #[test]
    fn centring_works_on_both_axes() {
        let (_, laid) = lay(
            "<style>.a { display: flex; justify-content: center; align-items: center; \
             height: 100% }.b { width: 40px; height: 20px }</style>\
             <div class=a><div class=b></div></div>",
            200.0,
            100.0,
        );
        let b = laid.boxes.iter().find(|x| x.rect[2] == 40.0).unwrap();
        assert_eq!(b.rect, [80.0, 40.0, 40.0, 20.0]);
    }

    #[test]
    fn padding_and_border_grow_a_content_box() {
        // CSS's default: width is the content, and padding and border are
        // added to it.
        let (_, laid) = lay(
            "<style>.a { width: 100px; height: 50px; padding: 8px; border: 2px solid red }\
             </style><div class=a></div>",
            200.0,
            100.0,
        );
        let a = &laid.boxes[0];
        assert_eq!(a.rect, [0.0, 0.0, 120.0, 70.0]);
        assert_eq!(a.border, [2.0, 2.0, 2.0, 2.0]);
        assert_eq!(a.content_rect, [10.0, 10.0, 100.0, 50.0]);
    }

    #[test]
    fn border_box_takes_the_width_as_written() {
        let (_, laid) = lay(
            "<style>.a { box-sizing: border-box; width: 100px; height: 50px; \
             padding: 8px; border: 2px solid red }</style><div class=a></div>",
            200.0,
            100.0,
        );
        let a = &laid.boxes[0];
        assert_eq!(a.rect, [0.0, 0.0, 100.0, 50.0]);
        assert_eq!(a.content_rect, [10.0, 10.0, 80.0, 30.0]);
    }

    #[test]
    fn absolute_positioning_uses_the_nearest_box() {
        let (_, laid) = lay(
            "<style>.a { position: relative; width: 200px; height: 100px }\
             .b { position: absolute; left: 10px; bottom: 5px; width: 30px; height: 15px }\
             </style><div class=a><div class=b></div></div>",
            200.0,
            100.0,
        );
        let b = laid.boxes.iter().find(|x| x.rect[2] == 30.0).unwrap();
        assert_eq!(b.rect, [10.0, 80.0, 30.0, 15.0]);
    }

    #[test]
    fn display_none_drops_the_subtree() {
        let (_, laid) = lay(
            "<style>.gone { display: none }</style><div><p class=gone>x</p><p>y</p></div>",
            100.0,
            50.0,
        );
        assert!(
            !laid
                .boxes
                .iter()
                .any(|b| matches!(&b.content, Content::Text { text, .. } if text == "x"))
        );
    }

    #[test]
    fn overflow_hidden_clips_descendants() {
        let (_, laid) = lay(
            "<style>.a { overflow: hidden; border-radius: 6px; width: 50px; height: 20px }\
             </style><div class=a><p>hello</p></div>",
            100.0,
            50.0,
        );
        let p = laid
            .boxes
            .iter()
            .find(|b| b.node != laid.boxes[0].node)
            .unwrap();
        let (rect, radius) = p.clip.unwrap();
        assert_eq!(rect, [0.0, 0.0, 50.0, 20.0]);
        assert_eq!(radius, [6.0; 4]);
    }

    #[test]
    fn an_image_keeps_its_aspect_when_one_side_is_given() {
        let (_, laid) = lay(
            "<style>img { width: 50px }</style><div><img src=x.png></div>",
            200.0,
            100.0,
        );
        let img = laid
            .boxes
            .iter()
            .find(|b| matches!(b.content, Content::Image { .. }))
            .unwrap();
        assert_eq!(img.rect[2], 50.0);
        assert_eq!(img.rect[3], 25.0);
    }

    #[test]
    fn opacity_opens_a_group_rather_than_multiplying_down() {
        let (_, laid) = lay(
            "<style>.a { opacity: 0.5 } .b { opacity: 0.5 } .c { animation: x 1s }</style>\
             <div class=a><div class=b><div class=c></div></div><div class=d></div></div>",
            100.0,
            50.0,
        );
        // Three groups: a, b inside a, c inside b. Each box carries its
        // innermost group and an opacity of one; the groups carry theirs.
        assert_eq!(laid.groups.len(), 3);
        assert_eq!(laid.groups[0].parent, None);
        assert_eq!(laid.groups[1].parent, Some(0));
        assert_eq!(laid.groups[2].parent, Some(1));
        assert!((laid.groups[0].opacity - 0.5).abs() < 1e-6);
        assert!((laid.groups[2].opacity - 1.0).abs() < 1e-6);
        for b in &laid.boxes {
            assert!((b.opacity - 1.0).abs() < 1e-6, "{b:?}");
        }
        let groups: Vec<Option<usize>> = laid.boxes.iter().map(|b| b.group).collect();
        // a's own box, then d in flow, then b (a context at z 0) with c.
        assert_eq!(groups, vec![Some(0), Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn a_group_paints_in_tree_order_with_positioned_boxes() {
        // The haze is a group (it has an animation); the vignette is a
        // positioned box after it. CSS paints both in the same layer, in
        // tree order, so the vignette lands on top of the haze.
        let (_, laid) = lay(
            "<style>.haze { position: absolute; animation: x 1s } .vignette { position: absolute }</style>\
             <div><div class=haze></div><div class=vignette></div></div>",
            100.0,
            50.0,
        );
        let order: Vec<Option<usize>> = laid.boxes.iter().map(|b| b.group).collect();
        assert_eq!(order, vec![None, Some(0), None]);
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(collapse("  a \n b  ", false), "a b");
        assert_eq!(collapse("  a \n b  ", true), "  a \n b  ");
    }
}
