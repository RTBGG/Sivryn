//! Icons from Lucide (https://lucide.dev, ISC license), drawn with the
//! element's text color (`currentColor`).
//!
//! Blitz rasterizes an inline SVG when it is built, so an icon whose color
//! should change with state must be rebuilt: render a different icon, or put
//! it in a different branch of the markup.

use dioxus_native::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Check,
    ChevronDown,
    Eye,
    EyeOff,
    Hash,
    Headphones,
    HeadphonesOff,
    Info,
    Keyboard,
    Lock,
    LogIn,
    LogOut,
    Maximize,
    Mic,
    Minimize,
    MicOff,
    Monitor,
    MonitorOff,
    MonitorUp,
    Palette,
    PhoneOff,
    Plus,
    Send,
    Settings,
    ShieldCheck,
    User,
    UserPlus,
    Video,
    VideoOff,
    Volume,
    X,
    Bell,
    AtSign,
    Ban,
    ChevronRight,
    CirclePlus,
    Copy,
    Clipboard,
    Scissors,
    Download,
    External,
    File,
    Image,
    Link,
    Message,
    Pause,
    Pencil,
    Phone,
    Play,
    Reply,
    Search,
    Smile,
    Trash,
    Upload,
    UserMinus,
    Users,
    Folder,
    BellOff,
    Crown,
    ArrowUp,
    ArrowDown,
    VolumeX,
    Shield,
}

