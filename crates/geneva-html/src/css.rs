//! A CSS subset: type, class and id selectors, the descendant and child
//! combinators, and the usual cascade.
//!
//! There is no `@media`, no pseudo-class and no shorthand this crate does
//! not name. What matters for the cascade is here: specificity as
//! (ids, classes, types), source order to break ties, `!important`, and
//! inheritance for the properties that inherit.

use std::collections::BTreeMap;
use std::fmt;

use crate::dom::{Document, Element, NodeId};

/// One compound selector: an optional tag with any number of classes and
/// at most one id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compound {
    /// Tag name, or `None` for `*`.
    pub tag: Option<String>,
    /// The `#id` part.
    pub id: Option<String>,
    /// Every `.class` part.
    pub classes: Vec<String>,
}

impl Compound {
    fn matches(&self, el: &Element) -> bool {
        if let Some(tag) = &self.tag {
            // The wrapper around the markup answers to the names a page
            // would give it, so `body { ... }` reaches the whole box the
            // way it does in a browser.
            let named =
                *tag == el.tag || (el.tag == ":root" && matches!(tag.as_str(), "html" | "body"));
            if !named {
                return false;
            }
        }
        if let Some(id) = &self.id {
            if el.id.as_ref() != Some(id) {
                return false;
            }
        }
        self.classes.iter().all(|c| el.classes.contains(c))
    }
}

/// How a compound selector relates to the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// Any ancestor.
    Descendant,
    /// The immediate parent.
    Child,
}

/// A complex selector, written left to right; the last compound matches
/// the element itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    /// The rightmost compound.
    pub subject: Compound,
    /// Ancestor compounds, nearest first, each with how it is reached.
    pub ancestors: Vec<(Combinator, Compound)>,
}

impl Compound {
    /// The compound written out again, close enough to what was typed to
    /// name it in a message.
    fn text(&self) -> String {
        let mut out = self.tag.clone().unwrap_or_else(|| {
            if self.id.is_none() && self.classes.is_empty() {
                "*".to_owned()
            } else {
                String::new()
            }
        });
        if let Some(id) = &self.id {
            out.push('#');
            out.push_str(id);
        }
        for c in &self.classes {
            out.push('.');
            out.push_str(c);
        }
        out
    }
}

impl std::fmt::Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ancestors are held nearest first, so they read right to left.
        for (combinator, compound) in self.ancestors.iter().rev() {
            f.write_str(&compound.text())?;
            f.write_str(match combinator {
                Combinator::Child => " > ",
                Combinator::Descendant => " ",
            })?;
        }
        f.write_str(&self.subject.text())
    }
}

impl Selector {
    /// Specificity as (ids, classes, types), compared lexicographically.
    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut s = (0, 0, 0);
        for c in std::iter::once(&self.subject).chain(self.ancestors.iter().map(|(_, c)| c)) {
            s.0 += u32::from(c.id.is_some());
            s.1 += u32::try_from(c.classes.len()).unwrap_or(u32::MAX);
            s.2 += u32::from(c.tag.is_some());
        }
        s
    }

    /// Whether the selector matches `node` in `doc`.
    pub fn matches(&self, doc: &Document, node: NodeId) -> bool {
        let Some(el) = doc.nodes[node].element() else {
            return false;
        };
        if !self.subject.matches(el) {
            return false;
        }
        let mut current = doc.nodes[node].parent;
        for (combinator, compound) in &self.ancestors {
            match combinator {
                Combinator::Child => {
                    let Some(p) = current else { return false };
                    let Some(el) = doc.nodes[p].element() else {
                        return false;
                    };
                    if !compound.matches(el) {
                        return false;
                    }
                    current = doc.nodes[p].parent;
                }
                Combinator::Descendant => {
                    let mut found = None;
                    let mut cur = current;
                    while let Some(p) = cur {
                        if doc.nodes[p].element().is_some_and(|e| compound.matches(e)) {
                            found = Some(p);
                            break;
                        }
                        cur = doc.nodes[p].parent;
                    }
                    let Some(p) = found else { return false };
                    current = doc.nodes[p].parent;
                }
            }
        }
        true
    }
}

