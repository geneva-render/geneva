//! Behavioural tests for validation and resolution, driven by JSON text.

use geneva_timeline::{Diagnostic, Ratio, ResolvedSource, Severity, load};

fn codes(diags: &[Diagnostic]) -> Vec<(&'static str, String)> {
    diags.iter().map(|d| (d.code, d.path.clone())).collect()
}

fn errors(text: &str) -> Vec<(&'static str, String)> {
    let l = load(text);
    codes(
        &l.diagnostics
            .iter()
            .filter(|d| d.is_error())
            .cloned()
            .collect::<Vec<_>>(),
    )
}

const HEAD: &str = r#""geneva":"0.1","output":{"width":640,"height":360,"fps":30,"duration":"4s"}"#;

fn doc(body: &str) -> String {
    format!("{{{HEAD},{body}}}")
}

#[test]
fn examples_load_without_errors() {
    for name in ["solid", "shapes", "overlay"] {
        let text = std::fs::read_to_string(format!("../../examples/{name}.json")).unwrap();
        let l = load(&text);
        assert!(l.is_ok(), "{name}: {:#?}", l.diagnostics);
        assert!(l.composition.is_some());
    }
}

#[test]
fn sequential_clips_and_transitions_resolve_exactly() {
    let text = std::fs::read_to_string("../../examples/overlay.json").unwrap();
    let comp = load(&text).composition.unwrap();
    assert_eq!(comp.fps, Ratio::new(30000, 1001));
    let video = &comp.layers[0];
    assert_eq!(video.clips[0].start, Ratio::ZERO);
    assert_eq!(video.clips[0].end, Ratio::from_int(6));
    // The second clip starts half a second before the first ends.
    assert_eq!(video.clips[1].start, Ratio::new(11, 2));
    assert_eq!(video.clips[1].end, Ratio::new(19, 2));
    assert!(video.clips[1].transition_in.is_some());
    // Duration is the last end across visual layers and audio.
    assert_eq!(comp.duration, Ratio::new(19, 2));
    assert_eq!(
        comp.frame_count(),
        (Ratio::new(19, 2) * Ratio::new(30000, 1001)).ceil() as u64
    );
    match &video.clips[0].source {
        ResolvedSource::Video { in_, audio, .. } => {
            assert_eq!(*in_, Ratio::from_int(2));
            assert!(*audio);
        }
        other => panic!("unexpected source {other:?}"),
    }
}

#[test]
fn open_ended_clips_run_to_the_output_end() {
    let text = std::fs::read_to_string("../../examples/solid.json").unwrap();
    let comp = load(&text).composition.unwrap();
    assert_eq!(comp.layers[0].clips[0].end, Ratio::from_int(2));
    assert_eq!(comp.clips_at(Ratio::new(1, 2)).count(), 1);
    assert_eq!(comp.clips_at(Ratio::from_int(2)).count(), 0);
}

#[test]
fn unsupported_version() {
    let text = r#"{"geneva":"0.9","output":{"width":640,"height":360,"fps":30,"duration":1}}"#;
    assert_eq!(errors(text), vec![("E110", "/geneva".to_owned())]);
}

#[test]
fn unknown_asset_suggests_a_name() {
    let text = doc(
        r#""assets":{"logo":{"src":"logo.png"}},"layers":[{"clips":[{"source":{"kind":"image","asset":"lgoo"}}]}]"#,
    );
    let l = load(&text);
    let e = l.diagnostics.iter().find(|d| d.code == "E200").unwrap();
    assert_eq!(e.path, "/layers/0/clips/0/source/asset");
    assert!(e.help.as_deref().unwrap().contains("\"logo\""));
}

#[test]
fn asset_kind_mismatch_and_inference() {
    let text = doc(
        r#""assets":{"clip":{"src":"a.mp4"},"odd":{"src":"thing.bin"}},"layers":[{"clips":[{"source":{"kind":"image","asset":"clip"}}]}]"#,
    );
    let e = errors(&text);
    assert!(e.contains(&("E201", "/layers/0/clips/0/source/asset".to_owned())));
    assert!(e.contains(&("E203", "/assets/odd/kind".to_owned())));
}

#[test]
fn asset_paths_must_stay_under_the_root() {
    let text = doc(r#""assets":{"a":{"src":"../secret.png"},"b":{"src":"/etc/x.png"}}"#);
    let e = errors(&text);
    assert!(e.contains(&("E202", "/assets/a/src".to_owned())));
    assert!(e.contains(&("E202", "/assets/b/src".to_owned())));
}

#[test]
fn overlapping_clips_are_rejected_with_a_fix() {
    let text = doc(r#""layers":[{"clips":[
        {"source":{"kind":"solid","color":"red"},"duration":"2s"},
        {"source":{"kind":"solid","color":"blue"},"start":"1s","duration":"1s"}]}]"#);
    let l = load(&text);
    let e = l.diagnostics.iter().find(|d| d.code == "E302").unwrap();
    assert_eq!(e.path, "/layers/0/clips/1/start");
    assert!(e.help.as_deref().unwrap().contains("2s"));
}

