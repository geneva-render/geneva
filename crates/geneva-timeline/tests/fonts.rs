//! The check on a font family the machine does not have.
//!
//! Text drawn in a family that is not installed still renders, in
//! whatever the shaper falls back to, so the only sign anything is wrong
//! is that the picture differs from one machine to the next. W405 is
//! what makes that visible before a render rather than after.

use geneva_timeline::{AssetInfo, Ratio, resolve_with};

/// Markup naming a family in its CSS, which is how a font is asked for
/// there: `font-family` takes a name, not an asset id.
fn markup_codes(css_family: &str, info: &dyn AssetInfo, assets: &str) -> Vec<String> {
    let html =
        format!("<style>.c {{ font-family: {css_family} }}</style><div class=\"c\">Hi</div>");
    let text = format!(
        r#"{{"geneva":"1.0","output":{{"width":320,"height":180,"fps":25,"duration":"1s"}},
        "assets":{{{assets}}},
        "layers":[{{"id":"m","clips":[{{"source":{{"kind":"html","html":{}}}}}]}}]}}"#,
        serde_json::to_string(&html).unwrap()
    );
    let timeline = geneva_timeline::parse(&text).unwrap();
    let (_, diagnostics) = resolve_with(&timeline, info);
    diagnostics.iter().map(|d| d.code.to_string()).collect()
}

/// A machine with one font on it, and nothing else to say.
struct OneFont(&'static str);

impl AssetInfo for OneFont {
    fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
        None
    }

    fn has_font_family(&self, family: &str) -> Option<bool> {
        Some(family == self.0)
    }
}

/// A caller with no font database at all, such as a validator running
/// somewhere other than the machine that will render.
struct NoFonts;

impl AssetInfo for NoFonts {
    fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
        None
    }
}

fn codes(font: &str, info: &dyn AssetInfo, assets: &str) -> Vec<String> {
    let text = format!(
        r#"{{"geneva":"1.0","output":{{"width":320,"height":180,"fps":25,"duration":"1s"}},
        "assets":{{{assets}}},
        "layers":[{{"id":"t","clips":[{{"source":{{"kind":"text","text":"Hello",
        "font":"{font}"}}}}]}}]}}"#
    );
    let timeline = geneva_timeline::parse(&text).unwrap();
    let (_, diagnostics) = resolve_with(&timeline, info);
    diagnostics.iter().map(|d| d.code.to_string()).collect()
}

#[test]
fn a_family_the_machine_does_not_have_is_reported() {
    let d = codes("Inter", &OneFont("DejaVu Sans"), "");
    assert!(d.contains(&"W405".to_owned()), "{d:?}");
}

#[test]
fn a_family_the_machine_has_is_not_reported() {
    let d = codes("DejaVu Sans", &OneFont("DejaVu Sans"), "");
    assert!(!d.contains(&"W405".to_owned()), "{d:?}");
}

/// The fix the warning suggests must not itself warn: a document that
/// carries the font names the asset id, and an asset id is not a family
/// the machine is expected to have.
#[test]
fn naming_a_font_asset_is_the_fix_and_stays_quiet() {
    let d = codes(
        "brand",
        &OneFont("DejaVu Sans"),
        r#""brand":{"src":"Brand.ttf","kind":"font"}"#,
    );
    assert!(!d.contains(&"W405".to_owned()), "{d:?}");
}

/// A caller that cannot see a font database says nothing rather than
/// guessing about a machine it is not running on.
#[test]
fn a_caller_with_no_font_database_stays_quiet() {
    let d = codes("Inter", &NoFonts, "");
    assert!(!d.contains(&"W405".to_owned()), "{d:?}");
}

/// The shorthand carries the family in its last part, and the check
/// reads the family the same way the renderer will.
#[test]
fn the_font_shorthand_is_checked_by_its_family() {
    let d = codes("600 40px/1.2 Inter", &OneFont("DejaVu Sans"), "");
    assert!(d.contains(&"W405".to_owned()), "{d:?}");
    let ok = codes("600 40px/1.2 DejaVu Sans", &OneFont("DejaVu Sans"), "");
    assert!(!ok.contains(&"W405".to_owned()), "{ok:?}");
}

/// The same check reaches families named inside markup CSS, which is
/// where most of them are named: a card written for a browser asks for
/// its font with `font-family`, never with an asset id.
#[test]
fn a_family_named_in_markup_css_is_reported() {
    let d = markup_codes("Nonesuch Sans", &OneFont("DejaVu Sans"), "");
    assert!(d.contains(&"W405".to_owned()), "{d:?}");
    let ok = markup_codes("DejaVu Sans", &OneFont("DejaVu Sans"), "");
    assert!(!ok.contains(&"W405".to_owned()), "{ok:?}");
}

/// A machine that carries the font as an asset and names it in CSS by
/// the family the file declares is doing exactly what the warning asks
/// for, so it must not be warned at. The caller reports that family as
/// present, which is what the CLI does after reading the file.
#[test]
fn markup_naming_a_carried_font_by_its_family_stays_quiet() {
    struct Carried;
    impl AssetInfo for Carried {
        fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
            None
        }
        // The machine has nothing; the family comes from the asset.
        fn has_font_family(&self, family: &str) -> Option<bool> {
            Some(family == "Brand Sans")
        }
    }
    let d = markup_codes(
        "Brand Sans",
        &Carried,
        r#""brand":{"src":"Brand.ttf","kind":"font"}"#,
    );
    assert!(!d.contains(&"W405".to_owned()), "{d:?}");
}
