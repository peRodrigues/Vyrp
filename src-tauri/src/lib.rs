//! Vyrp: processo único com threads dedicadas por área (spec §5.1).

pub mod audio;
pub mod context;
pub mod core;
pub mod dictation;
pub mod factory;
pub mod hotkey;
pub mod hub;
pub mod insert;
pub mod learn;
pub mod llm;
pub mod metrics;
pub mod overlay;
pub mod post;
pub mod privacy;
pub mod settings;
pub mod sound;
pub mod storage;
pub mod stt;
pub mod transform;
pub mod tray_i18n;
pub mod updates;
pub mod vad;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager, PhysicalPosition, WindowEvent};
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
            EnvFilter::try_from_env("VYRP_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,whisper_rs=warn")),
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

/// Item "Abrir ao iniciar o Windows" do tray (acompanha o Settings).
struct TrayAutostart(CheckMenuItem<tauri::Wry>);

/// Abriu pelo login do Windows (`--autostart`) ou num diagnóstico: fica só no tray.
fn launched_hidden() -> bool {
    std::env::args().any(|a| a == "--autostart" || a.starts_with("--diag-"))
}

/// Liga/desliga o "abrir ao iniciar o Windows" no sistema conforme o Settings.
fn sync_autostart(app: &AppHandle, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    let result = match launcher.is_enabled() {
        Ok(current) if current == enabled => return,
        _ if enabled => launcher.enable(),
        _ => launcher.disable(),
    };
    match result {
        Ok(()) => tracing::info!(enabled, "abrir ao iniciar o Windows atualizado"),
        Err(err) => tracing::warn!(%err, enabled, "não foi possível mudar o início com o Windows"),
    }
}

