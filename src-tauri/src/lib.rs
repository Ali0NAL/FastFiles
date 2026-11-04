mod chat;
mod config;
mod discovery;
mod node;
mod protocol;
mod tls;
pub mod update;

use chat::{ChatMessage, ConversationSummary};
use config::Settings;
use tauri_plugin_notification::NotificationExt;
use node::{Decision, Emitter, Node, Peer, Transfer};
use protocol::{DeviceInfo, DEFAULT_PORT};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter as _, Manager, RunEvent, State, WindowEvent};

type NodeState<'a> = State<'a, Arc<Node>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    device: DeviceInfo,
    ips: Vec<String>,
    settings: Settings,
    peers: Vec<Peer>,
    transfers: Vec<Transfer>,
    conversations: Vec<ConversationSummary>,
    update: UpdateState,
}

#[tauri::command]
fn get_snapshot(node: NodeState) -> Snapshot {
    Snapshot {
        device: node.device_info(),
        ips: discovery::local_ips(),
        settings: node.settings(),
        peers: node.peers(),
        transfers: node.transfers(),
        conversations: node.chat_summaries(),
        update: update_state_of(&node),
    }
}

#[tauri::command]
fn update_settings(
    node: NodeState,
    device_name: Option<String>,
    download_dir: Option<String>,
    close_to_tray: Option<bool>,
    onboarded: Option<bool>,
) -> Result<Settings, String> {
    if let Some(name) = &device_name {
        if name.trim().is_empty() {
            return Err("Cihaz adı boş olamaz".into());
        }
    }
    if let Some(dir) = &download_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("Klasör kullanılamıyor: {e}"))?;
    }
    Ok(node.update_settings(|s| {
        if let Some(v) = device_name {
            s.device_name = v.trim().chars().take(40).collect();
        }
        if let Some(v) = download_dir {
            s.download_dir = v;
        }
        if let Some(v) = close_to_tray {
            s.close_to_tray = v;
        }
        if let Some(v) = onboarded {
            s.onboarded = v;
        }
    }))
}

#[tauri::command]
async fn send_paths(node: NodeState<'_>, peer_ids: Vec<String>, paths: Vec<String>) -> Result<Vec<String>, String> {
    node.inner().send_paths(&peer_ids, paths).await
}

#[tauri::command]
fn chat_conversations(node: NodeState) -> Vec<ConversationSummary> {
    node.chat_summaries()
}

#[tauri::command]
fn chat_history(node: NodeState, fingerprint: String) -> Vec<ChatMessage> {
    node.chat_history(&fingerprint)
}

#[tauri::command]
async fn chat_send(node: NodeState<'_>, peer_id: String, text: String) -> Result<ChatMessage, String> {
    node.inner().chat_send(&peer_id, &text)
}

#[tauri::command]
async fn chat_retry(node: NodeState<'_>, fingerprint: String, id: String) -> Result<(), String> {
    node.inner().chat_retry(&fingerprint, &id)
}

#[tauri::command]
fn chat_mark_read(node: NodeState, fingerprint: String) {
    node.chat_mark_read(&fingerprint);
}

#[tauri::command]
fn chat_clear(node: NodeState, fingerprint: String) {
    node.chat_clear(&fingerprint);
}

// Grup komutları async: giden kutusu tokio görevleri başlatır.
#[tauri::command]
async fn group_create(node: NodeState<'_>, name: String, members: Vec<String>) -> Result<ConversationSummary, String> {
    node.inner().group_create(&name, &members)
}

#[tauri::command]
async fn group_edit(
    node: NodeState<'_>,
    key: String,
    name: Option<String>,
    add: Vec<String>,
    remove: Vec<String>,
) -> Result<ConversationSummary, String> {
    node.inner().group_edit(&key, name, &add, &remove)
}

#[tauri::command]
async fn group_leave(node: NodeState<'_>, key: String) -> Result<(), String> {
    node.inner().group_leave(&key)
}

#[tauri::command]
async fn group_send(node: NodeState<'_>, key: String, text: String) -> Result<ChatMessage, String> {
    node.inner().group_send(&key, &text)
}