/// Shapes in a 24×24 box, stroked with width 2 and round caps/joins.
enum Shape {
    Path(&'static str),
    Circle(f32, f32, f32),
    Rect(f32, f32, f32, f32, f32),
}

use Shape::*;

fn shapes(icon: Icon) -> &'static [Shape] {
    match icon {
        Icon::Check => &[Path("M20 6 9 17l-5-5")],
        Icon::ChevronDown => &[Path("m6 9 6 6 6-6")],
        Icon::Eye => &[Path("M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"), Circle(12.0, 12.0, 3.0)],
        Icon::EyeOff => {
            &[Path("M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"), Circle(12.0, 12.0, 3.0), Path("M3 3l18 18")]
        }
        Icon::Hash => &[Path("M4 9h16"), Path("M4 15h16"), Path("M10 3 8 21"), Path("M16 3l-2 18")],
        Icon::Headphones => &[Path(
            "M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3",
        )],
        Icon::HeadphonesOff => &[
            Path(
                "M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3",
            ),
            Path("M2 2l20 20"),
        ],
        Icon::Info => &[Circle(12.0, 12.0, 10.0), Path("M12 16v-4"), Path("M12 8h.01")],
        Icon::Keyboard => &[
            Rect(2.0, 4.0, 20.0, 16.0, 2.0),
            Path("M6 8h.01M10 8h.01M14 8h.01M18 8h.01M8 12h.01M12 12h.01M16 12h.01M7 16h10"),
        ],
        Icon::Lock => &[Rect(3.0, 11.0, 18.0, 11.0, 2.0), Path("M7 11V7a5 5 0 0 1 10 0v4")],
        Icon::LogIn => &[Path("M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"), Path("m10 17 5-5-5-5"), Path("M15 12H3")],
        Icon::LogOut => &[Path("M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"), Path("m16 17 5-5-5-5"), Path("M21 12H9")],
        Icon::Maximize => &[Path("M15 3h6v6"), Path("M9 21H3v-6"), Path("M21 3l-7 7"), Path("M3 21l7-7")],
        Icon::Minimize => &[Path("M4 14h6v6"), Path("M20 10h-6V4"), Path("M14 10l7-7"), Path("M3 21l7-7")],
        Icon::Mic => &[
            Path("M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"),
            Path("M19 10v2a7 7 0 0 1-14 0v-2"),
            Path("M12 19v3"),
        ],
        Icon::MicOff => &[
            Path("M2 2l20 20"),
            Path("M18.89 13.23A7.12 7.12 0 0 0 19 12v-2"),
            Path("M5 10v2a7 7 0 0 0 12 5"),
            Path("M15 9.34V5a3 3 0 0 0-5.68-1.33"),
            Path("M9 9v3a3 3 0 0 0 5.12 2.12"),
            Path("M12 19v3"),
        ],
        Icon::Monitor => &[Rect(2.0, 3.0, 20.0, 14.0, 2.0), Path("M8 21h8"), Path("M12 17v4")],
        Icon::MonitorOff => {
            &[Rect(2.0, 3.0, 20.0, 14.0, 2.0), Path("M8 21h8"), Path("M12 17v4"), Path("M9 7l6 6"), Path("M15 7l-6 6")]
        }
        Icon::MonitorUp => &[
            Rect(2.0, 3.0, 20.0, 14.0, 2.0),
            Path("M8 21h8"),
            Path("M12 17v4"),
            Path("m9 10 3-3 3 3"),
            Path("M12 13V7"),
        ],
        Icon::Palette => &[
            Circle(13.5, 6.5, 0.5),
            Circle(17.5, 10.5, 0.5),
            Circle(8.5, 7.5, 0.5),
            Circle(6.5, 12.5, 0.5),
            Path(
                "M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z",
            ),
        ],
        // A handset lying down: the usual "hang up" symbol.
        Icon::PhoneOff => &[Path(
            "M3.5 14.5c4.6-4.4 12.4-4.4 17 0l-1.9 2.6c-.3.4-.9.5-1.3.3l-2.4-1.2c-.4-.2-.6-.6-.5-1l.3-1.6c-2.4-.9-5-.9-7.4 0l.3 1.6c.1.4-.1.8-.5 1l-2.4 1.2c-.4.2-1 .1-1.3-.3z",
        )],
        Icon::Plus => &[Path("M5 12h14"), Path("M12 5v14")],
        Icon::Send => &[Path("m22 2-7 20-4-9-9-4Z"), Path("M22 2 11 13")],
        Icon::Settings => &[
            Path(
                "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z",
            ),
            Circle(12.0, 12.0, 3.0),
        ],
        Icon::ShieldCheck => &[
            Path(
                "M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z",
            ),
            Path("m9 12 2 2 4-4"),
        ],
        Icon::User => &[Path("M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"), Circle(12.0, 7.0, 4.0)],
        Icon::UserPlus => &[
            Path("M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"),
            Circle(9.0, 7.0, 4.0),
            Path("M19 8v6"),
            Path("M22 11h-6"),
        ],
        Icon::Video => &[
            Path("m16 13 5.223 3.482a.5.5 0 0 0 .777-.416V7.87a.5.5 0 0 0-.752-.432L16 10.5"),
            Rect(2.0, 6.0, 14.0, 12.0, 2.0),
        ],
        Icon::VideoOff => &[
            Path("m16 13 5.223 3.482a.5.5 0 0 0 .777-.416V7.87a.5.5 0 0 0-.752-.432L16 10.5"),
            Rect(2.0, 6.0, 14.0, 12.0, 2.0),
            Path("M2 2l20 20"),
        ],
        Icon::Volume => &[
            Path("M11 5 6 9H2v6h4l5 4V5z"),
            Path("M15.54 8.46a5 5 0 0 1 0 7.07"),
            Path("M19.07 4.93a10 10 0 0 1 0 14.14"),
        ],
        Icon::X => &[Path("M18 6 6 18"), Path("m6 6 12 12")],
        Icon::Bell => &[Path("M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"), Path("M10.3 21a1.94 1.94 0 0 0 3.4 0")],
        Icon::AtSign => &[Circle(12.0, 12.0, 4.0), Path("M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-4 8")],
        Icon::Ban => &[Circle(12.0, 12.0, 10.0), Path("m4.9 4.9 14.2 14.2")],
        Icon::ChevronRight => &[Path("m9 18 6-6-6-6")],
        Icon::CirclePlus => &[Circle(12.0, 12.0, 10.0), Path("M8 12h8"), Path("M12 8v8")],
        Icon::Clipboard => &[
            Rect(8.0, 2.0, 8.0, 4.0, 1.0),
            Path("M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"),
        ],
        Icon::Scissors => &[
            Circle(6.0, 6.0, 3.0),
            Circle(6.0, 18.0, 3.0),
            Path("M20 4 8.12 15.88"),
            Path("M14.47 14.48 20 20"),
            Path("M8.12 8.12 12 12"),
        ],
        Icon::Copy => {
            &[Rect(8.0, 8.0, 14.0, 14.0, 2.0), Path("M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2")]
        }
        Icon::Download => &[Path("M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"), Path("m7 10 5 5 5-5"), Path("M12 15V3")],
        Icon::External => {
            &[Path("M15 3h6v6"), Path("M10 14 21 3"), Path("M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6")]
        }
        Icon::File => {
            &[Path("M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"), Path("M14 2v4a2 2 0 0 0 2 2h4")]
        }
        Icon::Image => {
            &[Rect(3.0, 3.0, 18.0, 18.0, 2.0), Circle(9.0, 9.0, 2.0), Path("m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21")]
        }
        Icon::Link => &[
            Path("M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"),
            Path("M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"),
        ],
        Icon::Message => &[Path("M7.9 20A9 9 0 1 0 4 16.1L2 22Z")],
        Icon::Pause => &[Rect(14.0, 4.0, 4.0, 16.0, 1.0), Rect(6.0, 4.0, 4.0, 16.0, 1.0)],
        Icon::Pencil => &[
            Path(
                "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z",
            ),
            Path("m15 5 4 4"),
        ],
        Icon::Phone => &[Path(
            "M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7A2 2 0 0 1 22 16.92z",
        )],
        Icon::Play => &[Path("m6 3 14 9-14 9V3z")],
        Icon::Reply => &[Path("m9 17-5-5 5-5"), Path("M20 18v-2a4 4 0 0 0-4-4H4")],
        Icon::Search => &[Circle(11.0, 11.0, 8.0), Path("m21 21-4.3-4.3")],
        Icon::Smile => {
            &[Circle(12.0, 12.0, 10.0), Path("M8 14s1.5 2 4 2 4-2 4-2"), Path("M9 9h.01"), Path("M15 9h.01")]
        }
        Icon::Trash => {
            &[Path("M3 6h18"), Path("M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"), Path("M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2")]
        }
        Icon::Upload => &[Path("M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"), Path("m17 8-5-5-5 5"), Path("M12 3v12")],
        Icon::UserMinus => {
            &[Path("M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"), Circle(9.0, 7.0, 4.0), Path("M22 11h-6")]
        }
        Icon::Folder => &[Path(
            "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
        )],
        Icon::BellOff => &[
            Path("M8.7 3A6 6 0 0 1 18 8a21.3 21.3 0 0 0 .6 5"),
            Path("M17 17H3s3-2 3-9a4.67 4.67 0 0 1 .3-1.7"),
            Path("M10.3 21a1.94 1.94 0 0 0 3.4 0"),
            Path("m2 2 20 20"),
        ],
        Icon::Crown => &[
            Path(
                "M11.56 3.27a.5.5 0 0 1 .88 0l2.95 5.6a1 1 0 0 0 1.52.3l4.27-3.66a.5.5 0 0 1 .8.52l-2.83 10.24a1 1 0 0 1-.96.73H5.81a1 1 0 0 1-.96-.73L2.02 6.02a.5.5 0 0 1 .8-.52l4.27 3.66a1 1 0 0 0 1.52-.3z",
            ),
            Path("M5 21h14"),
        ],
        Icon::ArrowUp => &[Path("m5 12 7-7 7 7"), Path("M12 19V5")],
        Icon::ArrowDown => &[Path("M12 5v14"), Path("m19 12-7 7-7-7")],
        Icon::VolumeX => &[
            Path(
                "M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5z",
            ),
            Path("m22 9-6 6"),
            Path("m16 9 6 6"),
        ],
        Icon::Shield => &[Path(
            "M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z",
        )],
        Icon::Users => &[
            Path("M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"),
            Circle(9.0, 7.0, 4.0),
            Path("M22 21v-2a4 4 0 0 0-3-3.87"),
            Path("M16 3.13a4 4 0 0 1 0 7.75"),
        ],
    }
}

