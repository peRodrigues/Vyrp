//! Catálogo dos modelos whisper (ggml) oferecidos. Tamanho e SHA-256 conferidos na
//! API do Hugging Face (`ggerganov/whisper.cpp`, árvore `main`) em 2026-10-20.

use serde::Serialize;

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelSpec {
    /// Id estável usado no Settings e nos commands.
    pub id: &'static str,
    pub file: &'static str,
    pub label: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    /// Bom ponto de partida sem GPU / com GPU.
    pub recommended_cpu: bool,
    pub recommended_gpu: bool,
}

impl ModelSpec {
    pub fn url(&self) -> String {
        format!("{BASE_URL}{}", self.file)
    }
}

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: "tiny",
        file: "ggml-tiny.bin",
        label: "Tiny (78 MB): o mais rápido, qualidade baixa",
        size_bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        recommended_cpu: false,
        recommended_gpu: false,
    },
    ModelSpec {
        id: "base",
        file: "ggml-base.bin",
        label: "Base (148 MB): rápido, qualidade razoável",
        size_bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        recommended_cpu: false,
        recommended_gpu: false,
    },
    ModelSpec {
        id: "small-q5_1",
        file: "ggml-small-q5_1.bin",
        label: "Small quantizado (190 MB): bom equilíbrio sem GPU",
        size_bytes: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        recommended_cpu: true,
        recommended_gpu: false,
    },
    ModelSpec {
        id: "small",
        file: "ggml-small.bin",
        label: "Small (488 MB)",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        recommended_cpu: false,
        recommended_gpu: false,
    },
    ModelSpec {
        id: "large-v3-turbo-q5_0",
        file: "ggml-large-v3-turbo-q5_0.bin",
        label: "Large v3 Turbo quantizado (574 MB): melhor qualidade com GPU",
        size_bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        recommended_cpu: false,
        recommended_gpu: true,
    },
    ModelSpec {
        id: "large-v3-turbo",
        file: "ggml-large-v3-turbo.bin",
        label: "Large v3 Turbo (1,6 GB)",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        recommended_cpu: false,
        recommended_gpu: false,
    },
];

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let mut ids: Vec<_> = CATALOG.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len(), "ids repetidos");
        for m in CATALOG {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(m.file.starts_with("ggml-") && m.file.ends_with(".bin"));
            assert!(m.url().starts_with("https://huggingface.co/"));
        }
        assert_eq!(CATALOG.iter().filter(|m| m.recommended_cpu).count(), 1);
        assert_eq!(CATALOG.iter().filter(|m| m.recommended_gpu).count(), 1);
        assert!(find("small-q5_1").is_some() && find("nao-existe").is_none());
    }
}