#[tauri::command]
fn set_blocked(node: NodeState, fingerprint: String, name: String, blocked: bool) -> Settings {
    node.set_blocked(&fingerprint, &name, blocked)
}

#[tauri::command]
fn respond_incoming(node: NodeState, request_id: String, accept: bool, trust: bool) -> bool {
    node.respond(&request_id, Decision { accept, trust })
}

#[tauri::command]
fn cancel_transfer(node: NodeState, id: String) {
    node.cancel_transfer(&id);
}

#[tauri::command]
fn clear_finished(node: NodeState) -> Vec<Transfer> {
    node.clear_finished()
}

#[tauri::command]
async fn add_manual_peer(node: NodeState<'_>, address: String) -> Result<Peer, String> {
    node.inner().add_manual_peer(&address).await
}

#[tauri::command]
fn forget_manual_peer(node: NodeState, address: String) {
    node.forget_manual_peer(&address);
}

#[tauri::command]
fn set_trusted(node: NodeState, fingerprint: String, name: String, trusted: bool) -> Settings {
    node.set_trusted(&fingerprint, &name, trusted)
}

/// Dosyayı Gezgin'de seçili olarak gösterir.
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path))
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").args(["-R", &path]).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let p = std::path::Path::new(&path);
        let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        std::process::Command::new("xdg-open").arg(dir).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    std::fs::create_dir_all(&path).ok();
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    std::process::Command::new(cmd).arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Pencere görünür ve odakta mı? (Öndeyse uygulama içi bildirim yeterli.)
fn in_front(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(false))
        .unwrap_or(false)
}

