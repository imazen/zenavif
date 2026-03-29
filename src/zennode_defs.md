//! Zennode node definitions for zenavif encode and decode.
//!
//! Provides [`AvifEncode`] and [`AvifDecode`], self-documenting pipeline nodes
//! that bridge zennode's parameter system with zenavif's encoder/decoder configs.
//!
//! Feature-gated behind `feature = "zennode"`.

extern crate alloc;
use alloc::string::String;

use zennode::*;

/// AVIF encoding node for zennode pipelines.
///
/// Exposes quality, speed, alpha quality, bit depth, chroma subsampling,
/// color model, alpha mode, and lossless controls as pipeline parameters.
///
/// All fields are `Option<T>` so the node acts as an overlay: only explicitly-set
/// parameters are applied. `None` means "inherit from the base config."
///
/// Convert to [`crate::AvifEncoderConfig`] via
/// [`to_encoder_config()`](AvifEncode::to_encoder_config) (requires `zencodec` feature).
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenavif.encode", group = Encode, role = Encode)]
#[node(tags("avif", "encode", "av1"))]
pub struct AvifEncode {
    /// Encode quality (1.0 = worst, 100.0 = best).
    #[param(range(1.0..=100.0), default = 75.0, step = 1.0)]
    #[param(section = "Main", label = "Quality")]
    #[kv("avif.q", "avif.quality")]
    pub quality: Option<f32>,

    /// Encode speed (1 = slowest/best, 10 = fastest/worst).
    #[param(range(1..=10), default = 4)]
    #[param(section = "Main", label = "Speed")]
    #[kv("avif.speed")]
    pub speed: Option<u32>,

    /// Separate alpha channel quality (0.0 = use color quality).
    ///
    /// When None (default), the color quality is used for alpha too.
    /// Set to a specific value (1.0-100.0) for independent alpha quality.
    /// Set to 0.0 to explicitly use color quality.
    #[param(range(0.0..=100.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(section = "Main", label = "Alpha Quality")]
    #[kv("avif.alpha_quality", "avif.aq")]
    pub alpha_quality: Option<f32>,

    /// Output bit depth: "auto", "8", or "10".
    #[param(default = "auto")]
    #[param(section = "Main", label = "Bit Depth")]
    #[kv("avif.depth")]
    pub bit_depth: Option<String>,

    /// Chroma subsampling: "420", "422", or "444".
    ///
    /// Note: chroma subsampling is not yet configurable in the encoder;
    /// this field is reserved for future use.
    #[param(default = "444")]
    #[param(section = "Advanced", label = "Chroma Subsampling")]
    #[kv("avif.chroma")]
    pub chroma_subsampling: Option<String>,

    /// Internal color model: "ycbcr" or "rgb".
    ///
    /// YCbCr produces smaller files. RGB may be better for lossless.
    #[param(default = "ycbcr")]
    #[param(section = "Advanced", label = "Color Model")]
    #[kv("avif.color_model")]
    pub color_model: Option<String>,

    /// Alpha handling mode: "clean", "dirty", or "premultiplied".
    ///
    /// - "clean" = unassociated alpha, clean color under transparent pixels
    /// - "dirty" = unassociated alpha, preserve original color values
    /// - "premultiplied" = premultiplied alpha
    #[param(default = "clean")]
    #[param(section = "Advanced", label = "Alpha Mode")]
    #[kv("avif.alpha_mode")]
    pub alpha_mode: Option<String>,

    /// Enable mathematically lossless encoding.
    ///
    /// When enabled, sets quality to 100 and quantizer to 0.
    /// Requires the `encode-imazen` feature for full lossless support.
    #[param(default = false)]
    #[param(section = "Advanced")]
    #[kv("avif.lossless")]
    pub lossless: Option<bool>,
}

