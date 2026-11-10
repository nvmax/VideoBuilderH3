//! The three sites a picture can be made on. Port of `_PROVIDERS` and `_normalize_provider` of the pack's `browser/nodes.py`: the same
//! ids, addresses and debug ports, so the Image panel's settings (which name a provider by id) mean the same thing here.

use crate::error::{BrowserError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Google Flow with Nano Banana.
    Flow,
    /// ChatGPT Images (GPT Image).
    GptImage,
    /// Meta AI.
    MetaAi,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Flow, Provider::GptImage, Provider::MetaAi];

    /// A provider by the id the Image panel stores or by one of the names it is also called (`flow`, `chatgpt`, `meta`, ...).
    pub fn parse(value: &str) -> Result<Provider> {
        let key = value.trim().to_lowercase().replace(['-', ' '], "_");
        match key.as_str() {
            "flow" | "flow_browser" | "flow_nano" | "flow_nanobanana" | "flow_nano_banana" => Ok(Provider::Flow),
            "chatgpt" | "chatgpt_image" | "chatgpt_images" | "gpt" | "gpt_image" | "gpt_image_2" | "gpt_images" => Ok(Provider::GptImage),
            "meta" | "meta_ai" | "metaai" | "meta_image" | "meta_images" => Ok(Provider::MetaAi),
            _ => Err(BrowserError::page(format!("Unknown browser image provider: {}", if value.trim().is_empty() { "(empty)" } else { value.trim() }))),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Provider::Flow => "flow_nano_banana",
            Provider::GptImage => "gpt_image",
            Provider::MetaAi => "meta_ai",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Flow => "Flow Nano Banana",
            Provider::GptImage => "GPT Image",
            Provider::MetaAi => "Meta AI",
        }
    }

    /// The page the automation starts from, and the page the person signs in on.
    pub fn url(self) -> &'static str {
        match self {
            Provider::Flow => "https://labs.google/fx/tools/flow",
            Provider::GptImage => "https://chatgpt.com/images",
            Provider::MetaAi => "https://www.meta.ai/",
        }
    }

    /// The Chrome debug port, one per provider so the three can stay open side by side.
    pub fn debug_port(self) -> u16 {
        match self {
            Provider::Flow => 9222,
            Provider::GptImage => 9223,
            Provider::MetaAi => 9224,
        }
    }

    /// The folder (inside the app's data folder) that keeps the provider's Chrome profile, so the sign-in lasts.
    pub fn profile_name(self) -> &'static str {
        match self {
            Provider::Flow => "chrome-flow-profile",
            Provider::GptImage => "chrome-chatgpt-profile",
            Provider::MetaAi => "chrome-meta-profile",
        }
    }

    /// How long one picture may take when the request does not say.
    pub fn default_timeout_seconds(self) -> u64 {
        match self {
            Provider::Flow => 420,
            Provider::GptImage | Provider::MetaAi => 600,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_the_pack_accepts_means_the_same_provider() {
        for (name, provider) in [
            ("flow_nano_banana", Provider::Flow),
            ("Flow", Provider::Flow),
            ("flow-nano", Provider::Flow),
            (" flow nanobanana ", Provider::Flow),
            ("gpt_image", Provider::GptImage),
            ("ChatGPT", Provider::GptImage),
            ("gpt images", Provider::GptImage),
            ("meta_ai", Provider::MetaAi),
            ("metaai", Provider::MetaAi),
            ("Meta-Images", Provider::MetaAi),
        ] {
            assert_eq!(Provider::parse(name).unwrap(), provider, "{name}");
        }
        assert_eq!(Provider::parse("midjourney").unwrap_err().to_string(), "Unknown browser image provider: midjourney");
        assert_eq!(Provider::parse("  ").unwrap_err().to_string(), "Unknown browser image provider: (empty)");
    }

    #[test]
    fn the_ports_and_profiles_are_the_packs_so_the_three_sites_stay_open_side_by_side() {
        let table: Vec<_> = Provider::ALL.iter().map(|p| (p.id(), p.debug_port(), p.profile_name(), p.default_timeout_seconds())).collect();
        assert_eq!(table, [("flow_nano_banana", 9222, "chrome-flow-profile", 420), ("gpt_image", 9223, "chrome-chatgpt-profile", 600), ("meta_ai", 9224, "chrome-meta-profile", 600)]);
        for provider in Provider::ALL {
            assert_eq!(Provider::parse(provider.id()).unwrap(), provider);
            assert!(provider.url().starts_with("https://"));
        }
    }
}
