use crate::api::Track;
use crate::player::{Cmd, Player, Shared};
use block2::RcBlock;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist, MPMediaItemPropertyPlaybackDuration,
    MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingInfoPropertyElapsedPlaybackTime,
    MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState, MPRemoteCommand,
    MPRemoteCommandCenter, MPRemoteCommandHandlerStatus,
};
use std::cell::RefCell;
use std::time::{Duration, Instant};

struct Published {
    track_id: String,
    playing: bool,
    duration: f32,
    position: f32,
    at: Instant,
}

type Registration = (Retained<MPRemoteCommand>, Retained<AnyObject>);

struct MediaControls {
    targets: Vec<Registration>,
    published: Option<Published>,
}

thread_local! {
    static CONTROLS: RefCell<Option<MediaControls>> = const { RefCell::new(None) };
}

pub fn install(player: &Player) {
    uninstall();
    let targets = unsafe {
        let center = MPRemoteCommandCenter::sharedCommandCenter();
        let commands: [(_, fn() -> Cmd); 5] = [
            (center.playCommand(), || Cmd::Resume),
            (center.pauseCommand(), || Cmd::Pause),
            (center.togglePlayPauseCommand(), || Cmd::PlayPause),
            (center.nextTrackCommand(), || Cmd::Next),
            (center.previousTrackCommand(), || Cmd::Prev),
        ];
        commands
            .into_iter()
            .map(|(command, action)| {
                let player = player.clone();
                let handler = RcBlock::new(move |_| {
                    player.send(action());
                    MPRemoteCommandHandlerStatus::Success
                });
                let target = command.addTargetWithHandler(&handler);
                command.setEnabled(true);
                (command, target)
            })
            .collect()
    };
    CONTROLS.with_borrow_mut(|controls| {
        *controls = Some(MediaControls {
            targets,
            published: None,
        });
    });
}

pub fn sync(track: Option<&Track>, state: &Shared) {
    CONTROLS.with_borrow_mut(|controls| {
        let Some(controls) = controls else { return };
        let Some(track) = track else {
            if controls.published.take().is_some() {
                clear_now_playing();
            }
            return;
        };
        if controls.published.as_ref().is_some_and(|last| {
            let elapsed = last.at.elapsed();
            let position_current = if state.playing {
                let expected = last.position + elapsed.as_secs_f32();
                (expected - state.position).abs() < 0.75 && elapsed < Duration::from_secs(1)
            } else {
                last.position == state.position
            };
            last.track_id == track.id
                && last.playing == state.playing
                && last.duration == state.duration
                && position_current
        }) {
            return;
        }

        unsafe {
            let info = NSDictionary::<NSString, AnyObject>::from_retained_objects(
                &[
                    MPMediaItemPropertyTitle,
                    MPMediaItemPropertyArtist,
                    MPMediaItemPropertyAlbumTitle,
                    MPMediaItemPropertyPlaybackDuration,
                    MPNowPlayingInfoPropertyElapsedPlaybackTime,
                    MPNowPlayingInfoPropertyPlaybackRate,
                ],
                &[
                    NSString::from_str(&track.title).into(),
                    NSString::from_str(&track.artists).into(),
                    NSString::from_str(&track.album_title).into(),
                    NSNumber::numberWithDouble(state.duration.into()).into(),
                    NSNumber::numberWithDouble(state.position.into()).into(),
                    NSNumber::numberWithDouble(if state.playing { 1.0 } else { 0.0 }).into(),
                ],
            );
            let center = MPNowPlayingInfoCenter::defaultCenter();
            center.setNowPlayingInfo(Some(&info));
            let playback = if state.playing {
                MPNowPlayingPlaybackState::Playing
            } else {
                MPNowPlayingPlaybackState::Paused
            };
            if center.playbackState() != playback {
                center.setPlaybackState(playback);
            }
        }
        controls.published = Some(Published {
            track_id: track.id.clone(),
            playing: state.playing,
            duration: state.duration,
            position: state.position,
            at: Instant::now(),
        });
    });
}

fn clear_now_playing() {
    unsafe {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
        center.setNowPlayingInfo(None);
    }
}

pub fn uninstall() {
    if let Some(controls) = CONTROLS.with_borrow_mut(Option::take) {
        if controls.published.is_some() {
            clear_now_playing();
        }
        for (command, target) in controls.targets {
            unsafe { command.removeTarget(Some(&target)) };
        }
    }
}
