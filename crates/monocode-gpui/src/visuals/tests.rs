use super::*;
use gpui::TestAppContext;

fn document(source: &str) -> VisualDocument {
    VisualDocument {
        format: Default::default(),
        height: None,
        id: "native-test".into(),
        title: "Native visual".into(),
        source: source.into(),
    }
}
fn first_text(node: &Option<Node>) -> &str {
    match node.as_ref().unwrap() {
        Node::Column { children, .. } => match &children[0] {
            Node::Text { text, .. } => text,
            _ => panic!("Expected text"),
        },
        _ => panic!("Expected column"),
    }
}

#[gpui::test]
fn native_button_preserves_state_across_frames_and_reset_restarts_it(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        const state = visual.state({count: 0});
        return () => visual.column([
            visual.text(String(state.count)),
            visual.button('Increase', () => { state.count++; })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("visual-expand").is_none());
    for expected in ["1", "2"] {
        let bounds = visual
            .debug_bounds("visual-button-root/1")
            .expect("Native GPUI button must be present");
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| {
                assert_eq!(first_text(&view.tree), expected)
            })
            .unwrap();
    }
    let source = visual.debug_bounds("visual-source").unwrap();
    visual.simulate_click(source.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("visual-button-root/1").is_none());
    window
        .update(visual, |view, _, _| assert!(view.show_source))
        .unwrap();
    let copy = visual.debug_bounds("visual-copy").unwrap();
    visual.simulate_click(copy.center(), Default::default());
    visual.run_until_parked();
    let source_text = window
        .update(visual, |view, _, _| view.document.source.clone())
        .unwrap();
    assert_eq!(
        visual.read_from_clipboard().unwrap().text(),
        Some(source_text),
        "The compact copy action must preserve the complete original program"
    );
    let source = visual.debug_bounds("visual-source").unwrap();
    visual.simulate_click(source.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| assert_eq!(first_text(&view.tree), "2"))
        .unwrap();
    let reset = visual.debug_bounds("visual-reset").unwrap();
    visual.simulate_click(reset.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| assert_eq!(first_text(&view.tree), "0"))
        .unwrap();
}

#[gpui::test]
fn native_tabs_render_and_rebind_selection_across_clicks(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        const state = visual.state({tab: 0});
        return () => visual.column([
            visual.text(String(state.tab)),
            visual.tabs(['First', 'Second', 'Third'], state.tab, value => { state.tab = value; })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    for (selected, selector) in [
        (1, "visual-tab-root/1-1"),
        (2, "visual-tab-root/1-2"),
        (0, "visual-tab-root/1-0"),
    ] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| {
                assert!(view.error.is_none(), "{:?}", view.error);
                assert_eq!(first_text(&view.tree), selected.to_string());
                let Some(Node::Column { children, .. }) = &view.tree else {
                    panic!("Expected column")
                };
                let Node::Tabs {
                    selected: actual, ..
                } = &children[1]
                else {
                    panic!("Expected tabs")
                };
                assert_eq!(*actual, selected);
            })
            .unwrap();
    }
}

#[gpui::test]
fn slider_drag_rebinds_callbacks_after_each_native_update(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        const state = visual.state({amount: 0});
        return () => visual.column([
            visual.text(String(state.amount)),
            visual.slider('Amount', state.amount, 0, 100, value => { state.amount = value; })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let track = visual.debug_bounds("visual-slider-root/1").unwrap();
    visual.simulate_event(gpui::MouseDownEvent {
        position: point(track.left() + track.size.width * 0.25, track.center().y),
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.run_until_parked();
    visual.simulate_event(gpui::MouseMoveEvent {
        position: point(track.left() + track.size.width * 0.75, track.center().y),
        pressed_button: Some(MouseButton::Left),
        modifiers: Default::default(),
    });
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "75");
        })
        .unwrap();
}

#[gpui::test]
fn failed_handler_is_contained_and_reset_recovers(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        return () => visual.column([
            visual.text('Still here'),
            visual.button('Fail', () => { throw new Error('Expected failure'); })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let fail = visual.debug_bounds("visual-button-root/1").unwrap();
    visual.simulate_click(fail.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.as_deref().unwrap().contains("Expected failure"));
            assert_eq!(first_text(&view.tree), "Still here");
        })
        .unwrap();
    let reset = visual.debug_bounds("visual-reset").unwrap();
    visual.simulate_click(reset.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| assert!(view.error.is_none()))
        .unwrap();
}

#[gpui::test]
fn native_input_and_selection_keep_working_after_state_updates(cx: &mut TestAppContext) {
    cx.update(crate::input::bind);
    let window=cx.update(|cx|cx.open_window(Default::default(),|_,cx|cx.new(|cx|VisualView::new(document(r#"
        const state = visual.state({query:'',selected:0});
        return () => visual.column([
            visual.text(state.query + ':' + state.selected),
            visual.input('Filter',state.query,value=>{state.query=value;},{placeholder:'Type a filter'}),
            visual.select('Mode',['First','Second'],state.selected,value=>{state.selected=value;})
        ]);
    "#),cx))).unwrap());
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let field = visual.debug_bounds("visual-input-root/1").unwrap();
    visual.simulate_click(field.center(), Default::default());
    visual.simulate_input("a");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "a:0");
        })
        .unwrap();
    visual.simulate_input("b");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "ab:0");
        })
        .unwrap();
    let option = visual.debug_bounds("visual-option-root/2-1").unwrap();
    visual.simulate_click(option.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(first_text(&view.tree), "ab:1")
        })
        .unwrap();
    let field = visual.debug_bounds("visual-input-root/1").unwrap();
    visual.simulate_click(field.center(), Default::default());
    visual.simulate_input("c");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "abc:1");
        })
        .unwrap();
}

