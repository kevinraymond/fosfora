use super::*;
use crate::ui::shell::Workspace;
use crate::ui::shell_harness::ShellHarness;
use egui::{Pos2, Rect};

fn setup(size: Vec2, page: Page) -> ShellHarness {
    let mut h = ShellHarness::new(size);
    Workspace::Setup.write(&h.ctx);
    open(&h.ctx, page);
    h.settle(3);
    h
}

/// The page list's own rows: left of the page.
fn nav_rect(h: &mut ShellHarness, title: &str) -> Rect {
    h.text_rects(title)
        .into_iter()
        .find(|r| r.left() < 250.0)
        .unwrap_or_else(|| panic!("{title} is not in the list"))
}

// Kevin's pain was layout: rows cut off and fields that don't line up. Every
// page, at a laptop's window and a large one, must paint all its text inside
// the column it is drawn in, and the column must leave the output alone.
#[test]
fn every_page_draws_inside_its_column() {
    for size in [Vec2::new(1206.0, 760.0), Vec2::new(1920.0, 1080.0)] {
        for page in Page::ALL {
            let mut h = setup(size, page);
            let texts = h.texts();
            let at = format!("{size:?} {}", page.title());
            let heading = texts
                .iter()
                .find(|(t, r, _)| t == page.title() && r.left() > 250.0)
                .unwrap_or_else(|| panic!("{at}: no heading"));
            let column = heading.2;
            let mut inside = 0;
            for (t, r, clip) in &texts {
                if *clip != column {
                    continue;
                }
                inside += 1;
                assert!(
                    r.right() <= clip.right() + 1.0 && r.left() >= clip.left() - 1.0,
                    "{at}: {t:?} at {r:?} runs out of {clip:?}"
                );
            }
            assert!(inside > 3, "{at}: only {inside} texts on the page");
            assert!(
                column.right() < size.x - 250.0,
                "{at}: the page reaches over the output ({column:?})"
            );
        }
    }
}

// The list opens each page, and says each device page's state in words.
#[test]
fn the_list_opens_each_page() {
    let mut h = setup(Vec2::new(1400.0, 900.0), Page::Audio);
    for page in Page::ALL {
        let r = nav_rect(&mut h, page.title());
        h.click(r.center());
        h.settle(2);
        assert_eq!(current(&h.ctx), page);
    }
    // The harness's audio is offline, MIDI, OSC and web are off.
    let words: Vec<String> = h.texts().into_iter().map(|t| t.0).collect();
    let sync = if cfg!(feature = "link") {
        "Off"
    } else {
        "Not in this build"
    };
    for w in ["No input", "Off", sync] {
        assert!(words.iter().any(|t| t == w), "no {w:?} in the list");
    }
}

// The table used to list ten of the fourteen actions: Timeline, Tempo ÷2,
// Tempo ×2 and Tap could be fired over OSC but never mapped (decision #3243).
#[test]
fn the_triggers_table_lists_every_action() {
    // Tall enough for the whole page: egui paints nothing scrolled away.
    let mut h = setup(Vec2::new(1400.0, 3000.0), Page::Control);
    let words: Vec<String> = h.texts().into_iter().map(|t| t.0).collect();
    for a in crate::midi::types::TriggerAction::ALL {
        let name = control::action_name(*a);
        assert!(words.iter().any(|t| t == name), "{name} has no row");
    }
    assert_eq!(
        control::ACTIONS.len(),
        crate::midi::types::TriggerAction::ALL.len()
    );
    assert_eq!(h.text_rects("Learn").len(), 2 * control::ACTIONS.len());
}

