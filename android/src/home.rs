//! Под плашкой трека на экране волны: факт о треке и колесо волн. Данные качаются в фоне,
//! состояние живёт в UI-потоке.

use crate::api::{self, Api, WheelItem};
use crate::blob::{self, Blobs};
use crate::config::log;
use crate::images::Images;
use crate::{App, WheelCard};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct Home {
    ui: slint::Weak<App>,
    api: Arc<Api>,
    images: Images,
    blobs: Blobs,
    wheel: Vec<WheelItem>,
    /// Трек, для которого показан или качается факт.
    fact_track: String,
}

thread_local! {
    static HOME: RefCell<Option<Home>> = const { RefCell::new(None) };
}

fn with_home<R>(f: impl FnOnce(&mut Home) -> R) -> Option<R> {
    HOME.with(|h| h.borrow_mut().as_mut().map(f))
}

/// Вошли в аккаунт: завести API и загрузить колесо.
pub fn start(ui: &App, token: &str) {
    let api = Arc::new(Api::new(token));
    HOME.with(|h| {
        let mut h = h.borrow_mut();
        let images = h.take().map_or_else(|| Images::new(false), |old| old.images);
        *h = Some(Home { ui: ui.as_weak(), api: api.clone(), images, blobs: Blobs::default(), wheel: Vec::new(), fact_track: String::new() });
    });
    std::thread::spawn(move || {
        let wheel = api.wheel();
        let _ = slint::invoke_from_event_loop(move || match wheel {
            Ok(items) => {
                with_home(|h| h.set_wheel(items));
            }
            Err(e) => log(format!("колесо: {e}")),
        });
    });
}

/// Карточка колеса по номеру.
pub fn wheel_item(i: usize) -> Option<WheelItem> {
    with_home(|h| h.wheel.get(i).cloned()).flatten()
}

/// Заиграл другой трек: убрать старый факт и спросить новый.
pub fn track_changed(id: &str) {
    with_home(|h| {
        if h.fact_track == id {
            return;
        }
        h.fact_track = id.to_string();
        if let Some(ui) = h.ui.upgrade() {
            ui.set_fact("".into());
        }
        if id.is_empty() {
            return;
        }
        let (api, id) = (h.api.clone(), id.to_string());
        std::thread::spawn(move || {
            let fact = api.track_fact(&id);
            let _ = slint::invoke_from_event_loop(move || {
                with_home(|h| match fact {
                    Ok(Some(text)) if h.fact_track == id => {
                        if let Some(ui) = h.ui.upgrade() {
                            ui.set_fact(text.into());
                        }
                    }
                    Ok(_) => {}
                    Err(e) => log(format!("факт о треке: {e}")),
                });
            });
        });
    });
}

impl Home {
    fn set_wheel(&mut self, items: Vec<WheelItem>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let rows: Vec<WheelCard> = items
            .iter()
            .map(|w| WheelCard {
                name: w.name.clone().into(),
                description: w.description.clone().into(),
                cover: Default::default(),
                round: w.round,
                blob: self.blobs.get(blob::tint(w.color)),
            })
            .collect();
        let model = Rc::new(VecModel::from(rows));
        let px = (64.0 * ui.window().scale_factor()).round() as u32;
        for (i, w) in items.iter().enumerate() {
            let Some(uri) = &w.cover else { continue };
            let m = model.clone();
            self.images.get(&api::pic(uri, px), px, move |img| {
                if let Some(mut row) = m.row_data(i) {
                    row.cover = img.clone();
                    m.set_row_data(i, row);
                }
            });
        }
        ui.set_wheel(ModelRc::from(model));
        // У оригинала по центру вторая карточка, первая выглядывает слева.
        if items.len() > 1 {
            ui.set_wheel_offset(-ui.get_wheel_spacing());
        }
        self.wheel = items;
    }
}