#[gpui::test]
fn native_scene_orbits_and_reuses_static_projection(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        return () => visual.scene([visual.object({kind:'box'})],{height:240});
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let first = window
        .update(visual, |view, _, _| {
            view.scene_cache["root"]
                .borrow()
                .as_ref()
                .unwrap()
                .projection
                .clone()
        })
        .unwrap();
    visual.refresh().unwrap();
    visual.run_until_parked();
    let second = window
        .update(visual, |view, _, _| {
            view.scene_cache["root"]
                .borrow()
                .as_ref()
                .unwrap()
                .projection
                .clone()
        })
        .unwrap();
    assert!(
        Rc::ptr_eq(&first, &second),
        "Static repaints should reuse projected geometry"
    );
    let bounds = visual.debug_bounds("visual-scene-root").unwrap();
    visual.simulate_event(gpui::MouseDownEvent {
        position: bounds.center(),
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseMoveEvent {
        position: bounds.center() + point(px(60.), px(25.)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Default::default(),
    });
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            let orbit = view.orbits["root"];
            assert!((orbit.yaw - 0.6).abs() < 0.001);
            assert!((orbit.pitch - 0.25).abs() < 0.001);
            let after = view.scene_cache["root"]
                .borrow()
                .as_ref()
                .unwrap()
                .projection
                .clone();
            assert!(!Rc::ptr_eq(&first, &after));
        })
        .unwrap();
}

#[gpui::test]
fn tall_visual_uses_full_height_and_wheel_scrolls_the_conversation(cx: &mut TestAppContext) {
    struct Conversation {
        view: gpui::Entity<VisualView>,
        scroll: gpui::ScrollHandle,
    }
    impl Render for Conversation {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("visual-test-conversation")
                .debug_selector(|| "visual-test-conversation".into())
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(self.view.clone())
        }
    }
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let view = cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
                return () => visual.column(Array.from({length: 40}, (_, i) =>
                    visual.button('Row ' + i, () => {})), {gap: 12});
            "#,
                    ),
                    cx,
                )
            });
            cx.new(|_| Conversation {
                view,
                scroll: gpui::ScrollHandle::new(),
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_window_resize(window.into(), size(px(800.), px(500.)));
    visual.run_until_parked();
    let content = visual.debug_bounds("visual-content").unwrap();
    assert!(
        content.size.height > px(1400.),
        "All forty rows must contribute to the visual's height: {content:?}"
    );
    window
        .update(visual, |conversation, _, _| {
            assert!(conversation.scroll.max_offset().y > px(900.));
            assert_eq!(conversation.scroll.offset().y, px(0.));
        })
        .unwrap();
    let viewport = visual.debug_bounds("visual-test-conversation").unwrap();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: viewport.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-180.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.run_until_parked();
    window
        .update(visual, |conversation, _, _| {
            assert!(
                conversation.scroll.offset().y < px(-100.),
                "Scrolling over a tall visual must move the conversation"
            );
        })
        .unwrap();
    assert_eq!(
        visual.debug_bounds("visual-content").unwrap().size.height,
        content.size.height
    );
}

#[gpui::test]
fn source_and_reset_emit_row_remeasure_events(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| VisualView::new(document("return () => visual.text('Ready');"), cx))
        })
        .unwrap()
    });
    let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    let count = Rc::new(Cell::new(0));
    let event_count = count.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, _: &VisualChanged, _| {
            event_count.set(event_count.get() + 1)
        })
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    count.set(0);
    for selector in ["visual-source", "visual-reset"] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
    }
    assert_eq!(
        count.get(),
        2,
        "Every row-height-affecting action must notify the transcript"
    );
    visual.simulate_window_resize(window.into(), size(px(300.), px(700.)));
    visual.refresh().unwrap();
    visual.run_until_parked();
    assert!(
        count.get() > 2,
        "Responsive width changes must also remeasure the transcript row"
    );
}

