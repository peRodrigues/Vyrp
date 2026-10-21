# Vyrp

Ditado por voz para Windows: segure um atalho, fale, solte, e o texto limpo
é colado no app em foco. Tauri 2 + Rust + React/TS. A STT é local (whisper.cpp),
com nuvem opcional; a limpeza por LLM é opcional e usa a chave do próprio usuário.

Arquitetura e plano de cards: [`docs/re/wispr-flow-spec.md`](docs/re/wispr-flow-spec.md)
(§5 arquitetura, §5.2 contratos, §6 cards).

## Requisitos

- Windows 10/11 com WebView2 (já vem no Windows 11)
- [Rust](https://rustup.rs) stable (toolchain MSVC) + Visual Studio Build Tools (C++)
- Node.js 24+
- **CMake** e **libclang** (para compilar o whisper.cpp do STT local):
  - CMake: o do VS Build Tools serve
    (`...\Microsoft Visual Studio\<ver>\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`
    no `PATH`), ou `winget install Kitware.CMake`;
  - libclang: `winget install LLVM.LLVM` e `LIBCLANG_PATH=C:\Program Files\LLVM\bin`,
    ou, sem admin, `pip install --user libclang` e `LIBCLANG_PATH` apontando para
    a pasta `clang
ative` do pacote.

  O CI (windows-latest) já tem os dois. Os flags de compilação do whisper.cpp
  (otimização `/O2`, AVX2/FMA/F16C) ficam em `src-tauri/.cargo/config.toml`;
  sem eles o MSVC gera o whisper.cpp sem otimização e a transcrição fica ~10x mais lenta.

### GPU (opcional)

O build padrão é CPU. Para o whisper usar a GPU: `npm run tauri build -- --features gpu-vulkan`
(exige o [Vulkan SDK](https://vulkan.lunarg.com/) no build; roda em NVIDIA, AMD e Intel e
usa o `vulkan-1.dll` que já vem com o driver). Há também `gpu-cuda` (só NVIDIA, exige o CUDA
Toolkit e distribuir as DLLs do cuBLAS).

## Rodar

```powershell
npm install
npm run tauri dev        # app em modo dev (hot reload do front)
```

O app abre o **hub** (janela principal), o **overlay** (barra na borda
inferior) e um ícone no **tray**. Fechar o hub só esconde a janela; para
sair, use "Sair" no menu do tray. Os logs ficam em
`%LOCALAPPDATA%\app.vyrp.desktop\logs` (nível via `VYRP_LOG`, ex.: `debug`).

## Checagens

```powershell
npm run lint             # ESLint + TypeScript
npm test                 # Vitest
npm run lint:rust        # cargo fmt --check + clippy -D warnings
npm run test:rust        # cargo test
npm run check            # tudo acima
npm run tauri build -- --no-bundle   # exe release em src-tauri/target/release/vyrp.exe
npm run tauri build      # instaladores NSIS/MSI
```

O CI (`.github/workflows/ci.yml`, windows-latest) roda lint, testes e build.

## Transcrição local (whisper)

Os modelos ficam em `%APPDATA%\app.vyrp.desktop\models` e são baixados pelo app
(SHA-256 conferido). Diagnóstico e benchmark:

```powershell
src-tauri\target\release\vyrp.exe --diag-model small-q5_1           # baixa, verifica e carrega
src-tauri\target\release\vyrp.exe --diag-transcribe C:\caminho\fala.wav
src-tauri\target\release\vyrp.exe --diag-record 6                    # grava do mic e transcreve
cd src-tauri; cargo run --release --bin bench-stt -- <modelo.bin> <fala.wav> pt 3
```

## Estrutura

```
src/hub/          janela principal (React)
src/overlay/      barra de estado transparente (React)
src/shared/       tipos espelhados do Rust (eventos do ditado)
src-tauri/src/
  core/types.rs   contratos entre módulos (spec §5.2)
  hotkey/ audio/ vad/ stt/ context/ post/ llm/ insert/ storage/ settings/ metrics/
```

## Evidência de teste

O `.evidencias.json` configura a skill `test-evidence`: captura a janela
"Vyrp", com o painel em `http://localhost:8791/`. Os dados ficam em
`.evidencias/`, fora do git.