#[cfg(all(feature = "zencodec", feature = "encode"))]
impl AvifEncode {
    /// Convert this node into an [`crate::AvifEncoderConfig`].
    ///
    /// Maps zennode parameters to the zencodec-based encoder configuration.
    /// Fields that are `None` use sensible defaults (quality=75, speed=4, etc.).
    ///
    /// - `quality` -> [`AvifEncoderConfig::with_quality`] (default 75.0)
    /// - `speed` -> [`AvifEncoderConfig::with_effort_u32`] (default 4)
    /// - `alpha_quality` (if Some and > 0) -> [`AvifEncoderConfig::with_alpha_quality_value`]
    /// - `lossless` -> [`AvifEncoderConfig::with_lossless_mode`]
    /// - `bit_depth` -> [`crate::EncodeBitDepth`] on the inner config
    /// - `color_model` -> [`crate::EncodeColorModel`] on the inner config
    /// - `alpha_mode` -> [`crate::EncodeAlphaMode`] on the inner config
    pub fn to_encoder_config(&self) -> crate::AvifEncoderConfig {
        let quality = self.quality.unwrap_or(75.0);
        let speed = self.speed.unwrap_or(4);

        let mut cfg = crate::AvifEncoderConfig::new()
            .with_quality(quality)
            .with_effort_u32(speed);

        if let Some(aq) = self.alpha_quality {
            if aq > 0.0 {
                cfg = cfg.with_alpha_quality_value(aq);
            }
        }

        if let Some(true) = self.lossless {
            cfg = cfg.with_lossless_mode(true);
        }

        // Apply bit depth to the inner config
        if let Some(ref depth_str) = self.bit_depth {
            let depth = match depth_str.as_str() {
                "8" => crate::EncodeBitDepth::Eight,
                "10" => crate::EncodeBitDepth::Ten,
                _ => crate::EncodeBitDepth::Auto,
            };
            cfg.inner_mut().bit_depth = depth;
        }

        // Apply color model to the inner config
        if let Some(ref model_str) = self.color_model {
            let model = match model_str.to_ascii_lowercase().as_str() {
                "rgb" => crate::EncodeColorModel::Rgb,
                _ => crate::EncodeColorModel::YCbCr,
            };
            cfg.inner_mut().color_model = model;
        }

        // Apply alpha mode to the inner config
        if let Some(ref alpha_str) = self.alpha_mode {
            let alpha = match alpha_str.to_ascii_lowercase().as_str() {
                "dirty" => crate::EncodeAlphaMode::UnassociatedDirty,
                "premultiplied" => crate::EncodeAlphaMode::Premultiplied,
                _ => crate::EncodeAlphaMode::UnassociatedClean,
            };
            cfg.inner_mut().alpha_color_mode = alpha;
        }

        cfg
    }

    /// Apply this node's set fields onto an existing [`crate::AvifEncoderConfig`].
    ///
    /// Fields that are `None` are skipped, preserving whatever the incoming config
    /// already has. This lets pipeline nodes act as overlays: only explicitly-set
    /// parameters are written.
    pub fn apply(&self, mut config: crate::AvifEncoderConfig) -> crate::AvifEncoderConfig {
        // Quality
        if let Some(quality) = self.quality {
            config = config.with_quality(quality);
        }

        // Speed
        if let Some(speed) = self.speed {
            config = config.with_effort_u32(speed);
        }

        // Alpha quality (0.0 means "use color quality", but we still apply it
        // since the user explicitly set the field)
        if let Some(aq) = self.alpha_quality {
            if aq > 0.0 {
                config = config.with_alpha_quality_value(aq);
            }
        }

        // Lossless
        if let Some(lossless) = self.lossless {
            config = config.with_lossless_mode(lossless);
        }

        // Bit depth
        if let Some(ref depth_str) = self.bit_depth {
            let depth = match depth_str.as_str() {
                "8" => crate::EncodeBitDepth::Eight,
                "10" => crate::EncodeBitDepth::Ten,
                _ => crate::EncodeBitDepth::Auto,
            };
            config.inner_mut().bit_depth = depth;
        }

        // Color model
        if let Some(ref model_str) = self.color_model {
            let model = match model_str.to_ascii_lowercase().as_str() {
                "rgb" => crate::EncodeColorModel::Rgb,
                _ => crate::EncodeColorModel::YCbCr,
            };
            config.inner_mut().color_model = model;
        }

        // Alpha mode
        if let Some(ref alpha_str) = self.alpha_mode {
            let alpha = match alpha_str.to_ascii_lowercase().as_str() {
                "dirty" => crate::EncodeAlphaMode::UnassociatedDirty,
                "premultiplied" => crate::EncodeAlphaMode::Premultiplied,
                _ => crate::EncodeAlphaMode::UnassociatedClean,
            };
            config.inner_mut().alpha_color_mode = alpha;
        }

        config
    }
}

