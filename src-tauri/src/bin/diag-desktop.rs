//! Manual integration diagnostic. Never inject into a pre-existing window.
//! Run only with the app test slot released by the Maestro:
//! diag-desktop notepad|textbox|chrome|edge|edge-password|terminal context|paste|type|guard|copy --ack-test-slot
//! Leaves its own window open for screenshots; prints its HWND and temp directory.

#[cfg(not(windows))]
fn main() {
    eprintln!("Diagnóstico disponível somente no Windows.");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_diag::run() {
        eprintln!("ABORTADO: {error}; nenhuma tentativa automática de repetir teclas.");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows_diag {
    use std::collections::HashSet;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use vyrp_lib::core::types::{AppContext, InsertMethod, InsertRequest};
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowTextW, IsWindowVisible, SetForegroundWindow,
    };

    fn title(hwnd: isize) -> String {
        let mut buf = [0u16; 1024];
        let len = unsafe { GetWindowTextW(hwnd as HWND, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..len.max(0) as usize])
    }

    fn windows() -> Vec<isize> {
        unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> i32 {
            // SAFETY: EnumWindows calls synchronously with the live Vec pointer.
            unsafe {
                (&mut *(param as *mut Vec<isize>)).push(hwnd as isize);
            }
            1
        }
        let mut list = Vec::new();
        unsafe {
            EnumWindows(Some(visit), (&mut list as *mut Vec<isize>) as isize);
        }
        list
    }

    fn guard(hwnd: isize, marker: &str) -> Result<(), String> {
        // Recheck inside this test immediately before calling the inserter. The
        // inserter independently rechecks again immediately before each SendInput.
        if marker.is_empty()
            || unsafe { GetForegroundWindow() } as isize != hwnd
            || !title(hwnd).contains(marker)
        {
            eprintln!(
                "ABORTADO: alvo_hwnd={hwnd}, foco_hwnd={}; HWND/título não correspondem à janela criada pelo teste.",
                unsafe { GetForegroundWindow() } as isize
            );
            Err("foco não pertence ao teste".into())
        } else {
            Ok(())
        }
    }

    fn capture(hwnd: isize, marker: &str) -> Result<(AppContext, u128), String> {
        let source = vyrp_lib::context::diag_source(hwnd, marker.into());
        for _ in 0..3 {
            guard(hwnd, marker)?;
            let start = Instant::now();
            let context = source.capture();
            let elapsed = start.elapsed().as_millis();
            guard(hwnd, marker)?;
            if context.title.contains(marker) {
                return Ok((context, elapsed));
            }
            if !context.title.is_empty() {
                return Err("contexto não pertence ao teste; conteúdo descartado".into());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Ok((AppContext::default(), 150))
    }

    fn launch(app: &str, dir: &Path, marker: &str) -> Result<(), String> {
        match app {
            "notepad" => {
                let path = dir.join(format!("{marker}.txt"));
                std::fs::write(&path, "Antes do cursor. Texto de teste Vyrp.\r\n")
                    .map_err(|e| e.to_string())?;
                Command::new("notepad.exe")
                    .arg(path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            "textbox" => {
                let path = dir.join("textbox-test.ps1");
                let script = format!(
                    "Add-Type -AssemblyName System.Windows.Forms\nAdd-Type -AssemblyName System.Drawing\n$form = New-Object Windows.Forms.Form\n$form.Text = '{marker}'\n$form.Size = New-Object Drawing.Size(1000,450)\n$box = New-Object Windows.Forms.TextBox\n$box.Multiline = $true\n$box.Dock = 'Fill'\n$box.Font = New-Object Drawing.Font('Segoe UI',22)\n$box.Text = 'Antes do cursor. Texto de teste Vyrp.'\n$form.Controls.Add($box)\n$form.Add_Shown({{ $box.Focus(); $box.Select(6,9) }})\n[void]$form.ShowDialog()\n"
                );
                std::fs::write(&path, script).map_err(|e| e.to_string())?;
                Command::new("powershell.exe")
                    .args(["-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-File"])
                    .arg(path)
                    .creation_flags(0x08000000)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            "terminal" => {
                // Explicit new window and fixed tab title; no typed Enter. Even
                // accidental manual submission only feeds Read-Host, not a shell.
                let path = dir.join("terminal-test.ps1");
                std::fs::write(&path, format!("$Host.UI.RawUI.WindowTitle = '{marker}'\nWrite-Host 'Vyrp - janela exclusiva de teste'\nwhile ($true) {{ $line = Read-Host 'Texto'; Write-Host ('Recebido: ' + $line) }}\n")).map_err(|e| e.to_string())?;
                Command::new("wt.exe")
                    .args([
                        "-w",
                        "new",
                        "new-tab",
                        "--title",
                        marker,
                        "--suppressApplicationTitle",
                        "powershell.exe",
                        "-NoLogo",
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                    ])
                    .arg(path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            "chrome" | "edge" | "edge-password" => {
                let paths = if app != "chrome" {
                    [
                        "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
                        "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
                    ]
                } else {
                    [
                        "C:/Program Files/Google/Chrome/Application/chrome.exe",
                        "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
                    ]
                };
                let chrome = paths
                    .iter()
                    .find(|p| Path::new(p).exists())
                    .ok_or("navegador não encontrado")?;
                let listener =
                    std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
                let addr = listener.local_addr().map_err(|e| e.to_string())?;
                let page = if app == "edge-password" {
                    // Campo de senha em foco: o contexto tem de vir com password=true e
                    // nenhum texto; a inserção tem de recusar.
                    format!(
                        "<!doctype html><meta charset=utf-8><title>{marker}</title><h1>Vyrp: campo de senha de teste</h1><input id=pw type=password autofocus value='nao-e-segredo' style='font:24px sans-serif'><script>pw.focus()</script>"
                    )
                } else {
                    format!(
                        "<!doctype html><meta charset=utf-8><title>{marker}</title><h1>Vyrp: contexto de teste</h1><textarea id=editor autofocus style='width:90%;height:260px;font:24px sans-serif'>Antes seleção depois.</textarea><script>editor.focus();editor.setSelectionRange(6,13)</script>"
                    )
                };
                std::thread::spawn(move || {
                    for mut stream in listener.incoming().flatten() {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut request = [0u8; 4096];
                        let _ = stream.read(&mut request);
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                            page.len()
                        );
                    }
                });
                Command::new(chrome)
                    .args([
                        "--new-window",
                        "--no-first-run",
                        "--no-default-browser-check",
                        "--disable-extensions",
                        "--force-renderer-accessibility",
                    ])
                    .arg(format!(
                        "--user-data-dir={}",
                        dir.join("chrome-profile").display()
                    ))
                    .arg(format!("http://{addr}/"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            _ => {
                return Err("use notepad, textbox, chrome, edge, edge-password ou terminal".into());
            }
        }
        Ok(())
    }

    pub fn run() -> Result<(), String> {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init();
        let args: Vec<String> = std::env::args().collect();
        if args.len() != 4
            || args[3] != "--ack-test-slot"
            || !["context", "paste", "type", "guard", "copy"].contains(&args[2].as_str())
        {
            return Err("uso: diag-desktop notepad|textbox|chrome|edge|edge-password|terminal context|paste|type|guard|copy --ack-test-slot (somente após liberação do slot)".into());
        }
        let marker = format!("VYRP-TEST-{}", uuid::Uuid::new_v4());
        let dir = std::env::temp_dir().join(&marker);
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let existing: HashSet<isize> = windows().into_iter().collect();
        launch(&args[1], &dir, &marker)?;
        let start = Instant::now();
        let hwnd = loop {
            let matches: Vec<isize> = windows()
                .into_iter()
                .filter(|h| {
                    !existing.contains(h)
                        && unsafe { IsWindowVisible(*h as HWND) } != 0
                        && title(*h).contains(&marker)
                })
                .collect();
            if matches.len() == 1 {
                break matches[0];
            }
            if start.elapsed() > Duration::from_secs(12) {
                return Err(format!(
                    "não surgiu UMA janela nova com título único; nenhuma tecla enviada. Temporário: {}",
                    dir.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let target = serde_json::json!({ "hwnd": hwnd, "marker": marker, "temp_dir": dir });
        std::fs::write(dir.join("target.json"), target.to_string()).map_err(|e| e.to_string())?;
        println!("TARGET {target}");
        unsafe {
            SetForegroundWindow(hwnd as HWND);
        }
        std::thread::sleep(Duration::from_millis(200));
        guard(hwnd, &marker)?;
        let (before, elapsed_ms) = capture(hwnd, &marker)?;
        let outcome = if args[2] == "context" {
            None
        } else {
            let target_title = if args[2] == "guard" {
                format!("WRONG-{marker}")
            } else {
                marker.clone()
            };
            let inserter = vyrp_lib::insert::diag_inserter(vyrp_lib::insert::DiagTarget {
                hwnd,
                title_contains: target_title,
            });
            let req = InsertRequest {
                text: "Vyrp teste seguro — ação 🦀".into(),
                method: if args[2] == "type" {
                    InsertMethod::Type
                } else if args[1] == "textbox" {
                    InsertMethod::Paste
                } else {
                    InsertMethod::Auto
                },
                ..Default::default()
            };
            guard(hwnd, &marker)?;
            // `copy` não toca no teclado: só põe o texto no clipboard.
            let outcome = if args[2] == "copy" {
                inserter.copy_text(&req)
            } else {
                inserter.insert(&req)
            };
            if args[2] == "guard" && outcome.ok {
                return Err("FALHA DE SEGURANÇA: título incorreto aceito".into());
            }
            Some(outcome)
        };
        let (after, _) = capture(hwnd, &marker)?;
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "app": args[1], "operation": args[2], "hwnd": hwnd, "title": title(hwnd), "temp_dir": dir, "capture_ms": elapsed_ms, "before": before, "outcome": outcome, "after": after })).map_err(|e| e.to_string())?);
        Ok(())
    }
}
