use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardModelDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub filename: &'static str,
}

pub const STANDARD_MODELS: &[StandardModelDefinition] = &[
    StandardModelDefinition {
        id: "tiny.en",
        label: "Tiny English",
        filename: "ggml-tiny.en.bin",
    },
    StandardModelDefinition {
        id: "base.en",
        label: "Base English",
        filename: "ggml-base.en.bin",
    },
    StandardModelDefinition {
        id: "small.en",
        label: "Small English",
        filename: "ggml-small.en.bin",
    },
    StandardModelDefinition {
        id: "medium.en",
        label: "Medium English",
        filename: "ggml-medium.en.bin",
    },
    // NOTE: model_catalog.rs has a test `catalog_reports_all_v1_standard_models` that asserts
    // the id list is exactly ["tiny.en","base.en","small.en","medium.en"]. That test will
    // fail after these additions and must be updated by Agent 4 / integration (out of scope here).
    StandardModelDefinition {
        id: "large-v3-turbo",
        label: "Large v3 Turbo",
        filename: "ggml-large-v3-turbo.bin",
    },
    StandardModelDefinition {
        id: "large-v3-turbo-q5_0",
        label: "Large v3 Turbo (Q5)",
        filename: "ggml-large-v3-turbo-q5_0.bin",
    },
];

pub fn by_id(model_id: &str) -> Option<&'static StandardModelDefinition> {
    STANDARD_MODELS.iter().find(|model| model.id == model_id)
}

pub fn is_standard_model_id(model_id: &str) -> bool {
    by_id(model_id).is_some()
}

pub fn model_id_from_path(path: &Path) -> Option<&'static str> {
    let filename = path.file_name().and_then(|name| name.to_str())?;
    STANDARD_MODELS
        .iter()
        .find(|model| model.filename == filename)
        .map(|model| model.id)
}