#[test]
fn transition_overlap_is_allowed_but_must_be_covered() {
    let ok = doc(r#""layers":[{"clips":[
        {"source":{"kind":"solid","color":"red"},"duration":"2s"},
        {"source":{"kind":"solid","color":"blue"},"duration":"1s","transition":{"kind":"crossfade","duration":"0.5s"}}]}]"#);
    assert!(load(&ok).is_ok());
    let comp = load(&ok).composition.unwrap();
    assert_eq!(comp.layers[0].clips[1].start, Ratio::new(3, 2));

    let too_long = doc(r#""layers":[{"clips":[
        {"source":{"kind":"solid","color":"red"},"duration":"0.4s"},
        {"source":{"kind":"solid","color":"blue"},"duration":"1s","transition":{"kind":"crossfade","duration":"0.5s"}}]}]"#);
    assert!(errors(&too_long).iter().any(|(c, _)| *c == "E306"));
}

#[test]
fn keyframe_order_range_and_easing_are_checked() {
    let text = doc(
        r#""layers":[{"clips":[{"source":{"kind":"solid","color":"red"},
        "opacity":{"keyframes":[{"t":0,"v":0},{"t":"1s","v":2,"ease":{"cubic-bezier":[2,0,0,1]}},{"t":"0.5s","v":1}]}}]}]"#,
    );
    let e = errors(&text);
    assert!(e.contains(&("E303", "/layers/0/clips/0/opacity/keyframes/2/t".to_owned())));
    assert!(e.contains(&("E402", "/layers/0/clips/0/opacity/keyframes/1/v".to_owned())));
    assert!(e.contains(&(
        "E401",
        "/layers/0/clips/0/opacity/keyframes/1/ease".to_owned()
    )));
}

#[test]
fn keyframes_past_the_clip_end_warn() {
    let text = doc(
        r#""layers":[{"clips":[{"source":{"kind":"solid","color":"red"},"duration":"1s",
        "opacity":{"keyframes":[{"t":0,"v":0},{"t":"5s","v":1}]}}]}]"#,
    );
    let l = load(&text);
    assert!(l.is_ok());
    assert!(
        l.diagnostics
            .iter()
            .any(|d| d.code == "W300" && d.severity == Severity::Warning)
    );
}

#[test]
fn duration_must_be_determinable() {
    let text = r#"{"geneva":"0.1","output":{"width":640,"height":360,"fps":30},
        "layers":[{"clips":[{"source":{"kind":"solid","color":"red"}}]}]}"#;
    let e = errors(text);
    assert!(e.contains(&("E305", "/layers/0/clips/0".to_owned())));
}

#[test]
fn video_ranges_are_checked() {
    let text = doc(r#""assets":{"v":{"src":"v.mp4"}},"layers":[{"clips":[
        {"source":{"kind":"video","asset":"v","in":"5s","out":"3s"}},
        {"source":{"kind":"video","asset":"v","in":"0s","out":"1s"},"duration":"2s"}]}]"#);
    let e = errors(&text);
    assert!(e.contains(&("E301", "/layers/0/clips/0/source/out".to_owned())));
    assert!(e.contains(&("E301", "/layers/0/clips/1/duration".to_owned())));
}

#[test]
fn clips_past_the_output_are_cut_with_a_warning() {
    let text =
        doc(r#""layers":[{"clips":[{"source":{"kind":"solid","color":"red"},"duration":"10s"}]}]"#);
    let l = load(&text);
    assert!(l.is_ok());
    assert!(l.diagnostics.iter().any(|d| d.code == "W301"));
    assert_eq!(
        l.composition.unwrap().layers[0].clips[0].end,
        Ratio::from_int(4)
    );
}

#[test]
fn text_needs_content_and_ordered_words() {
    let text = doc(r#""layers":[{"clips":[
        {"source":{"kind":"text"}},
        {"source":{"kind":"text","words":[{"text":"b","start":"1s","end":"2s"},{"text":"a","start":"0.5s","end":"1.5s"}]}}]}]"#);
    let e = errors(&text);
    assert!(e.contains(&("E405", "/layers/0/clips/0/source".to_owned())));
    assert!(e.contains(&("E411", "/layers/0/clips/1/source/words/1/start".to_owned())));
}