/// One declaration of a rule or a `style` attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// Lowercased property name.
    pub property: String,
    /// The value, as written.
    pub value: String,
    /// Whether it carried `!important`.
    pub important: bool,
}

/// A rule: selectors and the declarations they apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The rule's selector list.
    pub selectors: Vec<Selector>,
    /// Its declarations, in source order.
    pub declarations: Vec<Declaration>,
}

/// One `@keyframes` rule: offsets to declaration blocks, as written.
/// This is the shape a timeline's own `keyframes` map has, so markup and
/// document feed the same animation.
pub type KeyframesRule = BTreeMap<String, String>;

/// A parsed stylesheet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stylesheet {
    /// Rules in source order.
    pub rules: Vec<Rule>,
    /// `@keyframes` rules by name. Nothing here interprets them: they are
    /// handed on to whatever plays the animation.
    pub keyframes: BTreeMap<String, KeyframesRule>,
}

/// Why a stylesheet did not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssError {
    /// What is wrong, in one line.
    pub message: String,
    /// One-based line in the source.
    pub line: usize,
}

impl fmt::Display for CssError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (line {})", self.message, self.line)
    }
}

/// Parses a stylesheet. At-rules are refused by name rather than skipped,
/// so nothing silently does nothing.
pub fn parse_stylesheet(source: &str) -> Result<Stylesheet, CssError> {
    let source = strip_comments(source);
    let mut rules = Vec::new();
    let mut keyframes: BTreeMap<String, KeyframesRule> = BTreeMap::new();
    let mut rest = source.as_str();
    let mut consumed = 0usize;
    while !rest.trim().is_empty() {
        let skipped = rest.len() - rest.trim_start().len();
        consumed += skipped;
        rest = rest.trim_start();
        let line = || source[..consumed.min(source.len())].matches('\n').count() + 1;
        if let Some(at) = rest.strip_prefix('@') {
            let name: String = at
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if name != "keyframes" {
                return Err(CssError {
                    message: format!("@{name} is not supported"),
                    line: line(),
                });
            }
            let (rule_name, rule, used) =
                parse_keyframes(&rest[1 + name.len()..]).map_err(|message| CssError {
                    message,
                    line: line(),
                })?;
            keyframes.insert(rule_name, rule);
            let used = 1 + name.len() + used;
            consumed += used;
            rest = &rest[used..];
            continue;
        }
        let Some(open) = rest.find('{') else {
            return Err(CssError {
                message: "a rule has no declaration block".to_owned(),
                line: line(),
            });
        };
        let Some(close) = rest.find('}') else {
            return Err(CssError {
                message: "a declaration block is never closed".to_owned(),
                line: line(),
            });
        };
        if close < open {
            return Err(CssError {
                message: "a stray \"}\"".to_owned(),
                line: line(),
            });
        }
        let prelude = &rest[..open];
        let body = &rest[open + 1..close];
        let mut selectors = Vec::new();
        for part in prelude.split(',') {
            if part.trim().is_empty() {
                return Err(CssError {
                    message: format!("an empty selector in {:?}", prelude.trim()),
                    line: line(),
                });
            }
            selectors.push(parse_selector(part).map_err(|message| CssError {
                message,
                line: line(),
            })?);
        }
        rules.push(Rule {
            selectors,
            declarations: parse_declarations(body),
        });
        consumed += close + 1;
        rest = &rest[close + 1..];
    }
    Ok(Stylesheet { rules, keyframes })
}

