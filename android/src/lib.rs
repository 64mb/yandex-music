//! Яндекс Музыка RS для Android. Общий с десктопом код (API, вход, плеер волны) берётся
//! из ../src, здесь только окно, системные вызовы и фон «Моей волны».

#[path = "../../src/api.rs"]
#[allow(dead_code)]
mod api;
#[path = "../../src/assets.rs"]
#[allow(dead_code)]
mod assets;
#[path = "../../src/auth.rs"]
#[allow(dead_code)]
mod auth;
#[path = "../../src/client.rs"]
#[allow(dead_code)]
mod client;
#[path = "../../src/config.rs"]
#[allow(dead_code)]
mod config;
#[path = "../../src/eq.rs"]
#[allow(dead_code)]
mod eq;
#[path = "../../src/images.rs"]
#[allow(dead_code)]
mod images;
#[path = "../../src/palette.rs"]
mod palette;
#[path = "../../src/player.rs"]
#[allow(dead_code)]
mod player;
#[path = "../../src/secret.rs"]
mod secret;
#[path = "../../src/stream.rs"]
mod stream;

mod blob;
mod home;
mod mood;
mod original;
mod sys;
mod vector;
mod wave_gl;

use config::{log, Config};
use player::{Cmd, Player, Source, Status};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

slint::include_modules!();

#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    sys::init(&app);
    if let Err(e) = slint::android::init(app) {
        log(format!("slint: {e}"));
        return;
    }
    if let Err(e) = run() {
        log(format!("окно: {e}"));
    }
}

fn run() -> Result<(), slint::PlatformError> {
    let ui = App::new()?;
    let mut apk = original::Apk::open_installed();
    if apk.is_none() {
        log("Яндекс Музыка не установлена: шрифты системные, волна без шейдера");
    }
    let shader = apk.as_mut().and_then(|a| {
        original::register_fonts(a);
        ui.set_icons(load_icons(a));
        original::wave_shader(a)
    });
    drop(apk);

    let wave: Rc<RefCell<Option<wave_gl::Wave>>> = Default::default();
    if let Some(src) = shader {
        install_wave(&ui, src, wave.clone());
    }

    let weak = ui.as_weak();
    let player = Player::spawn(std::sync::Arc::new(move || {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_refresh();
            }
        });
    }));

    // Громкость на телефоне задают кнопки системы, своего ползунка у оригинала нет.
    player.send(Cmd::Volume(1.0));
    if let Some(token) = Config::load().token {
        home::start(&ui, &token);
        player.send(Cmd::Login(token));
        ui.set_logged_in(true);
    } else {
        start_login(&ui, player.clone());
    }

    {
        let p = player.clone();
        ui.on_play_pause(move || p.send(Cmd::PlayPause));
    }
    {
        let p = player.clone();
        ui.on_like(move || {
            if let Some(t) = p.state().track {
                p.send(Cmd::LikeTrack(t));
            }
        });
    }
    {
        let p = player.clone();
        ui.on_play_wheel(move |i| {
            let Some(w) = home::wheel_item(i as usize) else { return };
            let source = match w.station() {
                Some(station) => Source::Seed { station, header: "Моя волна".into(), title: w.name },
                None => Source::Wave,
            };
            p.send(Cmd::Wave(source));
            p.send(Cmd::Resume);
        });
    }
    {
        let weak = ui.as_weak();
        let p = player.clone();
        ui.on_retry_login(move || {
            if let Some(ui) = weak.upgrade() {
                start_login(&ui, p.clone());
            }
        });
    }
    ui.on_open_login(|| sys::open_url("https://ya.ru/device"));

    // Плеер шлёт notify на каждое событие, а позиция трека меняется и без событий.
    let refresh = {
        let weak = ui.as_weak();
        let player = player.clone();
        let wave = wave.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                sync(&ui, &player, &wave);
            }
        }
    };
    ui.on_refresh(refresh.clone());
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), refresh);

    ui.run()
}

