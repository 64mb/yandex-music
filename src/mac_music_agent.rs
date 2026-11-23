use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::{
    msg_send,
    rc::{autoreleasepool, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    NSArray, NSBundle, NSError, NSNotification, NSOperationQueue, NSRunLoop, NSString, NSURL,
};

use crate::mac_window::PLAY_URL;

const AGENT_PLIST: &str = "ru.spolzermod.yamusic-rs.apple-music.plist";
const APP_ID: &str = "ru.spolzermod.yamusic-rs";
const MUSIC_ID: &str = "com.apple.Music";

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

pub enum Status {
    Unavailable,
    Disabled,
    RequiresApproval,
    Enabled,
}

pub fn status() -> Status {
    autoreleasepool(|_| {
        app_service().map_or(Status::Unavailable, |service| service_status(&service))
    })
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    autoreleasepool(|_| {
        let service = app_service().ok_or_else(unavailable_message)?;
        match (enabled, service_status(&service)) {
            (_, Status::Unavailable) => return Err(unavailable_message()),
            (true, Status::Enabled | Status::RequiresApproval) | (false, Status::Disabled) => {
                return Ok(());
            }
            _ => {}
        }

        let result: Result<(), Retained<NSError>> = unsafe {
            if enabled {
                msg_send![&*service, registerAndReturnError: _]
            } else {
                msg_send![&*service, unregisterAndReturnError: _]
            }
        };
        match result {
            Err(_) if enabled && matches!(service_status(&service), Status::RequiresApproval) => {
                Ok(())
            }
            result => result.map_err(|error| error.localizedDescription().to_string()),
        }
    })
}

fn unavailable_message() -> String {
    "Перенаправление Apple Music доступно в установленном приложении на macOS 13 и новее".to_owned()
}

fn app_service() -> Option<Retained<AnyObject>> {
    // look up [SMAppService] at runtime so the executable also loads on macOS versions before 13
    let class = AnyClass::get(c"SMAppService")?;
    let bundle = NSBundle::mainBundle();
    if !has_bundle_id(&bundle, APP_ID) {
        return None;
    }
    let path = bundle.bundleURL().path()?.to_string();
    if !std::path::Path::new(&path)
        .join("Contents/Library/LaunchAgents")
        .join(AGENT_PLIST)
        .is_file()
    {
        return None;
    }
    let name = NSString::from_str(AGENT_PLIST);
    unsafe { msg_send![class, agentServiceWithPlistName: &*name] }
}

fn service_status(service: &AnyObject) -> Status {
    let status: isize = unsafe { msg_send![service, status] };
    match status {
        // a packaged service can report [SMAppServiceStatusNotFound] before registration
        0 | 3 => Status::Disabled,
        1 => Status::Enabled,
        2 => Status::RequiresApproval,
        _ => Status::Unavailable,
    }
}

fn has_bundle_id(bundle: &NSBundle, expected: &str) -> bool {
    bundle
        .bundleIdentifier()
        .is_some_and(|id| *id == *NSString::from_str(expected))
}

pub fn run() {
    if !matches!(status(), Status::Enabled) {
        return;
    }

    let handled_pid = Rc::new(Cell::new(None));
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let callback = RcBlock::new({
        let handled_pid = handled_pid.clone();
        move |notification: NonNull<NSNotification>| {
            autoreleasepool(|_| {
                let app = unsafe { notification.as_ref() }
                    .userInfo()
                    .and_then(|info| info.objectForKey(unsafe { NSWorkspaceApplicationKey }))
                    .and_then(|app| app.downcast::<NSRunningApplication>().ok());
                if let Some(app) = app {
                    redirect_music_launch(app, &handled_pid);
                }
            });
        }
    });
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidLaunchApplicationNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &callback,
        )
    };
    autoreleasepool(|_| {
        let music = NSString::from_str(MUSIC_ID);
        for app in NSRunningApplication::runningApplicationsWithBundleIdentifier(&music).iter() {
            redirect_music_launch(app, &handled_pid);
        }
    });
    NSRunLoop::currentRunLoop().run();
    unsafe { center.removeObserver(observer.as_ref()) };
}

fn redirect_music_launch(app: Retained<NSRunningApplication>, handled_pid: &Cell<Option<i32>>) {
    if !app
        .bundleIdentifier()
        .is_some_and(|id| *id == *NSString::from_str(MUSIC_ID))
    {
        return;
    }

    let pid = app.processIdentifier();
    if app.isTerminated() || handled_pid.replace(Some(pid)) == Some(pid) {
        return;
    }
    let workspace = NSWorkspace::sharedWorkspace();
    let target = NSBundle::mainBundle();
    let Some(play_url) = NSURL::URLWithString(&NSString::from_str(PLAY_URL)) else {
        return;
    };
    if !has_bundle_id(&target, APP_ID) {
        return;
    }

    let config = NSWorkspaceOpenConfiguration::configuration();
    let opened = RcBlock::new(
        move |player: *mut NSRunningApplication, error: *mut NSError| {
            // the completion handler receives these objects for the duration of the call
            let (player, error) = unsafe { (player.as_ref(), error.as_ref()) };
            if let Some(error) = error {
                crate::config::log(format!(
                    "media: failed to open Yandex Music from the Apple Music agent: {}",
                    error.localizedDescription(),
                ));
            } else if !player.is_some_and(|player| {
                u32::try_from(player.processIdentifier()).is_ok_and(|pid| pid != std::process::id())
            }) {
                crate::config::log(
                    "media: the Apple Music agent could not reach the player process",
                );
            } else if !app.isTerminated() && !app.forceTerminate() {
                crate::config::log("media: the Apple Music agent could not close Apple Music");
            }
        },
    );
    workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
        &NSArray::from_slice(&[&*play_url]),
        &target.bundleURL(),
        &config,
        Some(&opened),
    );
}
