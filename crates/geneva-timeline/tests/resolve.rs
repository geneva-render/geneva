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
fn speed_shortens_a_clip_and_must_be_positive() {
    // Two seconds of source at speed 2 last one second; a nested
    // composition at speed 0.5 lasts twice its own length.
    let text = doc(r#""assets":{"v":{"src":"v.mp4"}},
        "compositions":{"c":{"width":64,"height":64,"layers":[{"clips":[{"source":{"kind":"solid","color":"red"},"duration":"1s"}]}]}},
        "layers":[{"clips":[
          {"source":{"kind":"video","asset":"v","out":"2s"},"speed":2},
          {"source":{"kind":"composition","composition":"c"},"speed":0.5}]}],
        "audio":[{"clips":[{"asset":"v","out":"3s","speed":1.5}]}]"#);
    let l = load(&text);
    assert!(l.is_ok(), "{:#?}", l.diagnostics);
    let comp = l.composition.unwrap();
    let clips = &comp.layers[0].clips;
    assert_eq!(clips[0].end, Ratio::from_int(1));
    assert_eq!(clips[0].speed, Ratio::from_int(2));
    assert_eq!(clips[1].start, Ratio::from_int(1));
    assert_eq!(clips[1].end, Ratio::from_int(3));
    assert_eq!(comp.audio[0].clips[0].end, Ratio::from_int(2));
    assert_eq!(comp.audio[0].clips[0].speed, Ratio::new(3, 2));
    let bad = errors(&doc(
        r#""assets":{"v":{"src":"v.mp4"}},"layers":[{"clips":[{"source":{"kind":"video","asset":"v","out":"2s"},"speed":0}]}]"#,
    ));
    assert_eq!(bad, vec![("E402", "/layers/0/clips/0/speed".to_owned())]);
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
fn words_in_list_form_run_to_the_next_word() {
    let text = doc(r#""layers":[{"clips":[{"source":{"kind":"text",
        "words":[["Dragon",0],["is","0.5s"],["captured","0.8s","1.5s"]]}}]}]"#);
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let clip = &l.composition.unwrap().layers[0].clips[0];
    let ResolvedSource::Text(t) = &clip.source else {
        panic!("expected a text source");
    };
    let ends: Vec<String> = t.words.iter().map(|w| w.2.to_string()).collect();
    assert_eq!(ends, ["0.5", "0.8", "1.5"]);
}

#[test]
fn the_last_word_needs_its_own_end() {
    let text = doc(r#""layers":[{"clips":[{"source":{"kind":"text",
        "words":[["Dragon",0],["is","0.5s"]]}}]}]"#);
    let e = errors(&text);
    assert!(e.contains(&("E102", "/layers/0/clips/0/source/words/1/end".to_owned())));
}

fn animated(rules: &str, clip: &str) -> String {
    format!(
        "{{{HEAD},\"keyframes\":{{{rules}}},\"layers\":[{{\"clips\":[{{\"source\":{{\"kind\":\"solid\",\"color\":\"red\"}},\"duration\":\"2s\",{clip}}}]}}]}}"
    )
}