#[test]
fn odd_dimensions_and_unused_assets_are_reported_softly() {
    let text = r#"{"geneva":"0.1","output":{"width":641,"height":360,"fps":30,"duration":1},
        "assets":{"x":{"src":"x.png"}}}"#;
    let l = load(text);
    assert!(l.is_ok());
    assert!(
        l.diagnostics
            .iter()
            .any(|d| d.code == "W401" && d.path == "/output/width")
    );
    assert!(
        l.diagnostics
            .iter()
            .any(|d| d.code == "W201" && d.path == "/assets/x")
    );
}

#[test]
fn hdr_output_is_refused() {
    let text = r#"{"geneva":"0.1","output":{"width":640,"height":360,"fps":30,"duration":1,"color":{"transfer":"pq"}}}"#;
    assert!(errors(text).iter().any(|(c, _)| *c == "E420"));
}

#[test]
fn diagnostics_serialize_for_machines() {
    let text =
        doc(r#""layers":[{"clips":[{"source":{"kind":"solid","color":"red"},"opacity":3}]}]"#);
    let l = load(&text);
    let json = serde_json::to_value(&l.diagnostics).unwrap();
    let first = &json[0];
    assert_eq!(first["severity"], "error");
    assert_eq!(first["code"], "E402");
    assert_eq!(first["path"], "/layers/0/clips/0/opacity");
    assert_eq!(first["value"], 3.0);
}

#[test]
fn compositions_resolve_relative_to_their_clip_and_can_be_reused() {
    let text = doc(
        r#""compositions":{"badge":{"width":200,"height":100,"layers":[
            {"clips":[{"source":{"kind":"shape","shape":"rect","width":"100%","height":"50%","fill":"red"},"duration":"1s"}]}]}},
        "layers":[{"clips":[
            {"source":{"kind":"composition","composition":"badge"},"start":"1s"},
            {"source":{"kind":"composition","composition":"badge"},"start":"3s","duration":"0.5s"}]}]"#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:#?}", l.diagnostics);
    let comp = l.composition.unwrap();
    let clips = &comp.layers[0].clips;
    // Natural length of the composition (its inner clip lasts 1s).
    assert_eq!(clips[0].start, Ratio::from_int(1));
    assert_eq!(clips[0].end, Ratio::from_int(2));
    // An explicit duration wins.
    assert_eq!(clips[1].end, Ratio::new(7, 2));
    match &clips[0].source {
        ResolvedSource::Composition(c) => {
            assert_eq!((c.width, c.height), (200, 100));
            let inner = &c.layers[0].clips[0];
            assert_eq!(inner.start, Ratio::ZERO);
            assert_eq!(inner.end, Ratio::from_int(1));
            // Percentages inside refer to the composition's own frame.
            match &inner.source {
                ResolvedSource::Shape { width, height, .. } => {
                    assert_eq!((*width, *height), (200.0, 50.0));
                }
                other => panic!("{other:?}"),
            }
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn open_ended_compositions_take_the_clip_length() {
    let text = doc(r#""compositions":{"bg":{"width":10,"height":10,"layers":[
            {"clips":[{"source":{"kind":"solid","color":"red"}}]}]}},
        "layers":[{"clips":[{"source":{"kind":"composition","composition":"bg"},"start":"1s"}]}]"#);
    let l = load(&text);
    assert!(l.is_ok(), "{:#?}", l.diagnostics);
    let comp = l.composition.unwrap();
    let clip = &comp.layers[0].clips[0];
    assert_eq!(clip.end, Ratio::from_int(4));
    match &clip.source {
        ResolvedSource::Composition(c) => assert_eq!(c.layers[0].clips[0].end, Ratio::from_int(3)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_and_cyclic_compositions_are_errors() {
    let text = doc(r#""compositions":{
            "a":{"width":10,"height":10,"layers":[{"clips":[{"source":{"kind":"composition","composition":"b"}}]}]},
            "b":{"width":10,"height":10,"layers":[{"clips":[{"source":{"kind":"composition","composition":"a"}}]}]}},
        "layers":[{"clips":[
            {"source":{"kind":"composition","composition":"a"},"duration":"1s"},
            {"source":{"kind":"composition","composition":"bdge"},"duration":"1s"}]}]"#);
    let e = errors(&text);
    assert!(
        e.iter().any(|(c, p)| *c == "E207"
            && p.starts_with("/compositions/b/layers/0/clips/0/source/composition")),
        "{e:?}"
    );
    assert!(e.contains(&("E206", "/layers/0/clips/1/source/composition".to_owned())));
}