#[gpui::test]
fn code_diff_and_callout_blocks_render_natively(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"return () => visual.column([
            visual.code('fn main() {}', {language: 'rust', title: 'main.rs'}),
            visual.diff('one\ntwo\nthree', 'one\n2\nthree', {path: 'a.txt', context: 1}),
            visual.callout('Check this', {title: 'Note'})
        ]);"#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("visual-block-root/0").is_some());
    assert!(visual.debug_bounds("visual-block-root/1").is_some());
}

fn html_document(height: Option<u32>) -> VisualDocument {
    VisualDocument {
        id: "page".into(),
        title: "Page".into(),
        source: "<h1>Hello</h1>".into(),
        format: VisualFormat::Html,
        height,
    }
}

#[gpui::test]
fn html_visual_reserves_its_frame_and_skips_the_script_runtime(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| VisualView::new(html_document(Some(300)), cx))
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let frame = visual
        .debug_bounds("visual-html")
        .expect("HTML visual must reserve a frame");
    assert_eq!(frame.size.height, px(300.));
    assert!(visual.debug_bounds("visual-reset").is_some());
}

#[gpui::test]
fn html_visual_with_an_invalid_height_shows_an_error_instead_of_a_page(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| VisualView::new(html_document(Some(5000)), cx))
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("visual-html").is_none());
}

#[test]
fn html_frames_hold_the_published_height_until_the_page_reports_its_own() {
    use crate::monos::html::Page;
    let waiting = Page::default();
    assert_eq!(html_frame_height(Some(520), waiting), 520.);
    assert_eq!(html_frame_height(None, waiting), DEFAULT_HTML_HEIGHT as f32);
    let measured = |height| Page {
        loaded: true,
        height: Some(height),
    };
    assert_eq!(html_frame_height(Some(520), measured(311.)), 311.);
    assert_eq!(html_frame_height(Some(520), measured(12.)), 80.);
    assert_eq!(html_frame_height(Some(520), measured(5000.)), 900.);
}

#[test]
fn html_frames_ease_to_a_new_height_and_retarget_from_where_they_are() {
    let start = std::time::Instant::now();
    let tween = FrameTween::settled(300.).toward(500., start, false);
    assert_eq!(tween.at(start), 300., "A new height starts from the old one");
    let midway = tween.at(start + FRAME_TWEEN / 2);
    assert!(midway > 400. && midway < 500., "Eases out: {midway}");
    assert!(tween.running(start + FRAME_TWEEN / 2));
    assert_eq!(tween.at(start + FRAME_TWEEN), 500.);
    assert!(!tween.running(start + FRAME_TWEEN));

    let retarget = start + FRAME_TWEEN / 2;
    let back = tween.toward(200., retarget, false);
    assert_eq!(back.at(retarget), midway, "A retarget never jumps");
    assert_eq!(tween.toward(500., retarget, false).start, start);

    let reduced = tween.toward(200., retarget, true);
    assert_eq!(reduced.at(retarget), 200., "Reduced motion skips the glide");
}

#[test]
fn html_frames_keep_their_first_height_as_a_floor_and_shrink_only_after_a_pause() {
    let start = std::time::Instant::now();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let mut hold = FrameHold::default();
    assert_eq!(hold.height(480., false, 600., at(0)), (480., None), "Loading");
    assert_eq!(hold.height(240., true, 600., at(10)).0, 240., "First measurement");

    assert_eq!(hold.height(180., true, 600., at(20)).0, 240., "Never below the floor");
    assert_eq!(hold.height(320., true, 600., at(30)).0, 320., "Grows at once");

    let (shown, recheck) = hold.height(260., true, 600., at(100));
    assert_eq!(shown, 320., "A replay's smaller start doesn't dip");
    assert_eq!(recheck, Some(SHRINK_AFTER));
    assert_eq!(hold.height(320., true, 600., at(400)), (320., None), "Grew back");

    hold.height(260., true, 600., at(500));
    assert_eq!(hold.height(250., true, 600., at(900)).0, 320.);
    assert_eq!(
        hold.height(250., true, 600., at(1500)),
        (250., None),
        "A lower height that lasts is taken, timed from the first drop"
    );

    assert_eq!(hold.height(200., true, 420., at(1600)).0, 200., "A reflow resets the floor");
    assert_eq!(hold.height(150., true, 420., at(1700)).0, 200.);
}
