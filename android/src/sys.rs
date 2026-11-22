//! То, что на Android даёт система: папки приложения, модель телефона, браузер и путь
//! к установленным приложениям. Всё, кроме папок, идёт через JNI.

use jni::objects::{JObject, JString, JValue};
use jni::JNIEnv;
use slint::android::AndroidApp;
use std::path::PathBuf;
use std::sync::OnceLock;

static APP: OnceLock<AndroidApp> = OnceLock::new();

pub fn init(app: &AndroidApp) {
    let _ = APP.set(app.clone());
}

fn files_dir() -> PathBuf {
    APP.get().and_then(|a| a.internal_data_path()).unwrap_or_else(|| PathBuf::from("/data/local/tmp"))
}

/// Папка настроек: личная папка приложения, другим приложениям её не прочесть.
pub fn config_base() -> PathBuf {
    files_dir()
}

/// `files` и `cache` лежат рядом в папке приложения; cache система чистит сама, когда мало места.
pub fn cache_base() -> PathBuf {
    let files = files_dir();
    files.parent().map(|p| p.join("cache")).unwrap_or(files)
}

/// Модель телефона: Яндекс показывает её в списке устройств аккаунта.
pub fn hostname() -> Option<String> {
    with_env(|env, _| {
        let model = env.get_static_field("android/os/Build", "MODEL", "Ljava/lang/String;")?.l()?;
        java_string(env, model)
    })
}

/// Открыть адрес в браузере.
pub fn open_url(url: &str) {
    let Some(url) = crate::client::browser_url(url) else { return };
    let ok = with_env(|env, activity| {
        let text = env.new_string(&url)?;
        let uri = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&text)])?
            .l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;
        env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)])?;
        env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
    if ok.is_none() {
        crate::config::log(format!("не удалось открыть {url}"));
    }
}

/// Путь к APK установленного приложения. Без `<queries>` в манифесте Android 11+ его не покажет.
pub fn package_path(package: &str) -> Option<PathBuf> {
    with_env(|env, activity| {
        let pm = env.call_method(activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[])?.l()?;
        let name = env.new_string(package)?;
        let info = env
            .call_method(
                &pm,
                "getApplicationInfo",
                "(Ljava/lang/String;I)Landroid/content/pm/ApplicationInfo;",
                &[JValue::Object(&name), JValue::Int(0)],
            )?
            .l()?;
        let dir = env.get_field(&info, "sourceDir", "Ljava/lang/String;")?.l()?;
        java_string(env, dir)
    })
    .map(PathBuf::from)
}

fn java_string(env: &mut JNIEnv, obj: JObject) -> jni::errors::Result<String> {
    Ok(env.get_string(&JString::from(obj))?.into())
}

/// Вызов Java из любого потока. Исключение Java гасится и превращается в None.
fn with_env<T>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>) -> Option<T> {
    let app = APP.get()?;
    // SAFETY: указатели на JavaVM и активити живут, пока жив процесс приложения.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let res = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    res.ok()
}