/// Pencere arkadaysa Windows bildirimi gösterir ve görev çubuğunu yakıp söndürür.
/// `target`: bildirime tıklanınca arayüzün açacağı yer, ör. {"view":"chat","key":...}.
fn notify_background(app: &AppHandle, title: &str, body: &str, target: serde_json::Value) {
    if in_front(app) {
        return;
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
    show_notification(app, title, body, target);
}

/// Windows: bildirime tıklanınca pencereyi öne getirir ve ilgili ekranı açar.
/// (Bildirim İşlem Merkezi'ne düştükten sonra tıklanırsa Windows uygulamayı yeniden başlatır;
/// tek örnek koruması bu durumda açık pencereyi öne getirir.)
#[cfg(target_os = "windows")]
fn show_notification(app: &AppHandle, title: &str, body: &str, target: serde_json::Value) {
    use tauri_winrt_notification::Toast;
    // Geliştirme derlemesinde uygulama kayıtlı değildir; eklentinin yaptığı gibi PowerShell kimliği kullanılır.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let dev = exe_dir.ends_with("\\target\\debug") || exe_dir.ends_with("\\target\\release");
    let app_id = if dev { Toast::POWERSHELL_APP_ID.to_string() } else { app.config().identifier.clone() };
    let handle = app.clone();
    let (title, body) = (title.to_string(), body.to_string());
    // WinRT çağrısı arayüz iş parçacığını bekletmesin.
    std::thread::spawn(move || {
        let click_handle = handle.clone();
        let result = Toast::new(&app_id)
            .title(&title)
            .text1(&body)
            .on_activated(move |_action| {
                show_main(&click_handle);
                let _ = click_handle.emit("notification-click", target.clone());
                Ok(())
            })
            .show();
        if result.is_err() {
            // Yedek: eklentinin bildirimi (tıklama desteği yok ama en azından görünür).
            let _ = handle.notification().builder().title(&title).body(&body).show();
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn show_notification(app: &AppHandle, title: &str, body: &str, _target: serde_json::Value) {
    let _ = app.notification().builder().title(title).body(body).show();
}

// ───────────────────────────── Güncelleme ─────────────────────────────

/// Hazır güncelleme ilk görüldükten bu kadar sonra (aktarım yoksa) kendiliğinden kurulur.
const AUTO_INSTALL_DELAY: Duration = Duration::from_secs(60);
const POSTPONE: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Default)]
struct UpdateGate {
    postponed_until: Mutex<Option<Instant>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateState {
    current: String,
    /// Kurulmayı bekleyen sürüm
    ready: Option<String>,
    /// Bu cihazın ağda dağıttığı paket sürümü
    serving: Option<String>,
}

fn update_state_of(node: &Node) -> UpdateState {
    UpdateState {
        current: update::CURRENT_VERSION.to_string(),
        ready: node.pending_update(),
        serving: node.cached_package_version(),
    }
}

#[tauri::command]
fn update_state(node: NodeState) -> UpdateState {
    update_state_of(&node)
}

#[tauri::command]
fn import_update(node: NodeState, path: String) -> Result<UpdateState, String> {
    node.import_update(std::path::Path::new(&path))?;
    Ok(update_state_of(&node))
}

#[tauri::command]
fn install_update(app: AppHandle, node: NodeState) -> Result<(), String> {
    start_install(&app, &node)
}

#[tauri::command]
fn postpone_update(gate: State<UpdateGate>) {
    *gate.postponed_until.lock().unwrap() = Some(Instant::now() + POSTPONE);
}

fn start_install(app: &AppHandle, node: &Node) -> Result<(), String> {
    let (setup, version) = node.prepare_install()?;
    let hidden = app.get_webview_window("main").map_or(true, |w| !w.is_visible().unwrap_or(false));
    launch_installer(&setup, hidden)?;
    eprintln!("[fastfiles] {version} kuruluyor, uygulama yeniden başlatılacak");
    app.exit(0);
    Ok(())
}

/// Uygulama kapandıktan sonra kurulumu sessizce çalıştırır ve uygulamayı yeniden açar.
#[cfg(target_os = "windows")]
fn launch_installer(setup: &std::path::Path, hidden: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let flag = if hidden { " --hidden" } else { "" };
    let script = format!(
        "ping -n 3 127.0.0.1 >nul & start \"\" /wait \"{}\" /S & start \"\" \"{}\"{}",
        setup.display(),
        exe.display(),
        flag
    );
    std::process::Command::new("cmd")
        .raw_arg(format!("/S /C \"{script}\""))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Kurulum başlatılamadı: {e}"))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn launch_installer(_setup: &std::path::Path, _hidden: bool) -> Result<(), String> {
    Err("Otomatik kurulum şimdilik yalnızca Windows'ta destekleniyor".into())
}

/// Hazır güncellemeyi, kullanıcı ertelemediyse ve aktarım yoksa kendiliğinden kurar.
fn spawn_auto_updater(app: AppHandle, node: Arc<Node>) {
    tauri::async_runtime::spawn(async move {
        let mut ready_since: Option<(String, Instant)> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let Some(version) = node.pending_update() else {
                ready_since = None;
                continue;
            };
            let since = match &ready_since {
                Some((v, t)) if *v == version => *t,
                _ => {
                    ready_since = Some((version, Instant::now()));
                    continue;
                }
            };
            let postponed = app
                .state::<UpdateGate>()
                .postponed_until
                .lock()
                .unwrap()
                .map_or(false, |t| Instant::now() < t);
            if since.elapsed() < AUTO_INSTALL_DELAY || postponed || node.is_busy() {
                continue;
            }
            if let Err(e) = start_install(&app, &node) {
                eprintln!("[fastfiles] otomatik güncelleme: {e}");
                ready_since = Some((version, Instant::now()));
            }
        }
    });
}

pub fn run() {
    let mut builder = tauri::Builder::default();
    // Aynı kullanıcıda ikinci kez açılırsa yeni süreç başlatma; mevcut pencereyi öne getir.
    // (Aynı kimlikle iki örnek çalışırsa diğer cihazlarda port sürekli değişir.)
    // FASTFILES_PROFILE ile test amaçlı ayrı "cihaz" başlatmaya izin verilir.
    if std::env::var("FASTFILES_PROFILE").is_err() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)));
    }
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // FASTFILES_PROFILE=ikinci ile aynı bilgisayarda ikinci bir "cihaz" çalıştırılabilir (test için).
            let mut config_dir = app.path().app_config_dir()?;
            if let Ok(profile) = std::env::var("FASTFILES_PROFILE") {
                config_dir = config_dir.join(format!("profile-{profile}"));
            }
            let download_dir = app
                .path()
                .download_dir()
                .unwrap_or_else(|_| config_dir.join("downloads"));

            let emit_handle = handle.clone();
            let emitter: Emitter = Arc::new(move |event, payload| {
                #[cfg(debug_assertions)]
                if event != "transfer" {
                    eprintln!("[event] {event}: {payload}");
                }
                match event {
                    // Onay bekleyen gönderim: bildirim + pencereyi öne getir.
                    "incoming" => {
                        let name = payload["device"]["name"].as_str().unwrap_or("Bir cihaz");
                        let count = payload["fileCount"].as_u64().unwrap_or(0);
                        let total = payload["total"].as_u64().unwrap_or(0);
                        notify_background(
                            &emit_handle,
                            &format!("{name} dosya göndermek istiyor"),
                            &format!("{count} dosya · {} — onaylamak için tıklayın", node::fmt_bytes(total)),
                            serde_json::json!({ "view": "transfers" }),
                        );
                        if let Some(w) = emit_handle.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
                        }
                    }
                    // Gelen mesaj
                    "chat" if payload["incoming"] == true => {
                        let name = payload["name"].as_str().unwrap_or("FastFiles");
                        let text: String = payload["message"]["text"].as_str().unwrap_or("").chars().take(140).collect();
                        // Grup mesajında gönderenin adı da görünsün.
                        let body = match payload["message"]["senderName"].as_str() {
                            Some(sender) => format!("{sender}: {text}"),
                            None => text,
                        };
                        let key = payload["fingerprint"].clone();
                        notify_background(&emit_handle, name, &body, serde_json::json!({ "view": "chat", "key": key }));
                    }
                    // Dosya alımı başladı (güvenilir cihaz) / bitti / başarısız
                    "notify" => {
                        notify_background(
                            &emit_handle,
                            payload["title"].as_str().unwrap_or("FastFiles"),
                            payload["body"].as_str().unwrap_or(""),
                            serde_json::json!({ "view": "transfers", "transferId": payload["transferId"] }),
                        );
                    }
                    _ => {}
                }
                let _ = emit_handle.emit(event, payload);
            });

            let node = Node::new(&config_dir, &download_dir, emitter)?;
            tauri::async_runtime::block_on(node.start_server(DEFAULT_PORT))?;
            tauri::async_runtime::spawn(discovery::run(node.clone()));
            tauri::async_runtime::spawn(node.clone().run_manual_pinger());
            app.manage(UpdateGate::default());
            spawn_auto_updater(handle.clone(), node.clone());

            // Güncellemeden sonraki ilk açılışta "güncellendi" bildirimi.
            let previous = node.settings().last_version;
            if previous != update::CURRENT_VERSION {
                node.update_settings(|s| s.last_version = update::CURRENT_VERSION.to_string());
                if !previous.is_empty() {
                    let h = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        let body = format!("{previous} → {}", update::CURRENT_VERSION);
                        show_notification(&h, "FastFiles güncellendi", &body, serde_json::json!({ "view": "main" }));
                        let _ = h.emit("updated", serde_json::json!({ "from": previous, "to": update::CURRENT_VERSION }));
                    });
                }
            }
            app.manage(node);

            // Güncelleme sonrası tepsiden açıldıysa (--hidden) pencereyi gösterme.
            if !std::env::args().any(|a| a == "--hidden") {
                show_main(&handle);
            }

            // Sistem tepsisi
            let show = MenuItem::with_id(app, "show", "FastFiles'ı aç", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Çıkış", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("FastFiles")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = e
                    {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let node = window.state::<Arc<Node>>();
                if node.settings().close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            update_settings,
            send_paths,
            respond_incoming,
            cancel_transfer,
            clear_finished,
            add_manual_peer,
            forget_manual_peer,
            set_trusted,
            set_blocked,
            chat_conversations,
            chat_history,
            chat_send,
            chat_retry,
            chat_mark_read,
            chat_clear,
            group_create,
            group_edit,
            group_leave,
            group_send,
            update_state,
            import_update,
            install_update,
            postpone_update,
            reveal_path,
            open_path,
        ])
        .build(tauri::generate_context!())
        .expect("FastFiles başlatılamadı");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            let node = app.state::<Arc<Node>>();
            node.shutdown.cancel();
            discovery::send_bye(&node);
        }
    });
}
