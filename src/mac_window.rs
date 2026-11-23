use crate::config::MiniPlacement;
use crate::player::{Cmd, Player};
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::ProtocolObject,
    runtime::{AnyObject, NSObject, NSObjectProtocol},
    sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSApplicationWillTerminateNotification,
    NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSImage, NSImageNameExitFullScreenTemplate,
    NSLayoutAttribute, NSScreen, NSTitlebarAccessoryViewController, NSView, NSWindow,
};
use objc2_foundation::{
    ns_string, NSAppleEventDescriptor, NSAppleEventManager, NSArray, NSNotificationCenter,
    NSNotificationName, NSNumber, NSPoint, NSRect, NSSize, NSString,
};
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

fn appkit_window(window: &slint::Window) -> Option<Retained<NSWindow>> {
    window.with_winit_window(|window| {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // winit keeps the [NSView] alive for the lifetime of the borrowed window handle
        let view = unsafe { &*handle.ns_view.cast::<NSView>().as_ptr() };
        view.window()
    })?
}

fn display_id(screen: &NSScreen) -> Option<u32> {
    let number = screen
        .deviceDescription()
        .objectForKey(ns_string!("NSScreenNumber"))?;
    Some(number.downcast_ref::<NSNumber>()?.unsignedIntValue())
}

pub fn mini_placement(window: &slint::Window) -> Option<MiniPlacement> {
    let window = appkit_window(window)?;
    let screen = window.screen()?;
    let frame = window.frame();
    placement_at(&screen, NSPoint::new(frame.origin.x, frame.max().y))
}

fn placement_at(screen: &NSScreen, top_left: NSPoint) -> Option<MiniPlacement> {
    let visible = screen.visibleFrame();
    Some(MiniPlacement {
        display: display_id(screen)?,
        offset: [top_left.x - visible.origin.x, visible.max().y - top_left.y],
    })
}

fn contains(frame: NSRect, point: NSPoint) -> bool {
    (frame.origin.x..frame.max().x).contains(&point.x)
        && point.y > frame.origin.y
        && point.y <= frame.max().y
}

fn legacy_placement(
    screens: &NSArray<NSScreen>,
    pos: slint::PhysicalPosition,
) -> Option<MiniPlacement> {
    let top = screens.firstObject()?.frame().max().y;
    screens.iter().find_map(|screen| {
        let scale = screen.backingScaleFactor();
        let point = NSPoint::new(f64::from(pos.x) / scale, top - f64::from(pos.y) / scale);
        contains(screen.frame(), point)
            .then(|| placement_at(&screen, point))
            .flatten()
    })
}

pub fn restore_mini_placement(
    window: &slint::Window,
    saved: Option<MiniPlacement>,
    legacy: Option<slint::PhysicalPosition>,
) {
    let (Some(mtm), Some(window)) = (MainThreadMarker::new(), appkit_window(window)) else {
        return;
    };
    let screens = NSScreen::screens(mtm);
    let saved = saved.or_else(|| legacy.and_then(|position| legacy_placement(&screens, position)));
    let saved_screen = saved
        .filter(|placement| placement.offset.iter().all(|offset| offset.is_finite()))
        .and_then(|placement| {
            let screen = screens
                .iter()
                .find(|screen| display_id(screen) == Some(placement.display))?;
            Some((screen, placement.offset))
        });
    let frame = window.frame();
    let origin = if let Some((screen, [x, y])) = saved_screen {
        mini_origin(screen.visibleFrame(), frame.size, x, y)
    } else {
        // the first screen contains the menu bar on the primary display
        let screen = if saved.is_none() && legacy.is_none() {
            window.screen().or_else(|| screens.firstObject())
        } else {
            screens.firstObject()
        };
        let Some(screen) = screen else { return };
        const GAP: f64 = 16.0;
        let visible = screen.visibleFrame();
        mini_origin(
            visible,
            frame.size,
            visible.size.width - frame.size.width - GAP,
            visible.size.height - frame.size.height - GAP,
        )
    };
    window.setFrameOrigin(origin);
}