#[component]
pub fn Svg(icon: Icon, #[props(default = 20)] size: u32, #[props(default = 2.0)] stroke: f32) -> Element {
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            // Flex layouts would otherwise stretch or shrink the icon.
            style: "width: {size}px; height: {size}px; min-width: {size}px; min-height: {size}px; flex-shrink: 0;",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "{stroke}",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            for shape in shapes(icon) {
                match *shape {
                    Path(d) => rsx! { path { d } },
                    Circle(cx, cy, r) => rsx! { circle { cx: "{cx}", cy: "{cy}", r: "{r}" } },
                    Rect(x, y, w, h, rx) => rsx! { rect { x: "{x}", y: "{y}", width: "{w}", height: "{h}", rx: "{rx}" } },
                }
            }
        }
    }
}

/// The Sivryn mark: two interlocking chevrons forming a diamond.
/// (Solid colors: Blitz's SVG renderer doesn't resolve gradient references
/// in inline SVG.)
#[component]
pub fn Logo(#[props(default = 36)] size: u32) -> Element {
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            style: "width: {size}px; height: {size}px; min-width: {size}px; flex-shrink: 0;",
            view_box: "0 0 32 32",
            rect { x: "0", y: "0", width: "32", height: "32", rx: "9", fill: "#7c6cff" }
            path { d: "M16 6 25 15.5 20.5 15.5 16 11 11.5 15.5 7 15.5Z", fill: "#ffffff" }
            path { d: "M16 26 7 16.5 11.5 16.5 16 21 20.5 16.5 25 16.5Z", fill: "#ffffff", fill_opacity: "0.72" }
        }
    }
}