/// Grava `autostart` no Settings (o follow_settings aplica no sistema e no tray).
fn toggle_autostart(app: &AppHandle) {
    let store = app.state::<settings::SettingsStore>();
    let mut next = store.get();
    next.autostart = !next.autostart;
    match store.set(next) {
        Ok(saved) => {
            let _ = app.emit(settings::EVENT_CHANGED, &saved);
        }
        Err(err) => tracing::warn!(%err, "autostart não gravado"),
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let lang = tray_i18n::current(app);
    let open = MenuItem::with_id(app, "open", tray_i18n::open(lang), true, None::<&str>)?;
    let autostart_on = app
        .try_state::<settings::SettingsStore>()
        .is_some_and(|s| s.get().autostart);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        tray_i18n::autostart(lang),
        true,
        autostart_on,
        None::<&str>,
    )?;
    let update = MenuItem::with_id(
        app,
        "update",
        tray_i18n::update(lang, &tray_i18n::UpdateLabel::Check),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", tray_i18n::quit(lang), true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &autostart, &update, &separator, &quit])?;
    app.manage(tray_i18n::TrayItems {
        open: open.clone(),
        autostart: autostart.clone(),
        quit: quit.clone(),
    });
    app.manage(TrayAutostart(autostart));
    app.manage(updates::Updates::new(update));
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Vyrp")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_hub(app),
            "autostart" => toggle_autostart(app),
            "update" => updates::on_click(app),
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

/// Liga atalhos globais (hook + gestos) com os atalhos do Settings. Por ora os gestos
/// só vão para o log e para o evento de diagnóstico; o orquestrador (V11) passa a
/// consumi-los.
fn start_hotkeys(app: &AppHandle) {
    #[cfg(windows)]
    {
        let gestures = app.state::<settings::SettingsStore>().get().gestures;
        let emitter = app.clone();
        match hotkey::start(gestures, move |gesture| {
            let _ = emitter.emit(hotkey::EVENT_GESTURE, gesture);
            if let Some(orchestrator) =
                emitter.try_state::<std::sync::Arc<dictation::Orchestrator>>()
            {
                orchestrator.on_gesture(gesture);
            }
        }) {
            Ok(service) => {
                app.manage(service);
            }
            Err(err) => tracing::error!(error = %err, "atalhos globais indisponíveis"),
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

/// Liga a captura de áudio (V7): stream pré-aquecido no microfone do Settings; nível
/// vai para o overlay, avisos para o log.
fn start_audio(app: &AppHandle) {
    let cfg = app.state::<settings::SettingsStore>().get().audio();
    let emitter = app.clone();
    let service = audio::AudioService::start(cfg, move |event| match event {
        audio::AudioEvent::Level(rms) => {
            let _ = emitter.emit(core::types::EVENT_LEVEL, core::types::AudioLevel { rms });
        }
        audio::AudioEvent::Warning(warning) => {
            tracing::warn!(?warning, "aviso de áudio");
        }
        audio::AudioEvent::Ready { .. } => {}
    });
    app.manage(service);
}

/// Repassa mudanças do Settings (`set_settings`) para atalhos e captura, sem reiniciar.
fn follow_settings(app: &AppHandle) {
    let handle = app.clone();
    app.listen(settings::EVENT_CHANGED, move |event| {
        let Ok(next) = serde_json::from_str::<settings::Settings>(event.payload()) else {
            return;
        };
        #[cfg(windows)]
        if let Some(hotkeys) = handle.try_state::<hotkey::HotkeyService>() {
            hotkeys.update(next.gestures.clone());
        }
        if let Some(audio) = handle.try_state::<audio::AudioService>() {
            audio.configure(next.audio());
        }
        if let Some(fx) = handle.try_state::<sound::Fx>() {
            fx.set_params(next.fx());
        }
        sync_autostart(&handle, next.autostart);
        if let Some(item) = handle.try_state::<TrayAutostart>() {
            let _ = item.0.set_checked(next.autostart);
        }
        tray_i18n::apply(&handle);
    });
}

/// Sons/mídia (V19) e o orquestrador do ditado (V11), com contexto e inserção pelas
/// fachadas (`context::default_source`, `insert::default_inserter`).
fn start_dictation(app: &AppHandle) {
    let fx_params = app.state::<settings::SettingsStore>().get().fx();
    let marker_dir = app.path().app_data_dir().ok();
    app.manage(sound::Fx::start(fx_params, marker_dir));
    let context = context::default_source();
    app.manage(dictation::Orchestrator::new(
        app.clone(),
        context.clone(),
        privacy::guard_inserter(app.clone(), insert::default_inserter(), context),
    ));
}

/// `--diag-dictate <segundos>`: simula PttStart, espera e PttEnd (o mesmo caminho do
/// atalho, pelo orquestrador). `--diag-dictate-wav <arquivo>`: o pipeline inteiro com um
/// WAV no lugar do microfone. Os dois colam no app em foco.
fn diag_dictate(app: &AppHandle) {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let secs = value("--diag-dictate").and_then(|v| v.parse::<u64>().ok());
    let wav = value("--diag-dictate-wav");
    if secs.is_none() && wav.is_none() {
        return;
    }
    // Num uso real o Vyrp fica no tray e o foco está no app onde se dita; aqui o hub
    // abriria por cima e receberia o Ctrl+V. Esconder devolve o foco ao app anterior.
    if let Some(hub) = app.get_webview_window(HUB) {
        let _ = hub.hide();
    }
    let app = app.clone();
    let value = move |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1).cloned())
    };
    std::thread::spawn(move || {
        // Espera o modelo e o VAD ficarem prontos, como num uso normal depois do boot.
        let engine = &app.state::<stt::LocalStt>().0;
        for _ in 0..240 {
            if engine.status().state != stt::whisper::LoadState::Loading
                && engine.status().state != stt::whisper::LoadState::None
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        // Regra de segurança: diagnóstico só injeta na janela de teste aberta pelo próprio
        // teste (HWND + título). Sem os dois, não há ditado de diagnóstico.
        let target = value("--diag-target-hwnd")
            .and_then(|v| v.parse::<isize>().ok())
            .zip(value("--diag-target"));
        let Some((hwnd, title_contains)) = target else {
            tracing::error!(
                "diag: informe --diag-target-hwnd <hwnd> e --diag-target <título>; nada foi feito"
            );
            return;
        };
        let orchestrator = dictation::Orchestrator::new(
            app.clone(),
            context::diag_source(hwnd, title_contains.clone()),
            insert::diag_inserter(insert::DiagTarget {
                hwnd,
                title_contains,
            }),
        );
        if let Some(path) = wav {
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| audio::wav::decode_to_16k_mono(&b))
            {
                Ok(pcm) => {
                    tracing::info!(%path, "diag: ditado com WAV");
                    orchestrator.dictate_clip(core::types::AudioClip::new(pcm));
                }
                Err(err) => tracing::error!(%path, error = %err, "diag: WAV inválido"),
            }
        } else if let Some(secs) = secs {
            tracing::info!(secs, "diag: ditado simulado (PttStart)");
            orchestrator.on_gesture(core::types::GestureEvent::PttStart);
            std::thread::sleep(std::time::Duration::from_secs(secs));
            tracing::info!("diag: ditado simulado (PttEnd)");
            orchestrator.on_gesture(core::types::GestureEvent::PttEnd);
        }
    });
}

/// `--diag-record <segundos>`: grava do microfone e salva um WAV de diagnóstico na
/// pasta de logs, com o resumo no log (teste da captura sem o orquestrador).
fn diag_record(app: &AppHandle) {
    let mut args = std::env::args().skip_while(|a| a != "--diag-record");
    if args.next().is_none() {
        return;
    }
    let secs: u64 = args
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
        .clamp(1, 60);
    let app = app.clone();
    std::thread::spawn(move || {
        use core::types::{DictationState, EVENT_STATE};
        // Dá tempo do stream pré-aquecido abrir.
        std::thread::sleep(std::time::Duration::from_secs(1));
        let audio = app.state::<audio::AudioService>();
        tracing::info!(secs, devices = ?audio.devices(), "diag: gravando");
        let _ = app.emit(EVENT_STATE, DictationState::Listening { locked: false });
        audio.begin();
        std::thread::sleep(std::time::Duration::from_secs(secs));
        let result = audio.end();
        let _ = app.emit(EVENT_STATE, DictationState::Idle);
        match result {
            Ok(clip) => {
                let peak = clip.pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                let rms = audio::dsp::rms(&clip.pcm);
                let path = app
                    .path()
                    .app_log_dir()
                    .unwrap_or_default()
                    .join(format!("diag-{}.wav", std::process::id()));
                let saved = audio::wav::write(&path, &clip.pcm, clip.sample_rate);
                transcribe_for_diag(&app, &clip);
                tracing::info!(
                    ms = clip.duration_ms(),
                    samples = clip.pcm.len(),
                    rate = clip.sample_rate,
                    rms = format_args!("{rms:.4}"),
                    peak,
                    wav = %path.display(),
                    saved = saved.is_ok(),
                    "diag: gravação concluída"
                );
            }
            Err(err) => tracing::error!(error = %err, "diag: gravação falhou"),
        }
    });
}

/// Espera o modelo local ficar pronto (até 2 min) e transcreve o clipe no log.
fn transcribe_for_diag(app: &AppHandle, clip: &core::types::AudioClip) {
    use stt::whisper::LoadState;
    let engine = &app.state::<stt::LocalStt>().0;
    for _ in 0..240 {
        if engine.status().state != LoadState::Loading {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let settings = app.state::<settings::SettingsStore>().get();
    let opts = core::types::SttOptions {
        languages: settings.dictation_languages.clone(),
        vocabulary: Vec::new(),
    };
    let status = engine.status();
    // Mesmo caminho do ditado: fábrica do Settings (VAD → política local/nuvem).
    let stt = factory::build_stt(&settings, app);
    match tauri::async_runtime::block_on(stt.transcribe(clip, &opts)) {
        Ok(t) => tracing::info!(
            model = ?status.model,
            backend = status.backend,
            audio_ms = clip.duration_ms(),
            decode_ms = t.timings.decode_ms,
            language = ?t.language,
            text = %t.text,
            "diag: transcrição"
        ),
        Err(err) => tracing::error!(error = %err, "diag: transcrição falhou"),
    }
}

/// `--diag-model <id>`: baixa (com verificação SHA-256) e ativa um modelo pelo mesmo
/// caminho do command `model_download`, com o progresso no log.
fn diag_model(app: &AppHandle) {
    let mut args = std::env::args().skip_while(|a| a != "--diag-model");
    if args.next().is_none() {
        return;
    }
    let Some(id) = args.next() else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let seen = last.clone();
        let progress = app.listen(stt::EVENT_MODEL_PROGRESS, move |event| {
            if let Ok(p) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                let pct = p["downloaded"].as_u64().unwrap_or(0) * 100
                    / p["total"].as_u64().unwrap_or(1).max(1);
                if pct >= seen.load(std::sync::atomic::Ordering::Relaxed) + 10 {
                    seen.store(pct, std::sync::atomic::Ordering::Relaxed);
                    tracing::info!(pct, "diag: download do modelo");
                }
            }
        });
        let selected = {
            let store = app.state::<settings::SettingsStore>();
            let mut next = store.get();
            next.stt_model = id.clone();
            store.set(next)
        };
        if let Err(err) = selected {
            tracing::error!(%err, "diag: modelo inválido");
            return;
        }
        let result = stt::model_download(app.clone(), id.clone()).await;
        app.unlisten(progress);
        let _ = last;
        tracing::info!(model = %id, ok = result.is_ok(), secs = started.elapsed().as_secs(), "diag: modelo");
    });
}

/// `--diag-transcribe <arquivo.wav>`: transcreve um WAV (PCM 16-bit) e registra no log.
fn diag_transcribe(app: &AppHandle) {
    let mut args = std::env::args().skip_while(|a| a != "--diag-transcribe");
    if args.next().is_none() {
        return;
    }
    let Some(path) = args.next() else { return };
    let app = app.clone();
    std::thread::spawn(move || {
        let pcm = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| audio::wav::decode_to_16k_mono(&bytes));
        match pcm {
            Ok(pcm) => {
                tracing::info!(%path, "diag: transcrevendo arquivo");
                transcribe_for_diag(&app, &core::types::AudioClip::new(pcm));
            }
            Err(err) => tracing::error!(%path, error = %err, "diag: WAV inválido"),
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Tem de ser o primeiro plugin: a segunda instância só traz o hub para a frente.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_hub(app)
        }))
        // Abrir no login do Windows: entrada em HKCU\...\Run com `--autostart`.
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--autostart"])
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            app_info,
            transform::transform_status,
            transform::transform_cancel,
            transform::transform_prompts,
            transform::transform_save_prompt,
            transform::transform_history,
            transform::transform_delete,
            transform::transform_undo,
            context::context_rules,
            context::set_context_rules,
            insert::insert_copy_last,
            stt::model_list,
            stt::model_download,
            stt::model_status,
            stt::model_select,
            settings::get_settings,
            settings::set_settings,
            settings::secrets::set_api_key,
            settings::secrets::clear_api_key,
            settings::secrets::has_api_key,
            storage::dictionary_commands::dictionary_list,
            storage::dictionary_commands::dictionary_save,
            storage::dictionary_commands::dictionary_delete,
            storage::dictionary_commands::dictionary_set_favorite,
            storage::dictionary_commands::dictionary_export_csv,
            storage::dictionary_commands::dictionary_import_csv,
            hub::history_list,
            hub::history_delete,
            hub::history_clear,
            hub::history_clear_audio,
            hub::list_microphones,
            hub::settings_defaults,
            hub::audio_monitor_start,
            hub::audio_monitor_stop,
            hub::dictation_test_start,
            hub::dictation_test_stop,
            hub::provider_test,
            hub::metrics_report,
            hub::diagnostics_text,
            hub::export_log,
            hub::open_log_dir,
            overlay::overlay_demo,
            learn::learn_suggestions,
            learn::learn_accept,
            learn::learn_dismiss,
            learn::learn_undo,
        ])
        .setup(|app| {
            let handle = app.handle();
            if let Err(err) = init_logging(handle) {
                eprintln!("log desativado: {err}");
            }
            settings::init(handle)?;
            context::init(handle);
            if let Err(err) = storage::init(handle) {
                tracing::error!(error = %err, "banco de dados indisponível");
            }
            post::init(handle);
            learn::init(handle);
            transform::init(handle);
            build_tray(handle)?;
            place_overlay(handle)?;
            overlay::init(handle);
            start_hotkeys(handle);
            start_audio(handle);
            if let Err(err) = vad::init(handle) {
                tracing::error!(error = %err, "VAD indisponível");
            }
            if let Err(err) = stt::init(handle) {
                tracing::error!(error = %err, "transcrição local indisponível");
            }
            start_dictation(handle);
            follow_settings(handle);
            sync_autostart(
                handle,
                handle.state::<settings::SettingsStore>().get().autostart,
            );
            // O hub nasce escondido (tauri.conf): no login do Windows e nos diagnósticos o
            // Vyrp fica só no tray, sem pegar o foco de quem está trabalhando.
            if launched_hidden() {
                tracing::info!("iniciado sem janela (tray)");
            } else {
                show_hub(handle);
            }
            if !std::env::args().any(|a| a.starts_with("--diag-")) {
                updates::start(handle);
            }
            diag_record(handle);
            diag_model(handle);
            diag_transcribe(handle);
            diag_dictate(handle);
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
