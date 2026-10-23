//! Model file lists, read from `/object_info`. ComfyUI lists the files of each model folder as the options of the loader
//! node that reads them, which is the same list the pack gets from `folder_paths.get_filename_list`.

use serde_json::Value;

/// The loader inputs that list a model folder.
fn loaders(category: &str) -> &'static [(&'static str, &'static str)] {
    match category {
        "loras" => &[("LoraLoaderModelOnly", "lora_name")],
        "vae" => &[("VAELoader", "vae_name")],
        "diffusion_models" | "unet" => &[("UNETLoader", "unet_name"), ("DiffusionModelLoaderKJ", "model_name")],
        "text_encoders" | "clip" => &[("CLIPLoader", "clip_name")],
        "latent_upscale_models" => &[("LatentUpscaleModelLoader", "model_name")],
        "upscale_models" => &[("UpscaleModelLoader", "model_name")],
        _ => &[],
    }
}

/// The options of one combo input, in either spelling ComfyUI uses (`[[a, b]]` or `["COMBO", {"options": [a, b]}]`).
fn combo_options(spec: &Value) -> Vec<String> {
    let list = spec.get(0).and_then(Value::as_array).or_else(|| spec.get(1).and_then(|o| o.get("options")).and_then(Value::as_array));
    list.map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// File names ComfyUI lists for a model folder category. Empty when ComfyUI has no loader for it.
pub fn model_choices(object_info: &Value, category: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for (class, input) in loaders(category) {
        let info = object_info.get(class).and_then(|n| n.get("input"));
        let spec = ["required", "optional"].iter().filter_map(|s| info.and_then(|i| i.get(*s)).and_then(|m| m.get(*input))).next();
        for option in spec.map(combo_options).unwrap_or_default() {
            if !found.contains(&option) {
                found.push(option);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_both_combo_spellings() {
        let info = json!({
            "VAELoader": {"input": {"required": {"vae_name": [["a.safetensors", "b.safetensors"]]}}},
            "LoraLoaderModelOnly": {"input": {"required": {"lora_name": ["COMBO", {"options": ["x.safetensors"]}]}}},
        });
        assert_eq!(model_choices(&info, "vae"), ["a.safetensors", "b.safetensors"]);
        assert_eq!(model_choices(&info, "loras"), ["x.safetensors"]);
        assert!(model_choices(&info, "unet").is_empty());
    }
}