/// Состояние плеера → свойства окна и сцена волны.
fn sync(ui: &App, player: &Player, wave: &Rc<RefCell<Option<wave_gl::Wave>>>) {
    let s = player.state();
    home::track_changed(s.track.as_ref().map_or("", |t| t.id.as_str()));
    let title = s.track.as_ref().map(|t| format!("{} — {}", t.artists, t.title)).unwrap_or_default();
    ui.set_track_title(title.into());
    ui.set_playing(s.playing);
    ui.set_loading(s.loading);
    ui.set_liked(s.track.as_ref().is_some_and(|t| player.is_liked(&t.id)));
    ui.set_progress(if s.duration > 0.0 { s.position / s.duration } else { 0.0 });
    let status = match &s.status {
        Status::LoggedOut => "",
        Status::Connecting => "Подключаемся…",
        Status::Ready if s.track.is_none() => "Загружаем волну…",
        Status::Ready => "",
        Status::Failed(e) => e.as_str(),
    };
    let status = s.error.as_ref().map(|(e, _)| e.as_str()).unwrap_or(status);
    ui.set_status(status.into());
    if let Some(w) = wave.borrow_mut().as_mut() {
        w.set_scene(s.track.as_ref().and_then(|t| t.mood), s.playing);
    }
}

/// Вход по коду: показать код, открыть ya.ru/device, ждать подтверждения в фоне.
fn start_login(ui: &App, player: Player) {
    ui.set_logged_in(false);
    ui.set_login_code("".into());
    ui.set_login_hint("Получаем код…".into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let set = |code: String, hint: String| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_login_code(code.into());
                    ui.set_login_hint(hint.into());
                }
            });
        };
        let code = match auth::device_code() {
            Ok(c) => c,
            Err(e) => return set(String::new(), format!("Не удалось получить код: {e}")),
        };
        set(code.user_code.clone(), "Откройте ya.ru/device и введите этот код. Можно на этом же телефоне.".into());
        let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(code.interval));
            match auth::poll(&code.device_code) {
                auth::Poll::Pending => continue,
                auth::Poll::Failed(e) => return set(String::new(), e),
                auth::Poll::Done(token) => {
                    let mut c = Config::load();
                    c.token = Some(token.access.clone());
                    c.refresh_token = token.refresh;
                    c.token_expires = token.expires_in.map(|s| config::now_secs() + s);
                    c.save();
                    player.send(Cmd::Login(token.access.clone()));
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            home::start(&ui, &token.access);
                            ui.set_logged_in(true);
                        }
                    });
                    return;
                }
            }
        }
        set(String::new(), "Код истёк. Откройте приложение заново.".into());
    });
}

fn load_icons(apk: &mut original::Apk) -> Icons {
    let mut icon = |name: &str| {
        apk.icon(name).and_then(|svg| slint::Image::load_from_svg_data(svg.as_bytes()).ok()).unwrap_or_default()
    };
    Icons {
        logo: icon("ic_yandex_music_logo_ru"),
        search: icon("ic_search_24"),
        play: icon("ic_play_32"),
        pause: icon("ic_pause_32"),
        like: icon("ic_like_unactive_24"),
        liked: icon("ic_like_active_24"),
        tab_wave: icon("ic_android_my_wibe"),
        tab_home: icon("ic_landing_default"),
        tab_concerts: icon("ic_android_concerts_light_theme_default"),
        tab_collection: icon("ic_android_my_music_test_default"),
        sparkle: icon("wave_words_sparkle_base"),
        sparkle_ai: icon("wave_words_sparkle_mini_ai"),
    }
}

/// Шейдер волны рисуется перед интерфейсом в каждом кадре, пока окно на экране.
fn install_wave(ui: &App, fragment: String, wave: Rc<RefCell<Option<wave_gl::Wave>>>) {
    let weak = ui.as_weak();
    let res = ui.window().set_rendering_notifier(move |state, api| match state {
        slint::RenderingState::RenderingSetup => {
            let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api else {
                log("волна: рендерер не на OpenGL");
                return;
            };
            // SAFETY: Slint вызывает колбэк с текущим контекстом GL своего окна.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|name| get_proc_address(name)) };
            match unsafe { wave_gl::Wave::new(gl, &fragment) } {
                Ok(w) => *wave.borrow_mut() = Some(w),
                Err(e) => log(format!("волна: {e}")),
            }
        }
        slint::RenderingState::BeforeRendering => {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(w) = wave.borrow_mut().as_mut() {
                let size = ui.window().size();
                unsafe { w.draw(size.width, size.height) };
                ui.window().request_redraw();
            }
        }
        slint::RenderingState::RenderingTeardown => *wave.borrow_mut() = None,
        _ => {}
    });
    if let Err(e) = res {
        log(format!("волна: {e:?}"));
    }
}
