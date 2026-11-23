use crate::player::Player;
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::ProtocolObject,
    runtime::{AnyObject, NSObjectProtocol},
    sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplicationWillTerminateNotification, NSAutoresizingMaskOptions, NSBezelStyle, NSButton,
    NSImage, NSImageNameExitFullScreenTemplate, NSLayoutAttribute,
    NSTitlebarAccessoryViewController, NSView,
};
use objc2_foundation::{NSNotificationCenter, NSPoint, NSRect, NSSize, NSString};
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
        let button_size = NSSize::new(34.0, 24.0);
        let trailing_padding = 8.0;
        let container = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::ZERO,
                NSSize::new(button_size.width + trailing_padding, button_size.height),
            ),
        );
        button.setFrameSize(button_size);
        button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        container.addSubview(&button);
        accessory.setView(&container);
        accessory.setLayoutAttribute(NSLayoutAttribute::Right);
        window.addTitlebarAccessoryViewController(&accessory);
    });
}

pub struct TerminationObserver {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

pub fn observe_termination(player: &Player) -> TerminationObserver {
    let center = NSNotificationCenter::defaultCenter();
    let player = player.clone();
    let handler = RcBlock::new(move |_| player.save_playback());
    // the notification runs synchronously before AppKit exits the process
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSApplicationWillTerminateNotification),
            None,
            None,
            &handler,
        )
    };
    TerminationObserver { center, token }
}

impl Drop for TerminationObserver {
    fn drop(&mut self) {
        let token: &ProtocolObject<dyn NSObjectProtocol> = &self.token;
        unsafe { self.center.removeObserver(token.as_ref()) };
    }
}