pub fn keep_window_visible(window: &slint::Window) {
    let (Some(mtm), Some(window)) = (MainThreadMarker::new(), appkit_window(window)) else {
        return;
    };
    let screens = NSScreen::screens(mtm);
    let frame = window.frame();
    let title_center = NSPoint::new(frame.mid().x, frame.max().y);
    if screens
        .iter()
        .any(|screen| contains(screen.frame(), title_center))
    {
        return;
    }
    if let Some(primary) = screens.firstObject() {
        let visible = primary.visibleFrame();
        let x = (visible.size.width - frame.size.width) / 2.0;
        let y = (visible.size.height - frame.size.height) / 2.0;
        window.setFrameOrigin(mini_origin(visible, frame.size, x, y));
    }
}

fn mini_origin(visible: NSRect, size: NSSize, x: f64, y: f64) -> NSPoint {
    let x = x.clamp(0.0, (visible.size.width - size.width).max(0.0));
    let y = y.clamp(0.0, (visible.size.height - size.height).max(0.0));
    NSPoint::new(visible.origin.x + x, visible.max().y - size.height - y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_origin_uses_top_left_on_displays_with_negative_coordinates() {
        let visible = NSRect::new(NSPoint::new(-1440.0, -900.0), NSSize::new(1440.0, 877.0));
        let size = NSSize::new(400.0, 108.0);
        assert_eq!(
            mini_origin(visible, size, 32.0, 64.0),
            NSPoint::new(-1408.0, -195.0),
        );
    }

    #[test]
    fn mini_origin_clamps_offsets_to_fit_window() {
        let visible = NSRect::new(NSPoint::new(100.0, 30.0), NSSize::new(1200.0, 800.0));
        let size = NSSize::new(400.0, 108.0);
        assert_eq!(
            mini_origin(visible, size, 1100.0, 790.0),
            NSPoint::new(900.0, 30.0),
        );
        assert_eq!(
            mini_origin(visible, size, -20.0, -80.0),
            NSPoint::new(100.0, 722.0),
        );
    }
}

pub fn install_mini_player_button(window: &slint::Window, toggle: impl Fn() + 'static) {
    if let (Some(mtm), Some(window)) = (MainThreadMarker::new(), appkit_window(window)) {
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
    }
}

pub struct ApplicationObserver {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

pub fn observe_termination(callback: impl Fn() + 'static) -> ApplicationObserver {
    // the notification runs synchronously before AppKit exits the process
    observe_application(unsafe { NSApplicationWillTerminateNotification }, callback)
}

pub fn observe_screen_changes(callback: impl Fn() + 'static) -> ApplicationObserver {
    observe_application(
        unsafe { NSApplicationDidChangeScreenParametersNotification },
        callback,
    )
}

fn observe_application(
    name: &NSNotificationName,
    callback: impl Fn() + 'static,
) -> ApplicationObserver {
    let center = NSNotificationCenter::defaultCenter();
    let handler = RcBlock::new(move |_| callback());
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &handler)
    };
    ApplicationObserver { center, token }
}

impl Drop for ApplicationObserver {
    fn drop(&mut self) {
        let token: &ProtocolObject<dyn NSObjectProtocol> = &self.token;
        unsafe { self.center.removeObserver(token.as_ref()) };
    }
}

const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
pub(crate) const PLAY_URL: &str = "yamusic-rs://media-play";

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Player]
    pub struct PlayUrlHandler;

    impl PlayUrlHandler {
        #[unsafe(method(handlePlayUrl:withReplyEvent:))]
        fn handle_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let url = event
                .paramDescriptorForKeyword(DIRECT_OBJECT)
                .and_then(|object| object.stringValue());
            if url.as_deref().is_some_and(|url| url == &*NSString::from_str(PLAY_URL)) {
                crate::config::log("media: play requested through the macOS URL handler");
                self.ivars().send(Cmd::Resume);
            }
        }
    }
);

pub fn handle_play_url(player: &Player) -> Retained<PlayUrlHandler> {
    let mtm = MainThreadMarker::new().expect("the player runs on the main thread");
    let handler = PlayUrlHandler::alloc(mtm).set_ivars(player.clone());
    let handler: Retained<PlayUrlHandler> = unsafe { msg_send![super(handler), init] };
    unsafe {
        NSAppleEventManager::sharedAppleEventManager()
            .setEventHandler_andSelector_forEventClass_andEventID(
                &handler,
                sel!(handlePlayUrl:withReplyEvent:),
                GET_URL,
                GET_URL,
            );
    }
    handler
}
