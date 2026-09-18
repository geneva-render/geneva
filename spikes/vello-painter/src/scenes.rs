//! The documents the spike draws both ways.
//!
//! Boxes only: every scene is something the markup subset draws and the
//! goldens rely on, with no text and no images, so a difference in the
//! picture is a difference in the drawing rather than in two text
//! engines disagreeing about where a line breaks.

/// One document to draw both ways.
pub struct Scene {
    /// What it is called on the command line and in the output files.
    pub name: &'static str,
    /// The markup.
    pub html: &'static str,
    /// The surface's width.
    pub width: u32,
    /// The surface's height.
    pub height: u32,
}

/// Every scene, in the order the report lists them.
pub fn all() -> Vec<Scene> {
    vec![
        Scene {
            name: "radii",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .b { position: absolute; left: 40px; top: 30px; width: 320px; height: 140px; \
                   background: #2f81f7; border-radius: 4px 28px 60px 12px }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "border-one-colour",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .b { position: absolute; left: 40px; top: 30px; width: 320px; height: 140px; \
                   background: #16232e; border: 8px solid #f0b429; border-radius: 20px }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "border-four-colours",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .b { position: absolute; left: 40px; top: 30px; width: 320px; height: 140px; \
                   background: #16232e; border-width: 10px; border-style: solid; \
                   border-color: #f0b429 #2f81f7 #00eee1 #ff6b6b }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "linear-gradient",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .b { position: absolute; left: 20px; top: 20px; width: 360px; height: 160px; \
                   background: linear-gradient(100deg, #00eee1 0%, #2f81f7 45%, #ffd233 100%) }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "linear-to-corner",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .b { position: absolute; left: 20px; top: 20px; width: 360px; height: 160px; \
                   background: linear-gradient(to bottom right, #00eee1, #ffd233) }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "radial-ellipse",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .b { position: absolute; left: 0; top: 0; width: 400px; height: 200px; \
                   background: radial-gradient(circle at 50% 50%, rgba(0,238,225,.85) 0%, \
                   rgba(0,238,225,.25) 40%, rgba(0,238,225,0) 70%) }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "box-shadow",
            width: 400,
            height: 240,
            html: "<style>body { margin: 0; width: 400px; height: 240px; background: #101418 }\
                   .b { position: absolute; left: 70px; top: 50px; width: 260px; height: 140px; \
                   background: #16232e; border-radius: 24px; \
                   box-shadow: 0 24px 64px rgba(0,0,0,.6) }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "clip-path-polygon",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .b { position: absolute; left: 20px; top: 20px; width: 360px; height: 160px; \
                   background: linear-gradient(90deg, #00eee1, #ffd233); \
                   clip-path: polygon(0 0, 62% 0, 48% 100%, 0 100%) }</style>\
                   <div class=b></div>",
        },
        Scene {
            name: "overflow-hidden",
            width: 400,
            height: 200,
            html: "<style>body { margin: 0; width: 400px; height: 200px; background: #101418 }\
                   .card { position: absolute; left: 40px; top: 30px; width: 320px; height: 140px; \
                   border-radius: 30px; overflow: hidden; background: #16232e }\
                   .inner { position: absolute; left: -20px; top: -20px; width: 240px; height: 240px; \
                   background: #f0b429 }</style>\
                   <div class=card><div class=inner></div></div>",
        },
        Scene {
            name: "stack",
            width: 400,
            height: 240,
            html: "<style>body { margin: 0; width: 400px; height: 240px; background: #0b1418 }\
                   .a { position: absolute; left: 30px; top: 30px; width: 200px; height: 120px; \
                   background: rgba(0,238,225,.5); border-radius: 18px }\
                   .b { position: absolute; left: 120px; top: 80px; width: 200px; height: 120px; \
                   background: rgba(255,210,51,.55); border-radius: 18px }\
                   .c { position: absolute; left: 70px; top: 120px; width: 260px; height: 90px; \
                   background: rgba(47,129,247,.6); border-radius: 45px }</style>\
                   <div class=a></div><div class=b></div><div class=c></div>",
        },
    ]
}