#[test]
fn a_css_animation_becomes_a_keyframe_track() {
    let text = animated(
        r#""slide":{"from":"translate: -100px","to":"translate: 0"}"#,
        r#""transform":{"position":"50 50"},"animation":"slide 0.5s ease-out""#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let clip = &l.composition.unwrap().layers[0].clips[0];
    assert_eq!(clip.position.sample(0.0), [-50.0, 50.0]);
    assert_eq!(clip.position.sample(0.5), [50.0, 50.0]);
    assert_eq!(clip.position.sample(1.9), [50.0, 50.0]);
}

#[test]
fn two_animations_share_one_property_over_disjoint_ranges() {
    let text = animated(
        r#""in":{"from":"opacity: 0","to":"opacity: 1"},"out":{"from":"opacity: 1","to":"opacity: 0"}"#,
        r#""animation":"in 0.2s, out 0.2s 1.8s""#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let o = &l.composition.unwrap().layers[0].clips[0].opacity;
    assert_eq!(o.sample(0.0), 0.0);
    assert_eq!(o.sample(0.2), 1.0);
    assert_eq!(o.sample(1.0), 1.0);
    assert_eq!(o.sample(2.0), 0.0);
}

#[test]
fn an_infinite_animation_fills_the_clip() {
    let text = animated(
        r#""spin":{"from":"rotate: 0deg","to":"rotate: 360deg"}"#,
        r#""animation":"spin 0.5s infinite""#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let r = &l.composition.unwrap().layers[0].clips[0].rotation;
    // Four runs of half a second each cover the two-second clip; each run
    // ends at 360deg and the next snaps back to 0.
    assert_eq!(r.keyframes().len(), 8);
    assert!((r.sample(1.75) - 180.0).abs() < 0.01, "{}", r.sample(1.75));
    assert!(r.sample(0.49) > 350.0);
    assert!(r.sample(0.51) < 30.0);
}

#[test]
fn animation_problems_are_reported_where_they_are_written() {
    let bad = [
        (
            "E440",
            "/layers/0/clips/0/animation",
            r#""slide":{"from":"opacity: 0","to":"opacity: 1"}"#,
            r#""animation":"nope 1s""#,
        ),
        (
            "E441",
            "/layers/0/clips/0/animation",
            r#""slide":{"from":"opacity: 0","to":"opacity: 1"}"#,
            r#""animation":"slide""#,
        ),
        (
            "E442",
            "/keyframes/slide/from",
            r#""slide":{"from":"colour: red","to":"opacity: 1"}"#,
            r#""animation":"slide 1s""#,
        ),
        (
            "E443",
            "/layers/0/clips/0/opacity",
            r#""slide":{"from":"opacity: 0","to":"opacity: 1"}"#,
            r#""opacity":{"keyframes":[[0,0],["1s",1]]},"animation":"slide 1s""#,
        ),
        (
            "E444",
            "/layers/0/clips/0/animation",
            r#""a":{"from":"opacity: 0","to":"opacity: 1"},"b":{"from":"opacity: 1","to":"opacity: 0"}"#,
            r#""animation":"a 1s, b 1s""#,
        ),
    ];
    for (code, path, rules, clip) in bad {
        let e = errors(&animated(rules, clip));
        assert!(
            e.contains(&(code, path.to_owned())),
            "{code} at {path} missing from {e:?}"
        );
    }
}

#[test]
fn a_rule_nothing_plays_is_a_note() {
    let text = animated(
        r#""slide":{"from":"opacity: 0","to":"opacity: 1"}"#,
        r#""id":"x""#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    assert!(
        l.diagnostics
            .iter()
            .any(|d| d.code == "W203" && d.path == "/keyframes/slide")
    );
}

#[test]
fn markup_carries_its_own_animation() {
    let text = doc(
        r#""layers":[{"clips":[{"duration":"2s","source":{"kind":"html","width":100,
        "html":"<style>@keyframes slide { from { translate: -50px } to { translate: 0 } } .c { animation: slide 0.5s; height: 10px; background: red }</style><div class='c'></div>"},
        "transform":{"position":"50 50"}}]}]"#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let clip = &l.composition.unwrap().layers[0].clips[0];
    assert_eq!(clip.position.sample(0.0), [0.0, 50.0]);
    assert_eq!(clip.position.sample(0.5), [50.0, 50.0]);
}