/// Parses the name and body of a `@keyframes` rule, given the text just
/// after the at-keyword. Returns how much of it was consumed.
fn parse_keyframes(text: &str) -> Result<(String, KeyframesRule, usize), String> {
    let open = text
        .find('{')
        .ok_or_else(|| "@keyframes has no block".to_owned())?;
    let name = text[..open].trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{name:?} is not a keyframes name"));
    }
    let close = matching_brace(&text[open..])
        .ok_or_else(|| format!("@keyframes {name} is never closed"))?;
    let body = &text[open + 1..open + close];

    let mut rule = KeyframesRule::new();
    let mut rest = body;
    while !rest.trim().is_empty() {
        rest = rest.trim_start();
        let Some(o) = rest.find('{') else {
            return Err(format!("a keyframe in @keyframes {name} has no block"));
        };
        let c = matching_brace(&rest[o..])
            .ok_or_else(|| format!("a keyframe in @keyframes {name} is never closed"))?;
        let offsets = &rest[..o];
        let block = rest[o + 1..o + c].trim().to_owned();
        for offset in offsets.split(',') {
            let offset = offset.trim();
            if offset.is_empty() {
                return Err(format!("an empty keyframe offset in @keyframes {name}"));
            }
            // Two blocks at one offset merge, as a browser merges them.
            rule.entry(offset.to_ascii_lowercase())
                .and_modify(|existing| {
                    existing.push(';');
                    existing.push_str(&block);
                })
                .or_insert_with(|| block.clone());
        }
        rest = &rest[o + c + 1..];
    }
    Ok((name.to_owned(), rule, open + close + 1))
}

/// The index of the `}` matching the `{` at the start of `text`.
fn matching_brace(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parses a `style` attribute or a rule body.
pub fn parse_declarations(body: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for decl in split_top_level(body, ';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((property, value)) = decl.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let (value, important) = match value.strip_suffix("!important") {
            Some(v) => (v.trim_end(), true),
            None => (value, false),
        };
        out.push(Declaration {
            property: property.trim().to_ascii_lowercase(),
            value: value.to_owned(),
            important,
        });
    }
    out
}

fn parse_selector(text: &str) -> Result<Selector, String> {
    let mut compounds: Vec<(Combinator, Compound)> = Vec::new();
    let mut combinator = Combinator::Descendant;
    for token in text
        .replace('>', " > ")
        .split_whitespace()
        .map(str::to_owned)
    {
        if token == ">" {
            combinator = Combinator::Child;
            continue;
        }
        compounds.push((combinator, parse_compound(&token)?));
        combinator = Combinator::Descendant;
    }
    let Some((joined_by, subject)) = compounds.pop() else {
        return Err(format!("an empty selector in {:?}", text.trim()));
    };
    // A compound carries the combinator that joins it to the one on its
    // left; matching walks outward, so each ancestor takes the combinator
    // recorded on the compound to its right.
    let mut ancestors: Vec<(Combinator, Compound)> = Vec::new();
    let mut next = joined_by;
    while let Some((combinator, compound)) = compounds.pop() {
        ancestors.push((next, compound));
        next = combinator;
    }
    Ok(Selector { subject, ancestors })
}

