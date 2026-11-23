use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol},
    sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSImage, NSImageNameExitFullScreenTemplate, NSLayoutAttribute,
    NSTitlebarAccessoryViewController, NSView,
};
use objc2_foundation::{NSSize, NSString};
use slint::winit_030::{
    winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
    WinitWindowAccessor,
};

struct MiniPlayerAction(Box<dyn Fn()>);

define_class!(
    // the window retains the controller and dispatches its action on the main thread
    #[unsafe(super = NSTitlebarAccessoryViewController)]
    #[thread_kind = MainThreadOnly]
    #[ivars = MiniPlayerAction]
    struct MiniPlayerAccessory;

    impl MiniPlayerAccessory {
        #[unsafe(method(toggleMiniPlayer:))]
        fn toggle_mini_player(&self, _sender: &AnyObject) {
            (self.ivars().0)();
        }
    }
);

impl MiniPlayerAccessory {
    fn new(mtm: MainThreadMarker, toggle: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MiniPlayerAction(Box::new(toggle)));
        unsafe { msg_send![super(this), init] }
    }
}

pub fn install_mini_player_button(window: &slint::Window, toggle: impl Fn() + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    window.with_winit_window(move |window| {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // winit keeps the [NSView] alive for the lifetime of the borrowed window handle
        let view = unsafe { &*handle.ns_view.cast::<NSView>().as_ptr() };
        let Some(window) = view.window() else { return };

        if window
            .titlebarAccessoryViewControllers()
            .iter()
            .any(|controller| controller.isKindOfClass(MiniPlayerAccessory::class()))
        {
            return;
        }

        let label = NSString::from_str(&crate::i18n::tr("Мини-плеер"));
        let supports_symbols = NSImage::class()
            .metaclass()
            .responds_to(sel!(imageWithSystemSymbolName:accessibilityDescription:));
        let image = supports_symbols
            .then(|| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("pip"),
                    Some(&label),
                )
            })
            .flatten()
            .or_else(|| NSImage::imageNamed(unsafe { NSImageNameExitFullScreenTemplate }));
        let Some(image) = image else { return };
        let accessory = MiniPlayerAccessory::new(mtm, toggle);
        let button = unsafe {
            NSButton::buttonWithImage_target_action(
                &image,
                Some(&accessory),
                Some(sel!(toggleMiniPlayer:)),
                mtm,
            )
        };
        button.setToolTip(Some(&label));
        button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        button.setFrameSize(NSSize::new(34.0, 24.0));
        accessory.setView(&button);
        accessory.setLayoutAttribute(NSLayoutAttribute::Right);
        window.addTitlebarAccessoryViewController(&accessory);
    });
}