#[test]
fn a_percentage_translate_is_a_share_of_the_animated_element() {
    let text = doc(
        r#""layers":[{"clips":[{"duration":"2s","source":{"kind":"html","width":1000,"height":100,
        "html":"<style>@keyframes slide { from { translate: -100% } to { translate: 0 } } .c { animation: slide 1s linear; width: 200px; height: 50px; background: red }</style><div class='c'></div>"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let clip = &l.composition.unwrap().layers[0].clips[0];
    // 100% is the element's 200px, not the 1000px surface it sits on.
    assert_eq!(clip.position.sample(0.0), [-200.0, 0.0]);
    assert_eq!(clip.position.sample(0.5), [-100.0, 0.0]);
    assert_eq!(clip.position.sample(1.0), [0.0, 0.0]);
}

#[test]
fn a_percentage_needs_a_box_to_be_a_share_of() {
    // A text clip is whatever size its content turns out to be.
    let text = format!(
        "{{{HEAD},\"keyframes\":{{\"slide\":{{\"from\":\"translate: -100%\",\"to\":\"translate: 0\"}}}},         \"layers\":[{{\"clips\":[{{\"duration\":\"2s\",\"source\":{{\"kind\":\"text\",\"text\":\"hi\"}},         \"animation\":\"slide 1s\"}}]}}]}}"
    );
    let e = errors(&text);
    assert!(
        e.contains(&("E442", "/layers/0/clips/0/animation".to_owned())),
        "{e:?}"
    );
}

#[test]
fn an_html_box_defaults_to_the_frame_and_auto_fits_the_content() {
    let both = |size: &str| {
        let text = doc(&format!(
            r#""layers":[{{"clips":[{{"duration":"2s","source":{{"kind":"html"{size},
            "html":"<style>.c {{ height: 30px }}</style><div class='c'></div>"}}}}]}}]"#
        ));
        let l = load(&text);
        assert!(l.is_ok(), "{:?}", l.diagnostics);
        match &l.composition.unwrap().layers[0].clips[0].source {
            ResolvedSource::Html(h) => (h.width, h.height),
            _ => panic!("expected markup"),
        }
    };
    // The frame of the test documents is 640 by 360.
    assert_eq!(both(""), (Some(640.0), Some(360.0)));
    assert_eq!(both(r#","height":"auto""#), (Some(640.0), None));
    assert_eq!(
        both(r#","width":"50%","height":100"#),
        (Some(320.0), Some(100.0))
    );
}

#[test]
fn a_clips_animation_replaces_the_markups() {
    let text = format!(
        "{{{HEAD},\"keyframes\":{{\"mine\":{{\"from\":\"opacity: 0\",\"to\":\"opacity: 1\"}}}},         \"layers\":[{{\"clips\":[{{\"duration\":\"2s\",\"source\":{{\"kind\":\"html\",\"width\":100,         \"html\":\"<style>@keyframes theirs {{ from {{ opacity: 1 }} to {{ opacity: 0 }} }}          .c {{ animation: theirs 1s; height: 10px }}</style><div class='c'></div>\"}},         \"animation\":\"mine 1s\"}}]}}]}}"
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    assert!(l.diagnostics.iter().any(|d| d.code == "W451"));
    let o = &l.composition.unwrap().layers[0].clips[0].opacity;
    // "mine" runs 0 to 1, not "theirs" running 1 to 0.
    assert_eq!(o.sample(0.0), 0.0);
    assert_eq!(o.sample(1.0), 1.0);
}

#[test]
fn an_animation_below_the_outermost_element_is_the_renderers_to_play() {
    let text = doc(
        r#""layers":[{"clips":[{"duration":"2s","source":{"kind":"html","width":100,
        "html":"<style>@keyframes a { to { opacity: 1 } } p { animation: a 1s; animation-delay: 0.5s }</style><div><p>hi</p></div>"}}]}]"#,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    assert!(
        !l.diagnostics
            .iter()
            .any(|d| d.code == "W450" || d.code == "W440"),
        "{:?}",
        l.diagnostics
    );
    let comp = l.composition.unwrap();
    let geneva_timeline::ResolvedSource::Html(h) = &comp.layers[0].clips[0].source else {
        panic!("an html source");
    };
    assert_eq!(h.motion.len(), 1);
    let play = &h.motion[0].plays[0];
    assert_eq!(play.animation.name, "a");
    assert!((play.animation.delay - 0.5).abs() < 1e-9);
    assert_eq!(play.frames.len(), 1, "a lone `to` is kept for the renderer");
}

#[test]
fn a_clip_cannot_play_a_rule_past_transform_and_opacity() {
    let text = doc(
        r#""layers":[{"clips":[{"duration":"2s","source":{"kind":"html","width":100,
        "html":"<style>@keyframes a { from { color: red } to { color: blue } } .c { animation: a 1s }</style><div class='c'>hi</div>"}}]}]"#,
    );
    let l = load(&text);
    assert!(
        l.diagnostics
            .iter()
            .any(|d| d.code == "E442" && d.message.contains("cannot play")),
        "{:?}",
        l.diagnostics
    );
}

#[test]
fn fixed_keyframes_need_an_interval() {
    let text = r#"{"geneva":"0.2","output":{"width":640,"height":360,"fps":30,"duration":1,"encode":{"video":{"fixed_keyframes":true}}}}"#;
    let errs = errors(text);
    assert!(
        errs.iter()
            .any(|(c, p)| *c == "E422" && p == "/output/encode/video/fixed_keyframes"),
        "{errs:?}"
    );
    let text = r#"{"geneva":"0.2","output":{"width":640,"height":360,"fps":30,"duration":1,"encode":{"video":{"fixed_keyframes":true,"keyframe_interval":2}}}}"#;
    assert!(errors(text).is_empty());
}

