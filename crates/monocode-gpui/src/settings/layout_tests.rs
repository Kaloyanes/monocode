use super::*;
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};

fn window(section: Section, cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(800.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = SettingsView::new(
                        section,
                        Preferences::default(),
                        SettingsContext::default(),
                        cx,
                    );
                    view.discovery_task = None;
                    view.discovery_loading = false;
                    view
                })
            },
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}

#[gpui::test]
fn workspace_style_is_only_offered_under_the_new_layout(cx: &mut TestAppContext) {
    let window = window(Section::Appearance, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for classic in [false, true, false] {
        window
            .update(visual, |view, _, cx| {
                view.ctx.classic = classic;
                view.ctx.new_layout_boxy = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("settings-new-layout-style-boxy")
                .is_some(),
            !classic
        );
        assert_eq!(
            visual
                .debug_bounds("settings-new-layout-style-rounded")
                .is_some(),
            !classic
        );
        assert!(
            visual
                .debug_bounds("settings-collapsed-project-rail-hidden")
                .is_none()
        );
    }
}

#[gpui::test]
fn skills_document_has_its_own_full_height_pane_and_compact_actions(cx: &mut TestAppContext) {
    let window = window(Section::Skills, cx);
    window
        .update(cx, |view, _, cx| {
            let path = std::path::PathBuf::from("/project/.agents/skills/review/SKILL.md");
            view.discovery.skills = vec![data::Skill {
                name: "review".into(),
                description: "Review changes".into(),
                path: path.clone(),
                scope: "project".into(),
                provider: "agents".into(),
            }];
            view.skill_preview = Some(path);
            view.skill_preview_text = Some(Ok(skill_document::DocumentData::new(format!(
                "---\nname: review\n---\n# Review\n\n{}",
                "Instructions.\n\n".repeat(150)
            ))));
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    let body = visual.debug_bounds("skill-document").unwrap();
    assert_eq!(left.size.height, px(800.));
    assert_eq!(right.size.height, px(800.));
    assert!(right.left() >= left.right());
    assert!(body.bottom() <= right.bottom());
    assert!(body.size.height > px(600.));
    assert!(visual.debug_bounds("transcript").is_none());
    for id in [
        "skill-preview-action",
        "skill-copy-action",
        "skill-reveal-action",
    ] {
        assert_eq!(
            visual.debug_bounds(id).unwrap().size,
            size(px(20.), px(20.))
        );
    }
    let source = visual
        .debug_bounds("settings-skill-document-source")
        .unwrap();
    assert!(source.size.width < px(100.));
    visual.simulate_click(source.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.skill_source_paths.len(), 1)
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    // Deliver the frame that applies the measured Skills container breakpoint.
    visual.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    assert!(
        right.top() >= left.bottom(),
        "list={left:?}, preview={right:?}"
    );
    assert_eq!(left.size.width, px(700.));
    assert_eq!(right.size.width, px(700.));
    assert!(visual.debug_bounds("skill-document").unwrap().size.height > px(200.));
}

#[gpui::test]
fn mono_mascot_is_beside_name_and_sidebar_switch_stays_visible(cx: &mut TestAppContext) {
    let window = window(Section::Monos, cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.monos = vec![MonoSetting {
                id: "layout-mono".into(),
                name: "Big Boss".into(),
                mascot: "ghost".into(),
                color: 0xee99bb,
                projects: vec!["monocode".into(), "mono-web".into()],
                show: false,
                folders: false,
            }];
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let mascot = visual.debug_bounds("settings-mono-mascot").unwrap();
    let switch = visual
        .debug_bounds("settings-toggle-mono-sidebar-show")
        .unwrap();
    let reset = visual.debug_bounds("settings-mono-reset").unwrap();
    assert_eq!(mascot.size, size(px(16.), px(16.)));
    assert_eq!(switch.size, size(px(36.), px(20.)));
    let name = visual.debug_bounds("settings-mono-name").unwrap();
    assert!(mascot.right() < name.left());
    assert!(mascot.left() < px(200.));
    assert!(switch.left() > px(800.));
    assert!(switch.right() <= reset.left());
    assert!(reset.right() <= px(1200.));
    assert!((mascot.top() - switch.top()).abs() < px(4.));
    assert!(
        visual
            .debug_bounds("settings-toggle-mono-sidebar-folders")
            .is_none()
    );
    visual.simulate_click(reset.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    window
        .update(visual, |view, window, cx| {
            view.close_management(window, cx);
            cx.notify();
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    let switch = visual
        .debug_bounds("settings-toggle-mono-sidebar-show")
        .unwrap();
    assert_eq!(switch.size, size(px(36.), px(20.)));
    assert!(switch.right() <= px(700.));
}

#[gpui::test]
fn cleanup_offers_removal_only_for_clean_merged_or_pushed_worktrees(cx: &mut TestAppContext) {
    use monocode_core::{git::Worktree, worktree_management::TreeStatus};
    let tree = |name: &str, main: bool| Worktree {
        path: format!("/project-worktrees/{name}"),
        branch: Some(name.into()),
        head: "abcdef0".into(),
        main,
        missing: false,
    };
    let status = |name: &str, edit: &dyn Fn(&mut TreeStatus)| {
        let mut s = TreeStatus {
            tree: tree(name, false),
            locked: false,
            dirty: Some(false),
            unpushed: Some(0),
            unmerged: Some(0),
            has_upstream: false,
            last_commit: Some(1_700_000_000),
        };
        edit(&mut s);
        s
    };
    let window = window(Section::Worktrees, cx);
    window
        .update(cx, |view, _, cx| {
            view.selected_project = "/project".into();
            view.discovery.worktrees.main = "/project".into();
            view.discovery.worktrees.base = Some("feat/gpui".into());
            view.discovery.worktrees.sweep = vec![
                status("project", &|s| s.tree.main = true),
                status("merged", &|_| {}),
                status("dirty", &|s| s.dirty = Some(true)),
                status("ahead", &|s| (s.unmerged, s.unpushed) = (Some(3), Some(3))),
                status("pushed", &|s| {
                    (s.unmerged, s.unpushed, s.has_upstream) = (Some(3), Some(0), true)
                }),
            ];
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("sweep-remove-0").is_none(),
        "main checkout"
    );
    assert!(visual.debug_bounds("sweep-remove-1").is_some(), "merged");
    assert!(visual.debug_bounds("sweep-remove-2").is_none(), "dirty");
    assert!(
        visual.debug_bounds("sweep-remove-3").is_none(),
        "unpushed commits"
    );
    assert!(visual.debug_bounds("sweep-remove-4").is_some(), "pushed");
    let bulk = visual.debug_bounds("sweep-remove-merged").unwrap();
    visual.simulate_click(bulk.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            let Some(management::Editor {
                target: management::Target::Sweep(list),
                ..
            }) = &view.management
            else {
                panic!("the bulk action asks for confirmation first");
            };
            let paths: Vec<_> = list.iter().map(|s| s.tree.path.as_str()).collect();
            assert_eq!(
                paths,
                ["/project-worktrees/merged", "/project-worktrees/pushed"]
            );
        })
        .unwrap();
}
