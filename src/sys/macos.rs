//! adapted from [rnikitin/yamusic-rs] at [8d62260]

use slint::winit_030::WinitWindowAccessor;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

pub fn created(w: &slint::Window) -> bool {
    w.with_winit_window(|_| ()).is_some()
}

pub fn prepare(_: &slint::Window) {}

pub fn raise(w: &slint::Window) {
    w.set_minimized(false);
    w.with_winit_window(|w| w.focus_window());
}

pub fn scale(w: &slint::Window) -> f32 {
    w.with_winit_window(|w| w.scale_factor() as f32)
        .unwrap_or(1.0)
}

pub fn work_area(w: &slint::Window) -> (i32, i32, i32, i32) {
    w.with_winit_window(|w| {
        let m = w.current_monitor()?;
        let (p, s) = (m.position(), m.size());
        Some((p.x, p.y, p.x + s.width as i32, p.y + s.height as i32))
    })
    .flatten()
    .unwrap_or_else(|| {
        let s = w.size();
        (0, 0, s.width as i32, s.height as i32)
    })
}

pub fn pick_image(_: &slint::Window, title: &str) -> Option<String> {
    let t = title.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("POSIX path of (choose file with prompt \"{t}\" of type {{\"public.jpeg\", \"public.png\"}})");
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn choose_folder_script(title: &str, start: &str) -> String {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let def = if !start.is_empty() && std::path::Path::new(start).exists() {
        format!(" default location (POSIX file \"{}\")", esc(start))
    } else {
        String::new()
    };
    format!(
        "POSIX path of (choose folder with prompt \"{}\"{def})",
        esc(title)
    )
}

pub fn pick_folder(_: &slint::Window, title: &str, start: &str) -> Option<String> {
    let script = choose_folder_script(title, start);
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(if s.len() > 1 {
        s.trim_end_matches('/').to_string()
    } else {
        s
    })
}

pub fn open_url(url: &str) {
    if let Ok(mut child) = Command::new("open")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || child.wait());
    }
}

pub fn trim_memory() {}

pub fn release_memory() {
    trim_memory();
}

pub fn system_light_theme() -> bool {
    Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .is_ok_and(|o| !o.status.success())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

pub fn config_base() -> PathBuf {
    home().join("Library").join("Application Support")
}

pub fn cache_base() -> PathBuf {
    config_base()
}

pub fn music_dir() -> PathBuf {
    home().join("Music")
}

pub fn hostname() -> Option<String> {
    let out = Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    let name = String::from_utf8(out.stdout).ok()?;
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

pub fn copy_text(text: &str) {
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

const LAUNCH_LABEL: &str = "ru.spolzermod.yamusic-rs";

fn launch_agent_path() -> Option<PathBuf> {
    let p = home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCH_LABEL}.plist"));
    std::fs::create_dir_all(p.parent()?).ok()?;
    Some(p)
}

pub fn autostart_enabled() -> bool {
    launch_agent_path().is_some_and(|p| p.exists())
}

pub fn set_autostart(on: bool) -> bool {
    let Some(p) = launch_agent_path() else {
        return false;
    };
    if !on {
        return match std::fs::remove_file(&p) {
            Ok(()) => true,
            Err(_) => !p.exists(),
        };
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LAUNCH_LABEL}</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n",
        exe.to_string_lossy().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    );
    std::fs::write(&p, plist).is_ok()
}

pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}

static INSTANCE_LOCK: OnceLock<File> = OnceLock::new();
static WAKE_SOCKET: Mutex<Option<UnixDatagram>> = Mutex::new(None);

fn socket_path() -> PathBuf {
    std::env::temp_dir().join("yamusic-rs.sock")
}

pub fn claim_instance() -> bool {
    let base = config_base();
    let Ok(()) = std::fs::create_dir_all(&base) else {
        return true;
    };
    let Ok(lock) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(base.join("yamusic-rs.lock"))
    else {
        return true;
    };
    match lock.try_lock() {
        Ok(()) => {
            let _ = INSTANCE_LOCK.set(lock);
            let path = socket_path();
            let _ = std::fs::remove_file(&path);
            *WAKE_SOCKET.lock().unwrap() = UnixDatagram::bind(path).ok();
            true
        }
        Err(std::fs::TryLockError::WouldBlock) => false,
        Err(_) => true,
    }
}

pub fn wake_existing() {
    if let Ok(socket) = UnixDatagram::unbound() {
        let _ = socket.send_to(b"show", socket_path());
    }
}

pub fn on_wake(f: impl Fn() + Send + 'static) {
    let Some(socket) = WAKE_SOCKET.lock().unwrap().take() else {
        return;
    };
    std::thread::spawn(move || {
        let mut command = [0; 4];
        while let Ok(len) = socket.recv(&mut command) {
            if len == command.len() && &command == b"show" {
                f();
            }
        }
    });
}

pub fn cleanup() {
    if INSTANCE_LOCK.get().is_some() {
        let _ = std::fs::remove_file(socket_path());
    }
}

#[cfg(test)]
mod tests {
    use super::choose_folder_script;

    #[test]
    fn missing_default_location_is_omitted() {
        let s = choose_folder_script("Папка", "/definitely/not/exists/xyz");
        assert!(
            !s.contains("default location"),
            "диалог упадёт на несуществующем пути: {s}"
        );
    }

    #[test]
    fn existing_default_location_is_used() {
        let dir = std::env::temp_dir();
        let s = choose_folder_script("Папка", dir.to_str().expect("temp"));
        assert!(
            s.contains("default location"),
            "стартовая папка потерялась: {s}"
        );
    }
}