#[test]
fn tune_names_are_x264s() {
    use geneva_timeline::schema::VideoTune;
    for (name, tune) in [
        ("film", VideoTune::Film),
        ("animation", VideoTune::Animation),
        ("grain", VideoTune::Grain),
        ("stillimage", VideoTune::StillImage),
        ("fastdecode", VideoTune::FastDecode),
        ("zerolatency", VideoTune::ZeroLatency),
    ] {
        let text = format!(
            r#"{{"geneva":"0.2","output":{{"width":640,"height":360,"fps":30,"duration":1,"encode":{{"video":{{"tune":"{name}"}}}}}}}}"#
        );
        let l = load(&text);
        assert!(errors(&text).is_empty(), "{name}");
        let video = l.timeline.unwrap().output.encode.unwrap().video.unwrap();
        assert_eq!(video.tune, Some(tune));
        assert_eq!(tune.as_str(), name);
    }
    assert!(
        errors(r#"{"geneva":"0.2","output":{"width":640,"height":360,"fps":30,"duration":1,"encode":{"video":{"tune":"psnr"}}}}"#)
            .iter()
            .any(|(c, _)| c.starts_with("E1"))
    );
}

#[test]
fn hdr_output_needs_a_ten_bit_codec() {
    // No codec: the default is H.264, eight bits.
    let text = r#"{"geneva":"0.2","output":{"width":640,"height":360,"fps":30,"duration":1,"color":{"transfer":"pq"}}}"#;
    assert!(errors(text).iter().any(|(c, _)| *c == "E420"));
    let text = r#"{"geneva":"0.2","output":{"width":640,"height":360,"fps":30,"duration":1,"color":{"transfer":"hlg"},"encode":{"video":{"codec":"h264"}}}}"#;
    assert!(errors(text).iter().any(|(c, _)| *c == "E420"));
    for codec in ["h265", "av1", "vp9", "prores"] {
        let text = format!(
            r#"{{"geneva":"0.2","output":{{"width":640,"height":360,"fps":30,"duration":1,"color":{{"primaries":"bt2020","transfer":"pq","matrix":"bt2020-ncl"}},"encode":{{"video":{{"codec":"{codec}"}}}}}}}}"#
        );
        assert!(errors(&text).is_empty(), "{codec}: {:?}", errors(&text));
    }
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

#[test]
fn output_color_defaults_to_bt709_at_any_size() {
    let text = r#"{"geneva":"0.1","output":{"width":320,"height":180,"fps":30,"duration":1}}"#;
    let comp = load(text).composition.unwrap();
    assert_eq!(comp.color, geneva_color::ResolvedTags::SDR_VIDEO);
}

const HEAD3: &str =
    r#""geneva":"0.3","output":{"width":640,"height":360,"fps":30,"duration":"4s"}"#;

fn doc3(body: &str) -> String {
    format!("{{{HEAD3},{body}}}")
}

#[test]
fn outputs_take_their_sizes_and_names_from_the_canvas() {
    let text = doc3(
        r##""layers":[{"clips":[{"source":{"kind":"solid","color":"#fff"}}]}],
        "outputs":{
          "full":{"kind":"video"},
          "small":{"kind":"video","width":320},
          "cover":{"kind":"poster","path":"cover.png","at":"1s"},
          "seek":{"kind":"sprites","every":"2s","columns":4},
          "sound":{"kind":"audio","audio":{"sample_rate":16000,"channels":1}}
        }"##,
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:#?}", l.diagnostics);
    let comp = l.composition.unwrap();
    let by_name = |n: &str| comp.outputs.iter().find(|o| o.name == n).unwrap();
    assert_eq!(by_name("full").path, "full.mp4");
    assert_eq!((by_name("full").width, by_name("full").height), (640, 360));
    assert_eq!(
        (by_name("small").width, by_name("small").height),
        (320, 180)
    );
    assert_eq!(by_name("cover").path, "cover.png");
    assert_eq!(by_name("cover").at, Some(Ratio::from_int(1)));
    assert_eq!((by_name("seek").width, by_name("seek").height), (160, 90));
    assert_eq!(by_name("seek").every, Some(Ratio::from_int(2)));
    assert_eq!(by_name("seek").columns, Some(4));
    assert_eq!(by_name("sound").path, "sound.wav");
    assert_eq!(
        by_name("sound").audio.as_ref().and_then(|a| a.sample_rate),
        Some(16000)
    );
}

#[test]
fn outputs_reject_stray_fields_bad_paths_clashes_and_bad_values() {
    let text = doc3(
        r##""layers":[{"clips":[{"source":{"kind":"solid","color":"#fff"}}]}],
        "outputs":{
          "a":{"kind":"poster","every":"1s"},
          "b":{"kind":"video","path":"sub/b.mp4"},
          "c":{"kind":"video","path":"c.txt"},
          "d":{"kind":"video","path":"e.mp4"},
          "e":{"kind":"video"},
          "f":{"kind":"poster","at":"9s"},
          "g":{"kind":"sprites","every":"0s","columns":0}
        }"##,
    );
    let errs = errors(&text);
    let has = |code: &str, path: &str| errs.iter().any(|(c, p)| *c == code && p == path);
    assert!(has("E430", "/outputs/a/every"), "{errs:?}");
    assert!(has("E431", "/outputs/b/path"), "{errs:?}");
    assert!(has("E431", "/outputs/c/path"), "{errs:?}");
    assert!(
        has("E432", "/outputs/e/path") || has("E432", "/outputs/d/path"),
        "{errs:?}"
    );
    assert!(has("E433", "/outputs/f/at"), "{errs:?}");
    assert!(has("E433", "/outputs/g/every"), "{errs:?}");
    assert!(has("E433", "/outputs/g/columns"), "{errs:?}");
}

#[test]
fn outputs_need_format_0_3_and_are_absent_by_default() {
    let l = load(&doc(
        r##""layers":[{"clips":[{"source":{"kind":"solid","color":"#fff"}}]}]"##,
    ));
    assert!(l.composition.unwrap().outputs.is_empty());
}

#[test]
fn a_fade_shows_one_clip_at_a_time_and_a_crossfade_shows_both() {
    let text = r##"{
      "geneva": "0.3",
      "output": { "width": 64, "height": 64, "fps": 25 },
      "layers": [ { "clips": [
        { "source": { "kind": "solid", "color": "red" }, "duration": "2s" },
        { "source": { "kind": "solid", "color": "blue" }, "duration": "2s",
          "transition": { "kind": "fade", "duration": "1s", "color": "#404040" } }
      ] } ]
    }"##;
    let comp = load(text).composition.unwrap();
    let tr = comp.layers[0].clips[1].transition_in.clone().unwrap();
    assert_eq!(tr.duration, Ratio::from_int(1));

    // A fade hands over in the middle: the outgoing clip is gone before
    // the incoming one appears, so they are never on screen together.
    let half = Ratio::new(1, 2);
    assert_eq!(tr.incoming(Ratio::ZERO), 0.0);
    assert_eq!(tr.incoming(half), 0.0);
    assert_eq!(tr.incoming(Ratio::from_int(1)), 1.0);
    assert_eq!(tr.outgoing(Ratio::ZERO), 0.0);
    assert_eq!(tr.outgoing(half), 0.0);
    assert_eq!(tr.outgoing(Ratio::from_int(1)), 1.0);
    for i in 0..=20 {
        let local = Ratio::new(i, 20);
        let both = tr.incoming(local) * tr.outgoing(Ratio::from_int(1) - local);
        assert_eq!(both, 0.0, "both clips are up at {local}s of the fade");
    }
    // The dip colour peaks in the middle and is clear at both ends.
    assert_eq!(tr.veil(Ratio::ZERO), 0.0);
    assert_eq!(tr.veil(half), 1.0);
    assert_eq!(tr.veil(Ratio::from_int(1)), 0.0);

    // A crossfade keeps the outgoing clip up and brings the other over it.
    let text = text.replace(r#""kind": "fade""#, r#""kind": "crossfade""#);
    let comp = load(&text).composition.unwrap();
    let tr = comp.layers[0].clips[1].transition_in.clone().unwrap();
    assert_eq!(tr.incoming(half), 0.5);
    assert_eq!(tr.outgoing(half), 1.0);
    assert_eq!(tr.veil(half), 0.0, "a crossfade shows no colour of its own");
}

#[test]
fn a_transition_colour_on_a_crossfade_is_a_warning() {
    let text = r#"{
      "geneva": "0.3",
      "output": { "width": 64, "height": 64, "fps": 25 },
      "layers": [ { "clips": [
        { "source": { "kind": "solid", "color": "red" }, "duration": "2s" },
        { "source": { "kind": "solid", "color": "blue" }, "duration": "2s",
          "transition": { "kind": "crossfade", "duration": "1s", "color": "white" } }
      ] } ]
    }"#;
    let found = codes(&load(text).diagnostics);
    assert!(
        found
            .iter()
            .any(|(c, p)| *c == "W304" && p == "/layers/0/clips/1/transition/color"),
        "{found:?}"
    );
}

#[test]
fn a_transition_can_open_and_close_a_layer() {
    let text = r#"{
      "geneva": "0.3",
      "output": { "width": 64, "height": 64, "fps": 25, "duration": "3s" },
      "layers": [ { "clips": [
        { "source": { "kind": "solid", "color": "red" }, "duration": "3s",
          "transition": { "kind": "fade", "duration": "1s" },
          "transition_out": { "kind": "fade", "duration": "1s" } }
      ] } ]
    }"#;
    let loaded = load(text);
    assert!(loaded.is_ok(), "{:#?}", loaded.diagnostics);
    assert!(
        !codes(&loaded.diagnostics).iter().any(|(c, _)| *c == "W303"),
        "opening a layer with a transition is no longer a warning"
    );
    let clip = &loaded.composition.unwrap().layers[0].clips[0];

    // Opening the layer, nothing waits: the clip comes up across the whole
    // ramp and the colour clears as it does.
    let open = clip.transition_in.clone().unwrap();
    assert!(!open.paired);
    assert_eq!(open.incoming(Ratio::ZERO), 0.0);
    assert_eq!(open.incoming(Ratio::new(1, 2)), 0.5);
    assert_eq!(open.incoming(Ratio::from_int(1)), 1.0);
    assert_eq!(open.veil(Ratio::ZERO), 1.0);
    assert_eq!(open.veil(Ratio::new(1, 2)), 0.5);

    // Closing it is the mirror, measured back from the clip's end.
    let close = clip.transition_out.clone().unwrap();
    assert!(!close.paired);
    assert_eq!(close.outgoing(Ratio::ZERO), 0.0);
    assert_eq!(close.outgoing(Ratio::from_int(1)), 1.0);
    assert_eq!(close.veil(Ratio::ZERO), 1.0);

    // The clip is not pulled earlier, since there is nothing to overlap.
    assert_eq!(clip.start, Ratio::ZERO);
}

#[test]
fn transition_out_beside_the_next_clips_transition_is_an_error() {
    let text = r#"{
      "geneva": "0.3",
      "output": { "width": 64, "height": 64, "fps": 25, "duration": "4s" },
      "layers": [ { "clips": [
        { "source": { "kind": "solid", "color": "red" }, "duration": "2s",
          "transition_out": { "kind": "fade", "duration": "0.5s" } },
        { "source": { "kind": "solid", "color": "blue" }, "duration": "2s",
          "transition": { "kind": "crossfade", "duration": "0.5s" } }
      ] } ]
    }"#;
    let found = errors(text);
    assert!(
        found
            .iter()
            .any(|(c, p)| *c == "E307" && p == "/layers/0/clips/0/transition_out"),
        "{found:?}"
    );
}