// Global's settings moved to where they act (decision #3243); each must
// still be on some page.
#[test]
fn every_setting_has_a_page() {
    for (page, labels) in [
        (
            Page::Audio,
            &["If it goes quiet", "Band scale", "Device"][..],
        ),
        (Page::Outputs, &["Black becomes", "Second window"][..]),
        (Page::General, &["Particle quality", "Flash limiter"][..]),
        (
            Page::Control,
            &["Listen on port", "Addresses start", "Port"][..],
        ),
        (Page::Tutorials, &["First run", "Layers and blending"][..]),
    ] {
        let mut h = setup(Vec2::new(1400.0, 3000.0), page);
        let words: Vec<String> = h.texts().into_iter().map(|t| t.0).collect();
        for l in labels {
            assert!(
                words.iter().any(|t| t == l),
                "{} has no {l:?}",
                page.title()
            );
        }
    }
}

// Recording's button sends the request `main.rs` handles, and the page
// says what is running.
#[test]
fn record_sends_the_same_request() {
    let mut h = setup(Vec2::new(1400.0, 900.0), Page::Outputs);
    let info = crate::ui::panels::recording_panel::RecordingInfo {
        ffmpeg_found: true,
        ..Default::default()
    };
    h.ctx
        .data_mut(|d| d.insert_temp(egui::Id::new("recording_info"), info.clone()));
    h.settle(2);
    let b = h.text_rect("●  Record").expect("a Record button");
    h.click(b.center());
    let sent = h
        .ctx
        .data_mut(|d| d.remove_temp::<bool>(egui::Id::new("recording_toggle")));
    assert_eq!(sent, Some(true));

    let rec = crate::ui::panels::recording_panel::RecordingInfo {
        recording: true,
        duration_secs: 84.0,
        ..info
    };
    h.ctx
        .data_mut(|d| d.insert_temp(egui::Id::new("recording_info"), rec));
    h.settle(2);
    assert!(h.text_rect("Recording 01:24").is_some());
    assert!(h.text_rect("REC 01:24").is_some(), "the list says so too");
}

// The switch says its state in words and by the knob's side, not by color.
#[test]
fn the_switch_says_on_and_off() {
    let ctx = egui::Context::default();
    let mut rects = Vec::new();
    for on in [false, true] {
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                kit::switch(ui, on, "MIDI");
            });
        });
        let words: Vec<(String, Pos2)> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.pos)),
                _ => None,
            })
            .collect();
        let want = if on { "On" } else { "Off" };
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].0, want);
        rects.push(words[0].1.x);
    }
    // The word moves to the side the knob left.
    assert!(rects[1] < rects[0]);
}

/// What main.rs publishes for the streams this build has: each on and
/// sending, or, with `missing`, each without what it needs installed.
#[cfg_attr(not(any(feature = "ndi", feature = "v4l2")), allow(unused_variables))]
fn publish_streams(h: &ShellHarness, missing: bool) {
    #[cfg(feature = "ndi")]
    h.ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::new("ndi_info"),
            crate::ui::panels::ndi_panel::NdiInfo {
                enabled: true,
                running: true,
                ndi_available: !missing,
                source_name: "Fosfora".into(),
                output_width: 1920,
                output_height: 1080,
                frames_sent: 12_480,
                ..Default::default()
            },
        );
    });
    #[cfg(all(target_os = "linux", feature = "v4l2"))]
    h.ctx.data_mut(|d| {
        let devices = if missing {
            vec![]
        } else {
            vec![("/dev/video10".to_string(), "Fosfora".to_string())]
        };
        d.insert_temp(
            egui::Id::new("v4l2_info"),
            crate::ui::panels::v4l2_panel::V4l2Info {
                enabled: true,
                running: true,
                devices,
                resolved_path: Some("/dev/video10".into()),
                output_width: 1280,
                output_height: 720,
                error: Some("A long failure message from the driver, which has to wrap onto a second line rather than run out of its block".into()),
                ..Default::default()
            },
        );
    });
}

