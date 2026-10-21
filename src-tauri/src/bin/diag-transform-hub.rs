//! Real Tauri hub with temporary settings, in-memory DB and isolated WebView profile.
//! No credentials, audio, global hooks, user history or network providers are loaded.
use tauri::{Emitter, Manager};
use vyrp_lib::{
    settings,
    storage::{
        Db,
        transformations::{SavedPrompt, TransformStatus, Transformation},
    },
    transform,
};

struct ReportPath(std::path::PathBuf);
#[tauri::command]
fn diag_report(app: tauri::AppHandle, report: serde_json::Value) {
    let _ = std::fs::write(
        app.state::<ReportPath>().0.join("ui-result.json"),
        report.to_string(),
    );
}

fn main() {
    if !std::env::args().any(|a| a == "--ack-test-slot") {
        eprintln!("Peça o slot e use --ack-test-slot");
        return;
    }
    let marker = format!("VYRP-V25-HUB-{}", uuid::Uuid::new_v4());
    let dir = std::env::temp_dir().join(&marker);
    std::fs::create_dir(&dir).unwrap();
    println!("TEMP {}", dir.display());
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![diag_report,settings::get_settings,settings::set_settings,transform::transform_status,transform::transform_cancel,transform::transform_prompts,transform::transform_save_prompt,transform::transform_history,transform::transform_delete])
        .setup(move |app| {
            let store=settings::SettingsStore::load(dir.join("settings.json"));
            store.set(settings::Settings{onboarding_done:true,ui_language:"pt-BR".into(),..Default::default()})?;
            app.manage(store);
            let db=Db::open_in_memory()?;
            db.transformations().save_prompt(&SavedPrompt{slot:1,title:"Resumo de teste".into(),instruction:"Resuma em uma frase.".into()})?;
            db.transformations().insert(&Transformation{id:"test-v25".into(),created_at:vyrp_lib::storage::now_ms(),original_text:"Podemos marcar nossa reunião para amanhã às dez horas da manhã?".into(),instruction:"Resuma".into(),transformed_text:"Reunião amanhã às 10h?".into(),prompt_slot:Some(1),status:TransformStatus::Inserted,provider:"fake-local".into()},settings::HistoryRetention::Keep)?;
            app.manage(db);
            transform::init(app.handle());
            app.manage(ReportPath(dir.clone()));
            let script=r#"window.addEventListener('DOMContentLoaded',()=>{let count=0; const timer=setInterval(()=>{const button=[...document.querySelectorAll('[role=tab]')].find(b=>b.textContent==='Transformações'); if(button){button.click();clearInterval(timer);}else if(++count>100)clearInterval(timer);},100);});"#;
            tauri::WebviewWindowBuilder::new(app,"hub",tauri::WebviewUrl::App("index.html".into())).title(&marker).inner_size(1050.,850.).data_directory(dir.join("webview")).initialization_script(script).build()?;
            let handle=app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(150));
                let command=dir.join("ui-command.txt");
                let Ok(value)=std::fs::read_to_string(&command) else {continue};
                let _=std::fs::remove_file(command);
                let Some(window)=handle.get_webview_window("hub") else {break};
                match value.trim() {
                    "history"=>{let _=window.eval("document.querySelector('.transform-entry')?.scrollIntoView({block:'center'})");}
                    "prompt"=>{let _=window.eval("const p=document.querySelector('.transform-prompt'); p.open=true;p.scrollIntoView({block:'center'});");}
                    "en"=>{
                        let store=handle.state::<settings::SettingsStore>();let mut next=store.get();next.ui_language="en".into();
                        if let Ok(next)=store.set(next){let _=handle.emit(settings::EVENT_CHANGED,next);}
                    }
                    "exercise"=>{let _=window.eval(r#"(async()=>{
                        const invoke=window.__TAURI_INTERNALS__.invoke;
                        const p=document.querySelector('.transform-prompt');p.open=true;p.scrollIntoView({block:'center'});
                        const input=p.querySelector('input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,'Prompt salvo no teste');input.dispatchEvent(new Event('input',{bubbles:true}));
                        await new Promise(r=>setTimeout(r,200));
                        [...p.querySelectorAll('button')].find(b=>b.textContent==='Salvar prompt').click();
                        await new Promise(r=>setTimeout(r,400));
                        const prompts=await invoke('transform_prompts');const history=await invoke('transform_history',{offset:0});
                        await invoke('diag_report',{report:{saved:prompts[0].title==='Prompt salvo no teste',slots:prompts.length,diff:document.querySelectorAll('.transform-diff').length,history:history.length}});
                    })().catch(e=>window.__TAURI_INTERNALS__.invoke('diag_report',{report:{error:String(e)}}));"#);}
                    "close"=>{handle.exit(0);break;}
                    _=>{}
                }
            });
            Ok(())
        })
        .run(context).expect("hub de diagnóstico");
}