#[test]
fn an_easing_shapes_the_transition_ramp() {
    let make = |ease: &str| {
        let text = format!(
            r#"{{
              "geneva": "0.3",
              "output": {{ "width": 64, "height": 64, "fps": 25, "duration": "2s" }},
              "layers": [ {{ "clips": [
                {{ "source": {{ "kind": "solid", "color": "red" }}, "duration": "2s",
                   "transition": {{ "kind": "crossfade", "duration": "1s", "ease": "{ease}" }} }}
              ] }} ]
            }}"#
        );
        load(&text).composition.unwrap().layers[0].clips[0]
            .transition_in
            .clone()
            .unwrap()
    };
    let quarter = Ratio::new(1, 4);
    let linear = make("linear").incoming(quarter);
    let eased = make("ease-in-out").incoming(quarter);
    assert!(
        (linear - 0.25).abs() < 1e-9,
        "linear should be flat: {linear}"
    );
    assert!(
        eased < linear - 0.05,
        "ease-in-out starts slower than linear: {eased} against {linear}"
    );
    // Both still start at nothing and finish whole.
    for ease in ["linear", "ease-in-out"] {
        let t = make(ease);
        assert_eq!(t.incoming(Ratio::ZERO), 0.0);
        assert_eq!(t.incoming(Ratio::from_int(1)), 1.0);
    }
}
