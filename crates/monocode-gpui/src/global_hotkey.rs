//! macOS application hotkeys via HIToolbox; no accessibility permission needed.
//!
//! One shared event handler serves every hotkey. Each press carries the id it
//! was registered under, and the handler passes on (`eventNotHandledErr`) any id
//! that is not in the registry, so a hotkey never swallows another one's press.
#![cfg_attr(not(all(target_os = "macos", not(test))), allow(dead_code))]
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) use carbon::Registration;
#[cfg(all(target_os = "macos", not(test)))]
mod carbon {
    use super::parse;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    };
    #[repr(C)]
    struct EventType {
        class: u32,
        kind: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct HotKeyId {
        signature: u32,
        id: u32,
    }
    type Handler = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;
    type Notify = Box<dyn Fn() + Send>;
    const EVENT_NOT_HANDLED: i32 = -9874;
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn InstallEventHandler(
            target: *mut c_void,
            handler: Handler,
            count: u32,
            types: *const EventType,
            data: *mut c_void,
            reference: *mut *mut c_void,
        ) -> i32;
        fn GetEventParameter(
            event: *mut c_void,
            name: u32,
            kind: u32,
            actual_kind: *mut u32,
            size: usize,
            actual_size: *mut usize,
            data: *mut c_void,
        ) -> i32;
        fn RegisterEventHotKey(
            code: u32,
            modifiers: u32,
            id: HotKeyId,
            target: *mut c_void,
            options: u32,
            reference: *mut *mut c_void,
        ) -> i32;
        fn UnregisterEventHotKey(reference: *mut c_void) -> i32;
    }
    static NEXT_ID: AtomicU32 = AtomicU32::new(1);
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static REGISTRY: Mutex<Option<HashMap<u32, Notify>>> = Mutex::new(None);
    fn signature() -> u32 {
        u32::from_be_bytes(*b"Mono")
    }
    pub(crate) struct Registration {
        hotkey: *mut c_void,
        id: u32,
    }
    unsafe extern "C" fn pressed(
        _next: *mut c_void,
        event: *mut c_void,
        _data: *mut c_void,
    ) -> i32 {
        let mut id = HotKeyId {
            signature: 0,
            id: 0,
        };
        // kEventParamDirectObject '----', typeEventHotKeyID 'hkid'.
        let error = unsafe {
            GetEventParameter(
                event,
                u32::from_be_bytes(*b"----"),
                u32::from_be_bytes(*b"hkid"),
                std::ptr::null_mut(),
                std::mem::size_of::<HotKeyId>(),
                std::ptr::null_mut(),
                (&mut id as *mut HotKeyId).cast(),
            )
        };
        if error != 0 || id.signature != signature() {
            return EVENT_NOT_HANDLED;
        }
        let registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        match registry.as_ref().and_then(|r| r.get(&id.id)) {
            Some(notify) => {
                notify();
                0
            }
            None => EVENT_NOT_HANDLED,
        }
    }
    /// Installs the shared handler once; `Err` carries the HIToolbox status.
    fn install(target: *mut c_void) -> Result<(), i32> {
        if INSTALLED.load(Ordering::Acquire) {
            return Ok(());
        }
        let event = EventType {
            class: u32::from_be_bytes(*b"keyb"),
            kind: 5,
        };
        let mut handler = std::ptr::null_mut();
        // HIToolbox callbacks and registration are confined to GPUI's main thread.
        let error = unsafe {
            InstallEventHandler(
                target,
                pressed,
                1,
                &event,
                std::ptr::null_mut(),
                &mut handler,
            )
        };
        if error != 0 {
            return Err(error);
        }
        INSTALLED.store(true, Ordering::Release);
        Ok(())
    }
    impl Registration {
        /// Registers `shortcut` (for example `cmd-shift-space`). `notify` runs on
        /// the main thread each time it is pressed; it must not block.
        pub(crate) fn new(
            shortcut: &str,
            name: &str,
            notify: impl Fn() + Send + 'static,
        ) -> Result<Self, String> {
            let (code, mods) = parse(shortcut, name)?;
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let mut hotkey = std::ptr::null_mut();
            unsafe {
                let target = GetApplicationEventTarget();
                if let Err(error) = install(target) {
                    return Err(format!("Could not register {name} ({error})."));
                }
                REGISTRY
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(HashMap::new)
                    .insert(id, Box::new(notify));
                let error = RegisterEventHotKey(
                    code,
                    mods,
                    HotKeyId {
                        signature: signature(),
                        id,
                    },
                    target,
                    1,
                    &mut hotkey,
                );
                if error != 0 {
                    if let Some(r) = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        r.remove(&id);
                    }
                    return Err(
                        "This shortcut is already in use. Choose another in Keybindings.".into(),
                    );
                }
            }
            Ok(Self { hotkey, id })
        }
        /// A registration that reports each press on a channel.
        pub(crate) fn channel(
            shortcut: &str,
            name: &str,
        ) -> Result<(Self, async_channel::Receiver<()>), String> {
            let (tx, rx) = async_channel::bounded(1);
            let registration = Self::new(shortcut, name, move || {
                let _ = tx.try_send(());
            })?;
            Ok((registration, rx))
        }
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            unsafe {
                UnregisterEventHotKey(self.hotkey);
            }
            if let Some(r) = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                r.remove(&self.id);
            }
        }
    }
}
pub(crate) fn parse(shortcut: &str, name: &str) -> Result<(u32, u32), String> {
    let mut modifiers = 0;
    let mut key = None;
    for part in shortcut.split('-') {
        match part {
            "cmd" => modifiers |= 256,
            "shift" => modifiers |= 512,
            "alt" => modifiers |= 2048,
            "ctrl" => modifiers |= 4096,
            v => key = Some(v),
        }
    }
    let key = key.ok_or_else(|| format!("Choose a key for {name}."))?;
    let code = match key {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "equal" => 24,
        "9" => 25,
        "7" => 26,
        "minus" => 27,
        "8" => 28,
        "0" => 29,
        "o" => 31,
        "u" => 32,
        "i" => 34,
        "p" => 35,
        "l" => 37,
        "j" => 38,
        "k" => 40,
        "n" => 45,
        "m" => 46,
        "tab" => 48,
        "space" => 49,
        "backspace" => 51,
        "escape" => 53,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        _ => return Err(format!("This {name} shortcut is not supported.")),
    };
    if modifiers == 0 {
        return Err(format!("{name} needs a modifier key."));
    }
    Ok((code, modifiers))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_modifiers_and_digit_keys() {
        assert_eq!(parse("ctrl-alt-1", "x"), Ok((18, 4096 | 2048)));
        assert_eq!(parse("cmd-shift-space", "x"), Ok((49, 256 | 512)));
        assert_eq!(parse("ctrl-alt-9", "x"), Ok((25, 4096 | 2048)));
    }

    #[test]
    fn rejects_a_missing_modifier_or_key() {
        assert!(parse("1", "Mono").unwrap_err().contains("modifier"));
        assert!(parse("ctrl-alt", "Mono").unwrap_err().contains("key"));
    }
}
