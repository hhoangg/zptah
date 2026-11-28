//! AppKit owns menu navigation and accessibility; actions return to GPUI.
use objc2::runtime::AnyObject;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, sel,
};
use objc2_app_kit::{NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem};
use objc2_foundation::{NSData, NSSize, NSString};
use std::rc::Rc;
struct Action {
    key: String,
    run: Rc<dyn Fn(&str)>,
}
define_class!(
    #[unsafe(super(NSMenuItem))]
    #[name="MonocodeMonoMenuItem"]
    #[thread_kind=MainThreadOnly]
    #[ivars=Action]
    struct Item;
    impl Item{
        #[unsafe(method(runMonoAction:))]
        fn run(&self,_:Option<&AnyObject>){(self.ivars().run)(&self.ivars().key);}
    }
);
impl Item {
    fn new(
        mtm: MainThreadMarker,
        title: &str,
        key: String,
        shortcut: Option<(u8, usize)>,
        run: Rc<dyn Fn(&str)>,
    ) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(Action { key, run });
        let item: Retained<Self> = unsafe {
            msg_send![super(this),initWithTitle:&*NSString::from_str(title),action:Some(sel!(runMonoAction:)),keyEquivalent:&*NSString::from_str("")]
        };
        if let Some((digit, mask)) = shortcut {
            item.setKeyEquivalent(&NSString::from_str(&digit.to_string()));
            item.setKeyEquivalentModifierMask(NSEventModifierFlags::from_bits_retain(mask));
        }
        // The status menu retains each item for as long as it can be invoked.
        unsafe {
            item.setTarget(Some(&item));
        }
        item
    }
}
pub(super) struct Status {
    item: Retained<NSStatusItem>,
    signature: String,
}
impl Status {
    pub(super) fn new() -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.);
        if let Some(button) = item.button(mtm) {
            let data = NSData::with_bytes(include_bytes!(
                "../../../../assets/native/macos/AppIcon.icon/Assets/icon.png"
            ));
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                image.setSize(NSSize::new(18., 18.));
                image.setTemplate(true);
                button.setImage(Some(&image));
            } else {
                button.setTitle(&NSString::from_str("Mono"));
            }
            button.setToolTip(Some(&NSString::from_str("zptah")));
        }
        Some(Self {
            item,
            signature: String::new(),
        })
    }
    /// `chief` is the Chief's name while it is on: its entry, "Ask <name>...", comes first.
    /// `hotkey` is the modifier string of the Mono shortcuts, `None` while off.
    pub(super) fn update(
        &mut self,
        chief: Option<&str>,
        monos: &[(String, String)],
        hotkey: Option<&str>,
        run: Rc<dyn Fn(&str)>,
    ) {
        let signature = format!("{chief:?}{monos:?}:{hotkey:?}");
        if self.signature == signature {
            return;
        }
        self.signature = signature;
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        if let Some(name) = chief {
            menu.addItem(&Item::new(
                mtm,
                &format!("Ask {name}..."),
                super::chief_ui::CHIEF_ID.into(),
                None,
                run.clone(),
            ));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        if monos.is_empty() {
            let title = NSMenuItem::new(mtm);
            title.setTitle(&NSString::from_str("No monos yet"));
            title.setEnabled(false);
            menu.addItem(&title);
        }
        let mask = hotkey.map(super::hotkeys::menu_mask);
        for (i, (id, name)) in monos.iter().enumerate() {
            let shortcut = mask
                .filter(|_| i < super::hotkeys::MAX)
                .map(|mask| (i as u8 + 1, mask));
            let item = Item::new(mtm, name, id.clone(), shortcut, run.clone());
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&Item::new(
            mtm,
            "Quick Composer",
            "composer".into(),
            None,
            run.clone(),
        ));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&Item::new(mtm, "Quit zptah", "quit".into(), None, run));
        self.item.setMenu(Some(&menu));
    }
}
impl Drop for Status {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
