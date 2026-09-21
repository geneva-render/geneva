//! The check on a font family the machine does not have.
//!
//! Text drawn in a family that is not installed still renders, in
//! whatever the shaper falls back to, so the only sign anything is wrong
//! is that the picture differs from one machine to the next. W405 is
//! what makes that visible before a render rather than after.

use geneva_timeline::{AssetInfo, Ratio, resolve_with};

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
        r#"{{"geneva":"0.3","output":{{"width":320,"height":180,"fps":25,"duration":"1s"}},
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
