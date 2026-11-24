//! Settings > Import from Monoptah. The data folders are temporary and the process list is a
//! fake: no test looks at the real Monoptah or zptah data or at the real processes.
use super::*;
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};
use monocode_core::store::monoptah_import::ProcessProbe;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

struct Processes(Vec<String>);
impl ProcessProbe for Processes {
    fn commands(&self) -> Result<Vec<String>, String> {
        Ok(self.0.clone())
    }
}

struct Folders {
    root: PathBuf,
    source: PathBuf,
    target: PathBuf,
}

fn folders(with_monoptah: bool) -> Folders {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "zptah-import-tests-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let source = root.join("com.monoptah.desktop");
    let target = root.join("app.zptah.desktop");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    if with_monoptah {
        drop(monocode_core::store::Store::open_monocode(&source.join("monocode.db")).unwrap());
    }
    Folders {
        root,
        source,
        target,
    }
}
impl Drop for Folders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn open(
    cx: &mut TestAppContext,
    folders: &Folders,
    running: Vec<String>,
) -> WindowHandle<SettingsView> {
    let (source, target) = (folders.source.clone(), folders.target.clone());
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(900.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = SettingsView::new(
                        Section::General,
                        Preferences::default(),
                        SettingsContext {
                            data_dir: Some(target.clone()),
                            ..Default::default()
                        },
                        cx,
                    );
                    view.discovery_task = None;
                    view.discovery_loading = false;
                    view.import =
                        pages::ImportState::for_test(source, &target, Arc::new(Processes(running)));
                    view
                })
            },
        )
        .unwrap()
    });
    cx.run_until_parked();
    window
}

fn settle(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
}

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    visual.simulate_click(bounds.center(), Default::default());
    settle(visual);
}

fn visible(window: &WindowHandle<SettingsView>, visual: &mut VisualTestContext) -> bool {
    window
        .update(visual, |view, _, _| view.import_visible())
        .unwrap()
}

#[gpui::test]
fn the_section_is_hidden_without_monoptah_data(cx: &mut TestAppContext) {
    let none = folders(false);
    let window = open(cx, &none, vec![]);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(!visible(&window, visual));
    // It is also not one of the searchable pages.
    assert!(!Section::ALL.contains(&Section::Import));
    assert!(
        !model::GROUPS
            .iter()
            .any(|(_, s)| s.contains(&Section::Import))
    );
    let found = folders(true);
    let window = open(cx, &found, vec![]);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visible(&window, visual));
}

#[gpui::test]
fn a_running_monoptah_blocks_the_import_with_a_message(cx: &mut TestAppContext) {
    let found = folders(true);
    let window = open(
        cx,
        &found,
        vec!["/Applications/Monoptah Native.app/Contents/MacOS/monocode-gpui".into()],
    );
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            view.select_section(Section::Import, cx)
        })
        .unwrap();
    settle(visual);
    click(visual, "import-start");
    assert!(visual.debug_bounds("import-error").is_some());
    assert!(!found.target.join(".imported-from-monoptah").exists());
    assert!(!found.target.join("zptah.db").exists());
    assert!(visual.debug_bounds("import-restart").is_none());
}

#[gpui::test]
fn a_prepared_import_asks_for_a_restart_and_the_section_goes_away_after_it(
    cx: &mut TestAppContext,
) {
    let found = folders(true);
    let window = open(cx, &found, vec!["/bin/zsh".into()]);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    window
        .update(visual, |view, _, cx| {
            let log = events.clone();
            cx.subscribe(&cx.entity(), move |_, _, event: &SettingsEvent, _| {
                if matches!(event, SettingsEvent::RestartApp) {
                    log.borrow_mut().push("restart");
                }
            })
            .detach();
            view.select_section(Section::Import, cx)
        })
        .unwrap();
    settle(visual);
    click(visual, "import-start");
    // Prepared only: nothing is replaced while zptah runs, and the marker is not written yet.
    assert!(found.target.join(".import-pending").is_file());
    assert!(!found.target.join(".imported-from-monoptah").exists());
    assert!(!found.target.join("zptah.db").exists());
    assert!(visible(&window, visual));
    assert!(visual.debug_bounds("import-start").is_none());
    click(visual, "import-restart");
    assert_eq!(events.borrow().as_slice(), ["restart"]);
    // The next start moves it in.
    monocode_core::store::monoptah_import::apply_pending(&found.target)
        .unwrap()
        .unwrap();
    assert!(found.target.join("zptah.db").is_file());
    // After a restart the marker hides the section for good.
    let reopened = open(cx, &found, vec![]);
    let visual = &mut VisualTestContext::from_window(reopened.into(), cx);
    assert!(!visible(&reopened, visual));
    assert!(Path::new(&found.source).join("monocode.db").is_file());
}
