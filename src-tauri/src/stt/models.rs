//! Modelos no disco: pasta no AppData, download com progresso e verificação SHA-256.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use super::catalog::ModelSpec;

/// Pasta dos modelos e downloads em andamento.
pub struct ModelStore {
    dir: PathBuf,
    downloading: Mutex<HashSet<&'static str>>,
}

impl ModelStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            downloading: Mutex::new(HashSet::new()),
        }
    }

    pub fn path(&self, spec: &ModelSpec) -> PathBuf {
        self.dir.join(spec.file)
    }

    /// Instalado = arquivo final com o tamanho do catálogo. O hash é conferido no
    /// download (e o arquivo só ganha o nome final depois disso); reconferir 500 MB a
    /// cada boot atrasaria o "modelo quente".
    pub fn is_installed(&self, spec: &ModelSpec) -> bool {
        std::fs::metadata(self.path(spec)).is_ok_and(|m| m.len() == spec.size_bytes)
    }

    pub fn is_downloading(&self, spec: &ModelSpec) -> bool {
        self.downloading.lock().unwrap().contains(spec.id)
    }

    /// Baixa o modelo (um download por id de cada vez). `on_progress(baixado, total)`.
    pub async fn download(
        &self,
        spec: &'static ModelSpec,
        on_progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, String> {
        if !self.downloading.lock().unwrap().insert(spec.id) {
            return Err(format!("o modelo {} já está sendo baixado", spec.id));
        }
        let dest = self.path(spec);
        let result = download_to(
            &spec.url(),
            &dest,
            spec.size_bytes,
            spec.sha256,
            on_progress,
        )
        .await;
        self.downloading.lock().unwrap().remove(spec.id);
        result.map(|()| dest)
    }
}

/// Download em fluxo para `<dest>.part`, com SHA-256 calculado no caminho; só renomeia
/// para `dest` se tamanho e hash baterem. Falha apaga o parcial.
pub async fn download_to(
    url: &str,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension("part");
    let outcome = async {
        let response = reqwest::get(url)
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("download falhou: {e}"))?;
        let total = response.content_length().unwrap_or(expected_size);
        let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        let mut throttle = ProgressThrottle::new(total);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("download interrompido: {e}"))?;
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            hasher.update(&chunk);
            done += chunk.len() as u64;
            if throttle.should_emit(done) {
                on_progress(done, total);
            }
        }
        file.flush().map_err(|e| e.to_string())?;
        drop(file);
        if done != expected_size {
            return Err(format!(
                "tamanho errado: {done} bytes, esperado {expected_size}"
            ));
        }
        let got = hex(&hasher.finalize());
        if !got.eq_ignore_ascii_case(expected_sha256) {
            return Err(format!("SHA-256 não confere: {got}"));
        }
        std::fs::rename(&part, dest).map_err(|e| e.to_string())
    }
    .await;
    if outcome.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    outcome
}

/// Avisa a cada 1% (e sempre no fim), para não inundar a UI de eventos.
struct ProgressThrottle {
    total: u64,
    step: u64,
    next: u64,
}

impl ProgressThrottle {
    fn new(total: u64) -> Self {
        let step = (total / 100).max(1);
        Self {
            total,
            step,
            next: 0,
        }
    }

    fn should_emit(&mut self, done: u64) -> bool {
        if done >= self.next || done >= self.total {
            self.next = done + self.step;
            true
        } else {
            false
        }
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Servidor HTTP mínimo que entrega `body` uma vez.
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(&body);
            }
        });
        format!("http://{addr}/modelo.bin")
    }

    fn temp_dest(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("vyrp-models-{}", std::process::id()))
            .join(name)
    }

    #[test]
    fn sha256_hex_of_known_input() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn download_verifies_and_renames() {
        let body: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let sha = hex(&Sha256::digest(&body));
        let url = serve_once(body.clone());
        let dest = temp_dest("ok.bin");
        let mut last = (0, 0);
        tauri::async_runtime::block_on(download_to(
            &url,
            &dest,
            body.len() as u64,
            &sha,
            |d, t| last = (d, t),
        ))
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(last, (body.len() as u64, body.len() as u64));
        assert!(!dest.with_extension("part").exists());
    }

    #[test]
    fn wrong_hash_is_rejected_and_cleaned() {
        let body = vec![7u8; 1_000];
        let url = serve_once(body.clone());
        let dest = temp_dest("bad.bin");
        let err = tauri::async_runtime::block_on(download_to(
            &url,
            &dest,
            1_000,
            &"0".repeat(64),
            |_, _| {},
        ))
        .unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(!dest.exists() && !dest.with_extension("part").exists());
    }

    #[test]
    fn throttle_emits_about_once_per_percent() {
        let mut t = ProgressThrottle::new(10_000);
        let emitted = (1..=10_000u64)
            .step_by(10)
            .filter(|&d| t.should_emit(d))
            .count();
        assert!((95..=105).contains(&emitted), "{emitted}");
        assert!(t.should_emit(10_000), "o fim sempre sai");
    }
}
