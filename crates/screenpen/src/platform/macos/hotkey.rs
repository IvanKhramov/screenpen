//! System-wide hotkeys through Carbon `RegisterEventHotKey`, which needs no
//! Accessibility or Input Monitoring permission. Carbon calls the handler from the
//! main run loop.

use std::ffi::c_void;
use std::ptr::null_mut;

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type Handler = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Default)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetEventDispatcherTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: Handler,
        count: u32,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hotkey: EventHotKeyRef) -> OSStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        kind: u32,
        out_kind: *mut u32,
        size: usize,
        out_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
}

const fn four_cc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SIGNATURE: u32 = four_cc(b"SPEN");
const EVENT_CLASS_KEYBOARD: u32 = four_cc(b"keyb");
const EVENT_HOTKEY_PRESSED: u32 = 5;
const PARAM_DIRECT_OBJECT: u32 = four_cc(b"----");
const TYPE_HOTKEY_ID: u32 = four_cc(b"hkid");

/// Registered hotkeys; a press calls `super::on_hotkey` with the id given to `register`.
pub struct HotKeys {
    registered: Vec<EventHotKeyRef>,
}

extern "C" fn handler(_: EventHandlerCallRef, event: EventRef, _: *mut c_void) -> OSStatus {
    let mut id = EventHotKeyID::default();
    let status = unsafe {
        GetEventParameter(
            event,
            PARAM_DIRECT_OBJECT,
            TYPE_HOTKEY_ID,
            null_mut(),
            size_of::<EventHotKeyID>(),
            null_mut(),
            (&mut id as *mut EventHotKeyID).cast(),
        )
    };
    if status == 0 && id.signature == SIGNATURE {
        super::on_hotkey(id.id);
    }
    status
}

impl HotKeys {
    /// Installs the event handler. Call once, on the main thread.
    pub fn new() -> HotKeys {
        let spec = EventTypeSpec {
            class: EVENT_CLASS_KEYBOARD,
            kind: EVENT_HOTKEY_PRESSED,
        };
        unsafe {
            InstallEventHandler(
                GetEventDispatcherTarget(),
                handler,
                1,
                &spec,
                null_mut(),
                null_mut(),
            );
        }
        HotKeys {
            registered: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        for hotkey in self.registered.drain(..) {
            unsafe { UnregisterEventHotKey(hotkey) };
        }
    }

    /// False when another app already holds the combination.
    pub fn register(&mut self, id: u32, code: u16, modifiers: u32) -> bool {
        let mut hotkey: EventHotKeyRef = null_mut();
        let status = unsafe {
            RegisterEventHotKey(
                code as u32,
                modifiers,
                EventHotKeyID {
                    signature: SIGNATURE,
                    id,
                },
                GetEventDispatcherTarget(),
                0,
                &mut hotkey,
            )
        };
        if status != 0 || hotkey.is_null() {
            return false;
        }
        self.registered.push(hotkey);
        true
    }
}

impl Drop for HotKeys {
    fn drop(&mut self) {
        self.clear();
    }
}
