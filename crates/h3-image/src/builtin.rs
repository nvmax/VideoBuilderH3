//! The presets that ship with the app. Their workflows are the pack's image templates, unchanged. The bindings use the
//! pack's setting names (`prompt`, `width`, `first_pass_width`...), so an agent that knows the pack's image settings
//! knows these.

use serde_json::{json, Value};

use crate::preset::{Binding, BindingKind, ImagePreset, Op, PresetOrigin};

const FLUX_KLEIN: &str = include_str!("../templates/fluxKleinMultiImage_API.json");
const ZIMAGE: &str = include_str!("../templates/text2image_zimage_API.json");
const ERNIE: &str = include_str!("../templates/image_ernie_image_turbo_API.json");

fn workflow(text: &str) -> Value {
    serde_json::from_str(text).expect("built-in workflow is valid JSON")
}

fn set(node: &str, input: &str, value: Value) -> Op {
    Op::Set { node: node.into(), input: input.into(), value }
}

fn link(node: &str, input: &str, from: &str, slot: u32) -> Op {
    Op::Link { node: node.into(), input: input.into(), from_node: from.into(), from_slot: slot }
}

fn seed(targets: &[(&str, &str)]) -> Binding {
    Binding::new("seed", "Seed", BindingKind::Seed, targets).default(json!(-1))
}

fn models(unet: &str, clip: &str, vae: &str) -> Vec<Binding> {
    vec![
        Binding::new("unet_name", "Diffusion model", BindingKind::Choice, &[(unet, "unet_name")]),
        Binding::new("clip_name", "Text encoder", BindingKind::Choice, &[(clip, "clip_name")]),
        Binding::new("vae_name", "VAE", BindingKind::Choice, &[(vae, "vae_name")]),
    ]
}

fn flux_klein() -> ImagePreset {
    let mut bindings = vec![Binding::new("prompt", "Prompt", BindingKind::Text, &[("1067", "text")])];
    bindings.push(Binding::new("width", "Width", BindingKind::Int, &[("1052", "width"), ("1057", "width")]).range(64.0, 4096.0).default(json!(1024)));
    bindings.push(Binding::new("height", "Height", BindingKind::Int, &[("1052", "height"), ("1057", "height")]).range(64.0, 4096.0).default(json!(576)));
    bindings.push(seed(&[("1056", "noise_seed")]));
    bindings.push(Binding::new("steps", "Steps", BindingKind::Int, &[("1052", "steps")]).range(1.0, 100.0));
    bindings.extend(models("1068", "1066", "1064"));
    let mut loras = Binding::new("loras", "LoRAs", BindingKind::Loras, &[("1075", "use_custom_loras")]);
    loras.two_pass = false;
    bindings.push(loras);
    // Without reference images the references and their size node leave the graph and the guider takes the text alone.
    let mut references = Binding::new("reference_images", "Reference images", BindingKind::Images, &[("1072", "image_paths")]);
    references.when_unset = vec![link("1053", "positive", "1067", 0), link("1053", "negative", "1058", 0), Op::Remove { node: "1072".into() }, Op::Remove { node: "1059".into() }];
    bindings.push(references);
    ImagePreset {
        id: "flux_klein".into(),
        name: "Flux Klein (reference images)".into(),
        description: "Flux 2 Klein. Optional reference images guide the result, so a character sheet can be edited into new scenes.".into(),
        origin: PresetOrigin::Builtin,
        workflow: workflow(FLUX_KLEIN),
        bindings,
        output_nodes: vec!["1048".into()],
    }
}

fn image_to_image(node: &str, switches: &[&str], start_node: &str) -> Vec<Binding> {
    let mut image = Binding::new("image_to_image", "Starting image", BindingKind::Image, &[(node, "image")]);
    image.placeholder = true;
    image.when_set = switches.iter().map(|s| set(s, "switch", json!(true))).collect();
    image.when_unset = switches.iter().map(|s| set(s, "switch", json!(false))).collect();
    vec![Binding::new("image_to_image_start_at_step", "Starting image strength step", BindingKind::Int, &[(start_node, "value")]).range(1.0, 8.0).default(json!(5)), image]
}

fn zimage() -> ImagePreset {
    let mut bindings = vec![Binding::new("prompt", "Prompt", BindingKind::Text, &[("971", "text")])];
    bindings.push(Binding::new("first_pass_width", "First pass width", BindingKind::Int, &[("965", "width")]).range(64.0, 4096.0).default(json!(1280)));
    bindings.push(Binding::new("first_pass_height", "First pass height", BindingKind::Int, &[("965", "height")]).range(64.0, 4096.0).default(json!(720)));
    bindings.push(Binding::new("second_pass_width", "Second pass width", BindingKind::Int, &[("967", "width")]).range(64.0, 4096.0).default(json!(1920)));
    bindings.push(Binding::new("second_pass_height", "Second pass height", BindingKind::Int, &[("967", "height")]).range(64.0, 4096.0).default(json!(1080)));
    bindings.push(Binding::new("batch_size", "Batch size", BindingKind::Int, &[("965", "batch_size")]).range(1.0, 16.0).default(json!(1)));
    bindings.push(seed(&[("964", "noise_seed"), ("966", "noise_seed")]));
    bindings.extend(models("972", "960", "961"));
    let mut loras = Binding::new("loras", "LoRAs", BindingKind::Loras, &[("984", "use_custom_loras")]);
    loras.two_pass = true;
    bindings.push(loras);
    bindings.extend(image_to_image("979", &["978", "981"], "983"));
    ImagePreset {
        id: "zimage".into(),
        name: "Z-Image Turbo (2 pass)".into(),
        description: "Z-Image Turbo with a second pass that upscales to the final size. Optional starting image for image to image.".into(),
        origin: PresetOrigin::Builtin,
        workflow: workflow(ZIMAGE),
        bindings,
        output_nodes: vec!["975".into()],
    }
}

fn ernie() -> ImagePreset {
    let mut bindings = vec![Binding::new("prompt", "Prompt", BindingKind::Text, &[("111", "text")])];
    bindings.push(Binding::new("width", "Width", BindingKind::Int, &[("104", "width"), ("120", "width")]).range(64.0, 4096.0).default(json!(1280)));
    bindings.push(Binding::new("height", "Height", BindingKind::Int, &[("104", "height"), ("120", "height")]).range(64.0, 4096.0).default(json!(720)));
    bindings.push(Binding::new("batch_size", "Batch size", BindingKind::Int, &[("104", "batch_size"), ("120", "batch_size")]).range(1.0, 16.0).default(json!(1)));
    bindings.push(seed(&[("121", "noise_seed")]));
    bindings.extend(models("105", "108", "109"));
    bindings.push(Binding::new("loras", "LoRAs", BindingKind::Loras, &[("113", "use_custom_loras")]));
    bindings.extend(image_to_image("118", &["114", "117"], "115"));
    ImagePreset {
        id: "ernie".into(),
        name: "Ernie Image Turbo".into(),
        description: "Ernie Image Turbo. Optional starting image for image to image.".into(),
        origin: PresetOrigin::Builtin,
        workflow: workflow(ERNIE),
        bindings,
        output_nodes: vec!["112".into()],
    }
}

/// Every built-in preset. Ids match the pack's `image_model_mode` values.
pub fn builtin_presets() -> Vec<ImagePreset> {
    vec![zimage(), flux_klein(), ernie()]
}
