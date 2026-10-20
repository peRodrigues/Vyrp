//! Vyrp: processo único com threads dedicadas por área (spec §5.1).

pub mod audio;
pub mod context;
pub mod core;
pub mod hotkey;
pub mod insert;
pub mod llm;
pub mod metrics;
pub mod post;
pub mod settings;
pub mod storage;
pub mod stt;
pub mod vad;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, WindowEvent};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

pub const HUB: &str = "hub";
pub const OVERLAY: &str = "overlay";
/// Distância do overlay até a borda inferior da área de trabalho, em pixels lógicos.
const OVERLAY_BOTTOM_MARGIN: f64 = 48.0;

/// Mantém o writer de log vivo enquanto o app roda.
struct LogGuard(#[allow(dead_code)] WorkerGuard);

#[derive(serde::Serialize)]
struct AppInfo {
    version: String,
    log_dir: String,
}

#[tauri::command]
fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        log_dir: app
            .path()
            .app_log_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

fn init_logging(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_log_dir()?;
    std::fs::create_dir_all(&dir)?;
    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&dir, "vyrp.log"));
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("VYRP_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(writer)
        .with_ansi(false)
        .init();
    app.manage(LogGuard(guard));
    tracing::info!(version = %app.package_info().version, log_dir = %dir.display(), "Vyrp iniciado");
    Ok(())
}

fn show_hub(app: &AppHandle) {
    if let Some(hub) = app.get_webview_window(HUB) {
        let _ = hub.unminimize();
        let _ = hub.show();
        let _ = hub.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Abrir Vyrp", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Vyrp")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_hub(app),
            "quit" => {
                tracing::info!("saindo pelo tray");
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_hub(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Centraliza o overlay na borda inferior da área de trabalho do monitor principal.
fn place_overlay(app: &AppHandle) -> tauri::Result<()> {
    let Some(overlay) = app.get_webview_window(OVERLAY) else {
        return Ok(());
    };
    let Some(monitor) = overlay.primary_monitor()? else {
        return Ok(());
    };
    let area = monitor.work_area();
    let size = overlay.outer_size()?;
    let margin = (OVERLAY_BOTTOM_MARGIN * monitor.scale_factor()) as i32;
    let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - margin;
    overlay.set_position(PhysicalPosition::new(x, y))?;
    // Por enquanto o overlay é só indicador: cliques passam para o app de baixo.
    overlay.set_ignore_cursor_events(true)?;
    overlay.show()?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Tem de ser o primeiro plugin: a segunda instância só traz o hub para a frente.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_hub(app)
        }))
        .invoke_handler(tauri::generate_handler![app_info])
        .setup(|app| {
            let handle = app.handle();
            if let Err(err) = init_logging(handle) {
                eprintln!("log desativado: {err}");
            }
            build_tray(handle)?;
            place_overlay(handle)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Fechar o hub esconde; o app continua no tray.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == HUB
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("falha ao iniciar o Vyrp");
}