fn parse_compound(token: &str) -> Result<Compound, String> {
    let mut c = Compound::default();
    let mut rest = token;
    if !rest.starts_with(['.', '#']) {
        let end = rest.find(['.', '#']).unwrap_or(rest.len());
        let tag = &rest[..end];
        if tag != "*" {
            if !tag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!("{token:?} is not a selector geneva understands"));
            }
            c.tag = Some(tag.to_ascii_lowercase());
        }
        rest = &rest[end..];
    }
    while !rest.is_empty() {
        let kind = rest.as_bytes()[0];
        let end = rest[1..].find(['.', '#']).map_or(rest.len(), |i| i + 1);
        let name = &rest[1..end];
        if name.is_empty() {
            return Err(format!("{token:?} has an empty class or id"));
        }
        match kind {
            b'.' => c.classes.push(name.to_owned()),
            b'#' => {
                if c.id.is_some() {
                    return Err(format!("{token:?} names two ids"));
                }
                c.id = Some(name.to_owned());
            }
            _ => return Err(format!("{token:?} is not a selector geneva understands")),
        }
        rest = &rest[end..];
    }
    Ok(c)
}

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            // Keep the newlines so reported line numbers stay right.
            Some(j) => {
                let skipped = &rest[i..i + 2 + j + 2];
                out.extend(std::iter::repeat_n('\n', skipped.matches('\n').count()));
                rest = &rest[i + 2 + j + 2..];
            }
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Splits on a separator outside parentheses and quotes.
fn split_top_level(text: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                c if c == sep && depth == 0 => {
                    out.push(&text[start..i]);
                    start = i + c.len_utf8();
                }
                _ => {}
            },
        }
    }
    out.push(&text[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_and_specificity() {
        let s =
            parse_stylesheet(".card h1, #t { color: red; font-size: 12px !important }").unwrap();
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors.len(), 2);
        assert_eq!(s.rules[0].selectors[0].specificity(), (0, 1, 1));
        assert_eq!(s.rules[0].selectors[1].specificity(), (1, 0, 0));
        assert_eq!(s.rules[0].declarations.len(), 2);
        assert!(s.rules[0].declarations[1].important);
        assert_eq!(s.rules[0].declarations[0].value, "red");
    }

    #[test]
    fn matches_descendants_and_children() {
        let doc = crate::dom::parse("<div class=card><p><b id=x>hi</b></p></div>").unwrap();
        let div = doc.children(doc.root)[0];
        let p = doc.children(div)[0];
        let b = doc.children(p)[0];

        let sel =
            |s: &str| parse_stylesheet(&format!("{s} {{}}")).unwrap().rules[0].selectors[0].clone();
        assert!(sel(".card b").matches(&doc, b));
        assert!(sel(".card > p").matches(&doc, p));
        assert!(!sel(".card > b").matches(&doc, b));
        assert!(sel("div p > b#x").matches(&doc, b));
        assert!(!sel("p div b").matches(&doc, b));
        assert!(sel("*").matches(&doc, b));
    }

    #[test]
    fn comments_keep_the_line_count() {
        let err = parse_stylesheet("a {}\n/* two\nlines */\n@media print { }").unwrap_err();
        assert!(err.message.contains("@media"), "{err}");
        assert_eq!(err.line, 4);
    }

    #[test]
    fn parses_keyframes_rules() {
        let s = parse_stylesheet(
            "a { color: red }\n\
             @keyframes slide-in { from { transform: translateX(-10px) } to { transform: none } }\n\
             @keyframes pulse { from, to { opacity: 1 } 50% { opacity: 0.4 } }\n\
             b { color: blue }",
        )
        .unwrap();
        assert_eq!(s.rules.len(), 2);
        assert_eq!(s.keyframes.len(), 2);
        let slide = &s.keyframes["slide-in"];
        assert_eq!(slide["from"], "transform: translateX(-10px)");
        assert_eq!(slide["to"], "transform: none");
        let pulse = &s.keyframes["pulse"];
        assert_eq!(pulse["from"], "opacity: 1");
        assert_eq!(pulse["to"], "opacity: 1");
        assert_eq!(pulse["50%"], "opacity: 0.4");
    }

    #[test]
    fn a_broken_keyframes_rule_says_so() {
        assert!(parse_stylesheet("@keyframes { from {} }").is_err());
        assert!(parse_stylesheet("@keyframes a { from { opacity: 1 }").is_err());
        assert!(parse_stylesheet("@keyframes a { opacity: 1 }").is_err());
        // Other at-rules are still refused by name.
        assert!(
            parse_stylesheet("@media print { a {} }")
                .unwrap_err()
                .message
                .contains("@media")
        );
    }

    #[test]
    fn refuses_what_it_cannot_do() {
        assert!(parse_stylesheet("a:hover {}").is_err());
        assert!(parse_stylesheet("a + b {}").is_err());
        assert!(parse_stylesheet("a { color: red").is_err());
        assert!(parse_stylesheet("@import url(x);").is_err());
    }
}