/// AVIF decoding node for zennode pipelines.
///
/// Exposes film grain synthesis and gain map extraction controls as
/// pipeline parameters.
///
/// Convert to [`crate::AvifDecoderConfig`] via
/// [`to_decoder_config()`](AvifDecode::to_decoder_config) (requires `zencodec` feature).
#[derive(Node, Clone, Debug)]
#[node(id = "zenavif.decode", group = Decode, role = Decode)]
#[node(tags("avif", "decode", "av1"))]
pub struct AvifDecode {
    /// Enable film grain synthesis during decode.
    ///
    /// When enabled (default), AV1 film grain specified in the stream
    /// is synthesized and applied to the decoded image.
    #[param(default = true)]
    #[param(section = "Main", label = "Film Grain")]
    #[kv("avif.grain", "avif.film_grain")]
    pub film_grain: bool,

    /// Extract the gain map (UltraHDR / ISO 21496-1) if present.
    ///
    /// When enabled, the decoder will attempt to extract the gain map
    /// from the AVIF container. This is a forward-looking option;
    /// extraction support depends on the decoder pipeline.
    #[param(default = false)]
    #[param(section = "Advanced", label = "Extract Gain Map")]
    #[kv("avif.gain_map")]
    pub extract_gain_map: bool,
}

impl Default for AvifDecode {
    fn default() -> Self {
        Self {
            film_grain: true,
            extract_gain_map: false,
        }
    }
}

#[cfg(feature = "zencodec")]
impl AvifDecode {
    /// Convert this node into an [`crate::AvifDecoderConfig`].
    ///
    /// Maps zennode parameters to the zencodec-based decoder configuration:
    /// - `film_grain` -> [`crate::AvifDecoderConfig::with_film_grain`]
    /// - `extract_gain_map` -> [`crate::AvifDecoderConfig::with_extract_gain_map`]
    pub fn to_decoder_config(&self) -> crate::AvifDecoderConfig {
        crate::AvifDecoderConfig::new()
            .with_film_grain(self.film_grain)
            .with_extract_gain_map(self.extract_gain_map)
    }
}

/// Register all AVIF zennode definitions with a registry.
pub fn register(registry: &mut NodeRegistry) {
    registry.register(&AVIF_ENCODE_NODE);
    registry.register(&AVIF_DECODE_NODE);
}

