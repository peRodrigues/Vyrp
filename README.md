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
cd src-tauri; cargo run --release --features dev-tools --bin bench-stt -- <modelo.bin> <fala.wav> pt 3
```

As ferramentas de desenvolvimento (`bench-stt`, `diag-desktop`) só compilam com a feature
`dev-tools` e não entram no instalador.

## Instalador, início com o Windows e atualizações

```powershell
npm run tauri build      # src-tauri\target\release\bundle\nsis\Vyrp_<versão>_x64-setup.exe (+ .msi)
```

- O instalador NSIS é por usuário (sem admin). Instalação silenciosa:
  `Vyrp_<versão>_x64-setup.exe /S /D=C:\caminho` (desinstalar: `uninstall.exe /S`).
- **Abrir ao iniciar o Windows**: Settings `autostart` ou o item do tray. O login abre o
  Vyrp com `--autostart`: só o ícone no tray, sem abrir o hub e sem pegar o foco.
- **Atualizações**: o app procura o `latest.json` do último Release de
  `peRodrigues/Vyrp` 1 min depois de abrir e a cada 6 h. Instalar é sempre um clique no
  tray ("Instalar atualização x.y.z"). O pacote é verificado pela chave pública do
  `tauri.conf.json` (`plugins.updater.pubkey`).

### Publicar uma versão

1. Suba a versão em `src-tauri/tauri.conf.json` (e `package.json`/`Cargo.toml`).
2. `git tag v0.2.0 && git push origin v0.2.0`: o workflow `Release` builda o instalador
   (CPU), assina os artefatos do updater e publica o Release com o `latest.json`.
   A variante com GPU (Vulkan) roda só pelo "Run workflow" manual, marcando `vulkan`.

**Chave do updater (uma vez):** o par foi gerado com `npx tauri signer generate`. A
chave PRIVADA nunca entra no repositório. Cadastre em *Settings → Secrets and variables →
Actions* do GitHub:

- `TAURI_SIGNING_PRIVATE_KEY`: o conteúdo do arquivo da chave privada;
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: vazio (a chave foi gerada sem senha).

Perder a chave privada significa que as instalações existentes não aceitam mais
atualizações: guarde uma cópia num cofre de senhas.

### Assinatura de código e SmartScreen

O instalador NÃO é assinado com certificado de código. No primeiro download o Windows
mostra "O Windows protegeu o computador" (SmartScreen): *Mais informações → Executar
assim mesmo*. Para tirar o aviso é preciso assinar o `.exe`/`.msi` com um certificado de
assinatura de código (OV/EV, ou o Azure Trusted Signing) configurado em
`bundle.windows` (`certificateThumbprint` ou `signCommand`). A reputação do SmartScreen
vem com o tempo e os downloads; com certificado EV ela é imediata. A assinatura do
updater (minisign) é outra coisa: garante que a atualização veio de nós, mas não
silencia o SmartScreen.

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