// The stream blocks, running and missing their runtimes, fit the column
// like every other block.
#[test]
fn the_streams_draw_inside_the_column() {
    for missing in [false, true] {
        for size in [Vec2::new(1206.0, 2400.0), Vec2::new(1920.0, 2400.0)] {
            let mut h = setup(size, Page::Outputs);
            publish_streams(&h, missing);
            h.settle(3);
            let texts = h.texts();
            let column = texts
                .iter()
                .find(|(t, r, _)| t == "Outputs and streams" && r.left() > 250.0)
                .unwrap()
                .2;
            for (t, r, clip) in texts.iter().filter(|(_, _, c)| *c == column) {
                assert!(
                    r.right() <= clip.right() + 1.0,
                    "{size:?} missing {missing}: {t:?} runs out of the column"
                );
            }
            #[cfg(feature = "ndi")]
            {
                let words = if missing {
                    "Needs the NDI® Runtime"
                } else {
                    "Sending as Fosfora"
                };
                assert!(texts.iter().any(|t| t.0 == words), "{words}");
            }
            #[cfg(all(target_os = "linux", feature = "v4l2"))]
            {
                let words = if missing {
                    "Needs v4l2loopback"
                } else {
                    "Sending to /dev/video10"
                };
                assert!(texts.iter().any(|t| t.0 == words), "{words}");
            }
        }
    }
}

// A stream's switch sends the request `main.rs` handles.
#[cfg(feature = "ndi")]
#[test]
fn the_ndi_switch_sends_the_same_request() {
    let mut h = setup(Vec2::new(1400.0, 2400.0), Page::Outputs);
    publish_streams(&h, false);
    h.settle(3);
    let title = h.text_rect("NDI®").expect("an NDI block");
    let switch = h
        .text_rects("On")
        .into_iter()
        .find(|r| (r.center().y - title.center().y).abs() < 12.0)
        .expect("a switch on the NDI row");
    h.click(switch.center());
    let sent = h
        .ctx
        .data_mut(|d| d.remove_temp::<bool>(egui::Id::new("ndi_set_enabled")));
    assert_eq!(sent, Some(false), "a click on On turns it off");
}

// Kevin's live check: four switches at On, two of them waiting, and the
// list said "1 on". It counts what is switched on and says what waits.
#[test]
fn a_page_counts_what_is_switched_on() {
    use kit::{Status, summarize};
    let on = |w: &str| Status::new(Mark::Active, w);
    let waiting = |w: &str| Status::new(Mark::Idle, w);
    let s = summarize(&[
        on("Connected to Midi Through"),
        waiting("Listening on port 9000"),
        on("Sending to 127.0.0.1:9001"),
        waiting("Running, nothing connected"),
    ]);
    assert_eq!(s, Status::new(Mark::Active, "4 on, 2 waiting"));
    assert_eq!(summarize(&[on(""), Status::off()]).words, "1 on");
    let s = summarize(&[waiting(""), Status::off()]);
    assert_eq!(s, Status::new(Mark::Idle, "1 on, 1 waiting"));
    assert_eq!(summarize(&[Status::off(), Status::off()]), Status::off());
    assert_eq!(
        summarize(&[on(""), Status::new(Mark::Fault, "")]).mark,
        Mark::Fault
    );
}

// A block's state sits on its title's row, centered, not above it.
#[test]
fn a_blocks_state_is_level_with_its_title() {
    let mut h = setup(Vec2::new(1400.0, 900.0), Page::Control);
    let texts = h.texts();
    // The block's title, not the status bar's MIDI along the bottom.
    let title = texts
        .iter()
        .find(|t| t.0 == "MIDI" && t.1.left() > 250.0 && t.1.top() < 400.0)
        .unwrap()
        .1;
    // The words just right of the title: the state, not the switch.
    let state = texts
        .iter()
        .filter(|t| t.1.left() > title.right() && t.1.left() < title.right() + 60.0)
        .filter(|t| (t.1.center().y - title.center().y).abs() < 30.0)
        .min_by(|a, b| a.1.left().total_cmp(&b.1.left()))
        .expect("MIDI's state")
        .1;
    assert!(
        (state.center().y - title.center().y).abs() < 2.0,
        "title {title:?}, state {state:?}"
    );
}
