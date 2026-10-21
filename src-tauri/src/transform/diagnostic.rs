//! Executable integration check. Called only with an owned diagnostic window.
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

pub fn run(target: crate::insert::DiagTarget, undo: bool) -> Result<serde_json::Value, String> {
    let privacy = crate::privacy::Privacy::default();
    let selected = selection::selected(privacy.clone(), Some(target.clone()))?;
    let original = selected.text.clone();
    let server = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let address = server.local_addr().map_err(|e| e.to_string())?;
    let expected = original.clone();
    std::thread::spawn(move || {
        let (mut stream, _) = server.accept().expect("local diagnostic server");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                return;
            }
            bytes.extend_from_slice(&buf[..n]);
            if bytes.len() > 128 * 1024 {
                return;
            }
            if let Some(i) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..i]).to_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if bytes.len() >= i + 4 + len {
                    let body: serde_json::Value =
                        serde_json::from_slice(&bytes[i + 4..i + 4 + len]).unwrap();
                    let user: serde_json::Value =
                        serde_json::from_str(body["messages"][1]["content"].as_str().unwrap())
                            .unwrap();
                    assert_eq!(user["selection"], expected);
                    assert_eq!(user["instruction"], "Convert to uppercase");
                    break;
                }
            }
        }
        let body=serde_json::json!({"message":{"content":expected.to_uppercase()},"done":true,"done_reason":"stop"}).to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    struct NoKeys;
    impl crate::stt::cloud::KeyProvider for NoKeys {
        fn api_key(&self, _: crate::settings::secrets::Provider) -> Result<Option<String>, String> {
            Ok(None)
        }
    }
    let cleaner = LlmCleaner::new(
        crate::llm::LlmKind::Ollama,
        Arc::new(NoKeys),
        LlmConfig {
            timeout: Duration::from_secs(3),
            url: Some(format!("http://{address}/api/chat")),
            model: Some("fake-v25".into()),
        },
    );
    let text = tauri::async_runtime::block_on(cleaner.transform(&original, "Convert to uppercase"))
        .map_err(|e| e.to_string())?;
    let outcome = selection::replace(selected.clone(), text.clone(), privacy);
    if !outcome.ok || outcome.verified != Some(true) {
        return Err(format!("colagem não verificada: {outcome:?}"));
    }
    let after = crate::context::snapshot_target(target.hwnd, Some(&target.title_contains));
    if after.hwnd != target.hwnd {
        return Err("foco mudou após colagem; não ler outra janela".into());
    }
    let db = Db::open_in_memory().map_err(|e| e.to_string())?;
    let entry = Transformation {
        id: selected.id.clone(),
        created_at: now_ms(),
        original_text: original.clone(),
        instruction: "Convert to uppercase".into(),
        transformed_text: text.clone(),
        prompt_slot: Some(1),
        status: TransformStatus::Inserted,
        provider: "fake-local".into(),
    };
    db.transformations()
        .insert(&entry, crate::settings::HistoryRetention::Keep)
        .map_err(|e| e.to_string())?;
    let mut undone = None;
    if undo {
        let result = selection::undo(selected.id.clone());
        if !result.ok {
            return Err(format!("undo recusado: {result:?}"));
        }
        std::thread::sleep(Duration::from_millis(150));
        let restored = crate::context::snapshot_target(target.hwnd, Some(&target.title_contains));
        if restored.hwnd != target.hwnd {
            return Err("foco mudou após Undo".into());
        }
        let full = format!(
            "{}{}{}",
            restored.context.before, restored.context.selection, restored.context.after
        );
        if !full.contains(&original) {
            return Err("Undo não restaurou o texto selecionado".into());
        }
        db.transformations()
            .mark_undone(&selected.id)
            .map_err(|e| e.to_string())?;
        undone = Some(result);
    }
    Ok(
        serde_json::json!({"original":original,"transformed":text,"insert":outcome,"after":after.context,"undo":undone,"history":db.transformations().list(1,0).map_err(|e|e.to_string())?}),
    )
}