/// All AVIF zennode definitions.
pub static ALL: &[&dyn NodeDef] = &[&AVIF_ENCODE_NODE, &AVIF_DECODE_NODE];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_schema_basics() {
        let schema = AVIF_ENCODE_NODE.schema();
        assert_eq!(schema.id, "zenavif.encode");
        assert_eq!(schema.group, NodeGroup::Encode);
        assert_eq!(schema.role, NodeRole::Encode);
        assert!(schema.tags.contains(&"avif"));
        assert!(schema.tags.contains(&"encode"));
        assert!(schema.tags.contains(&"av1"));

        let param_names: alloc::vec::Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(param_names.contains(&"quality"));
        assert!(param_names.contains(&"speed"));
        assert!(param_names.contains(&"alpha_quality"));
        assert!(param_names.contains(&"bit_depth"));
        assert!(param_names.contains(&"chroma_subsampling"));
        assert!(param_names.contains(&"color_model"));
        assert!(param_names.contains(&"alpha_mode"));
        assert!(param_names.contains(&"lossless"));

        // All encode params should be marked optional
        for p in schema.params {
            assert!(p.optional, "param {} should be optional", p.name);
        }
    }

    #[test]
    fn encode_default_values() {
        let node = AVIF_ENCODE_NODE.create_default().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::None));
        assert_eq!(node.get_param("speed"), Some(ParamValue::None));
        assert_eq!(node.get_param("alpha_quality"), Some(ParamValue::None));
        assert_eq!(node.get_param("bit_depth"), Some(ParamValue::None));
        assert_eq!(node.get_param("chroma_subsampling"), Some(ParamValue::None));
        assert_eq!(node.get_param("color_model"), Some(ParamValue::None));
        assert_eq!(node.get_param("alpha_mode"), Some(ParamValue::None));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::None));
    }

    #[test]
    fn encode_kv_keys() {
        let schema = AVIF_ENCODE_NODE.schema();

        let quality_param = schema.params.iter().find(|p| p.name == "quality").unwrap();
        assert!(quality_param.kv_keys.contains(&"avif.q"));
        assert!(quality_param.kv_keys.contains(&"avif.quality"));

        let speed_param = schema.params.iter().find(|p| p.name == "speed").unwrap();
        assert!(speed_param.kv_keys.contains(&"avif.speed"));

        let aq_param = schema
            .params
            .iter()
            .find(|p| p.name == "alpha_quality")
            .unwrap();
        assert!(aq_param.kv_keys.contains(&"avif.alpha_quality"));
        assert!(aq_param.kv_keys.contains(&"avif.aq"));
    }

    #[test]
    fn encode_kv_parsing() {
        let mut kv = KvPairs::from_querystring("avif.q=85&avif.speed=6&avif.lossless=false");
        let node = AVIF_ENCODE_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::F32(85.0)));
        assert_eq!(node.get_param("speed"), Some(ParamValue::U32(6)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(false)));
        // Unset fields should be None
        assert_eq!(node.get_param("alpha_quality"), Some(ParamValue::None));
        assert_eq!(node.get_param("bit_depth"), Some(ParamValue::None));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn encode_kv_no_match() {
        let mut kv = KvPairs::from_querystring("jpeg.q=80");
        let result = AVIF_ENCODE_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn encode_downcast() {
        let node = AVIF_ENCODE_NODE.create_default().unwrap();
        let enc = node.as_any().downcast_ref::<AvifEncode>().unwrap();
        assert_eq!(enc.quality, None);
        assert_eq!(enc.speed, None);
        assert_eq!(enc.lossless, None);
    }

    #[test]
    fn encode_downcast_with_values() {
        let mut kv = KvPairs::from_querystring("avif.q=85&avif.speed=6");
        let node = AVIF_ENCODE_NODE.from_kv(&mut kv).unwrap().unwrap();
        let enc = node.as_any().downcast_ref::<AvifEncode>().unwrap();
        assert_eq!(enc.quality, Some(85.0));
        assert_eq!(enc.speed, Some(6));
        assert_eq!(enc.alpha_quality, None);
        assert_eq!(enc.lossless, None);
    }

    #[test]
    fn encode_set_then_clear() {
        let mut enc = AvifEncode {
            quality: Some(85.0),
            speed: Some(6),
            ..Default::default()
        };

        // Clear quality with ParamValue::None
        assert!(enc.set_param("quality", ParamValue::None));
        assert_eq!(enc.quality, None);
        assert_eq!(enc.get_param("quality"), Some(ParamValue::None));

        // Speed should still be set
        assert_eq!(enc.speed, Some(6));
    }

    #[test]
    fn decode_schema_basics() {
        let schema = AVIF_DECODE_NODE.schema();
        assert_eq!(schema.id, "zenavif.decode");
        assert_eq!(schema.group, NodeGroup::Decode);
        assert_eq!(schema.role, NodeRole::Decode);
        assert!(schema.tags.contains(&"avif"));
        assert!(schema.tags.contains(&"decode"));
        assert!(schema.tags.contains(&"av1"));

        let param_names: alloc::vec::Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(param_names.contains(&"film_grain"));
        assert!(param_names.contains(&"extract_gain_map"));

        // Decode params are not optional (they are feature flags)
        for p in schema.params {
            assert!(!p.optional, "param {} should not be optional", p.name);
        }
    }

    #[test]
    fn decode_default_values() {
        let node = AVIF_DECODE_NODE.create_default().unwrap();
        assert_eq!(node.get_param("film_grain"), Some(ParamValue::Bool(true)));
        assert_eq!(
            node.get_param("extract_gain_map"),
            Some(ParamValue::Bool(false))
        );
    }

    #[test]
    fn decode_kv_keys() {
        let schema = AVIF_DECODE_NODE.schema();

        let grain_param = schema
            .params
            .iter()
            .find(|p| p.name == "film_grain")
            .unwrap();
        assert!(grain_param.kv_keys.contains(&"avif.grain"));
        assert!(grain_param.kv_keys.contains(&"avif.film_grain"));

        let gain_param = schema
            .params
            .iter()
            .find(|p| p.name == "extract_gain_map")
            .unwrap();
        assert!(gain_param.kv_keys.contains(&"avif.gain_map"));
    }

    #[test]
    fn decode_kv_parsing() {
        let mut kv = KvPairs::from_querystring("avif.grain=false&avif.gain_map=true");
        let node = AVIF_DECODE_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("film_grain"), Some(ParamValue::Bool(false)));
        assert_eq!(
            node.get_param("extract_gain_map"),
            Some(ParamValue::Bool(true))
        );
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn decode_kv_no_match() {
        let mut kv = KvPairs::from_querystring("jpeg.q=80");
        let result = AVIF_DECODE_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn decode_downcast() {
        let node = AVIF_DECODE_NODE.create_default().unwrap();
        let dec = node.as_any().downcast_ref::<AvifDecode>().unwrap();
        assert!(dec.film_grain);
        assert!(!dec.extract_gain_map);
    }

    #[cfg(all(feature = "zencodec", feature = "encode"))]
    mod encode_config_integration {
        use super::*;

        #[test]
        fn encode_to_config_defaults() {
            let node = AvifEncode::default();
            let cfg = node.to_encoder_config();
            let inner = cfg.inner();
            // Default node (all None) uses fallback quality=75, speed=4
            assert!((inner.quality - 75.0).abs() < f32::EPSILON);
            assert_eq!(inner.speed, 4);
        }

        #[test]
        fn encode_to_config_lossless() {
            let node = AvifEncode {
                lossless: Some(true),
                ..Default::default()
            };
            let cfg = node.to_encoder_config();
            // Lossless sets quality to 100
            assert!((cfg.inner().quality - 100.0).abs() < f32::EPSILON);
        }

        #[test]
        fn encode_to_config_alpha_quality() {
            let node = AvifEncode {
                alpha_quality: Some(50.0),
                ..Default::default()
            };
            let cfg = node.to_encoder_config();
            assert_eq!(cfg.inner().alpha_quality, Some(50.0));
        }

        #[test]
        fn encode_to_config_bit_depth() {
            let node = AvifEncode {
                bit_depth: Some(String::from("10")),
                ..Default::default()
            };
            let cfg = node.to_encoder_config();
            assert_eq!(cfg.inner().bit_depth, crate::EncodeBitDepth::Ten);
        }

        #[test]
        fn encode_to_config_color_model_rgb() {
            let node = AvifEncode {
                color_model: Some(String::from("rgb")),
                ..Default::default()
            };
            let cfg = node.to_encoder_config();
            assert_eq!(cfg.inner().color_model, crate::EncodeColorModel::Rgb);
        }

        #[test]
        fn encode_to_config_alpha_mode_premultiplied() {
            let node = AvifEncode {
                alpha_mode: Some(String::from("premultiplied")),
                ..Default::default()
            };
            let cfg = node.to_encoder_config();
            assert_eq!(
                cfg.inner().alpha_color_mode,
                crate::EncodeAlphaMode::Premultiplied
            );
        }

        #[test]
        fn apply_defaults_preserves_config() {
            // A default AvifEncode (all None) applied to an existing config should not change it.
            let base = crate::AvifEncoderConfig::new()
                .with_quality(90.0)
                .with_effort_u32(2);
            let quality_before = base.inner().quality;
            let speed_before = base.inner().speed;
            let depth_before = base.inner().bit_depth;
            let model_before = base.inner().color_model;
            let alpha_before = base.inner().alpha_color_mode;

            let node = AvifEncode::default();
            let result = node.apply(base);

            assert!(
                (result.inner().quality - quality_before).abs() < f32::EPSILON,
                "quality changed: {} -> {}",
                quality_before,
                result.inner().quality,
            );
            assert_eq!(result.inner().speed, speed_before);
            assert_eq!(result.inner().bit_depth, depth_before);
            assert_eq!(result.inner().color_model, model_before);
            assert_eq!(result.inner().alpha_color_mode, alpha_before);
        }

        #[test]
        fn apply_quality_override() {
            // Only quality differs from default; other fields should be untouched.
            let base = crate::AvifEncoderConfig::new().with_effort_u32(2);
            let speed_before = base.inner().speed;

            let node = AvifEncode {
                quality: Some(50.0),
                ..Default::default()
            };
            let result = node.apply(base);

            // Quality should be overwritten (50.0 maps through with_quality)
            assert!(
                (result.inner().quality - 50.0).abs() < f32::EPSILON,
                "expected quality 50.0, got {}",
                result.inner().quality,
            );
            // Speed should be preserved from the base config
            assert_eq!(result.inner().speed, speed_before);
        }

        #[test]
        fn apply_lossless_override() {
            // Lossless=true should set quality to 100 on the config.
            let base = crate::AvifEncoderConfig::new()
                .with_quality(50.0)
                .with_effort_u32(3);
            let speed_before = base.inner().speed;

            let node = AvifEncode {
                lossless: Some(true),
                ..Default::default()
            };
            let result = node.apply(base);

            // Lossless sets quality to 100
            assert!(
                (result.inner().quality - 100.0).abs() < f32::EPSILON,
                "expected quality 100.0 for lossless, got {}",
                result.inner().quality,
            );
            // Speed should remain from base (lossless doesn't change speed)
            assert_eq!(result.inner().speed, speed_before);
        }
    }

    #[cfg(feature = "zencodec")]
    mod decode_config_integration {
        use super::*;

        #[test]
        fn decode_to_config_defaults() {
            let node = AvifDecode::default();
            let cfg = node.to_decoder_config();
            assert!(cfg.inner().apply_grain);
        }

        #[test]
        fn decode_to_config_no_grain() {
            let node = AvifDecode {
                film_grain: false,
                ..Default::default()
            };
            let cfg = node.to_decoder_config();
            assert!(!cfg.inner().apply_grain);
        }
    }

    #[test]
    fn registry_integration() {
        let mut registry = NodeRegistry::new();
        register(&mut registry);
        assert!(registry.get("zenavif.encode").is_some());
        assert!(registry.get("zenavif.decode").is_some());

        // avif.q triggers the encode node
        let result = registry.from_querystring("avif.q=80&avif.speed=4");
        assert_eq!(result.instances.len(), 1);
        assert_eq!(result.instances[0].schema().id, "zenavif.encode");

        // avif.grain triggers the decode node
        let result2 = registry.from_querystring("avif.grain=false");
        assert_eq!(result2.instances.len(), 1);
        assert_eq!(result2.instances[0].schema().id, "zenavif.decode");
    }
}
