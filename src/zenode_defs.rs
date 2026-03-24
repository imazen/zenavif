//! zennode node definitions for AVIF encoding.
//!
//! Defines [`EncodeAvif`] with RIAPI-compatible querystring keys
//! for AVIF encoding parameters.

use zennode::*;

/// AVIF encoding with quality, speed, lossless, and alpha quality options.
///
/// JSON API: `{ "quality": 80, "speed": 6, "lossless": false, "alpha_quality": 80 }`
/// RIAPI: `?avif.quality=80&avif.speed=6&avif.lossless=false&avif.alpha_quality=80`
#[derive(Node, Clone, Debug)]
#[node(id = "zenavif.encode", group = Encode, role = Encode)]
#[node(tags("codec", "avif", "lossy", "encode", "hdr"))]
pub struct EncodeAvif {
    /// Generic quality 0-100 (mapped via with_generic_quality at execution time).
    ///
    /// When set (>= 0), this value is passed through zencodec's
    /// `with_generic_quality()` which maps it to the codec's native
    /// quality scale. Use this for uniform quality across all codecs.
    #[param(range(0..=100), default = -1, step = 1)]
    #[param(unit = "", section = "Main", label = "Quality")]
    #[kv("quality")]
    pub quality: i32,

    /// Codec-specific AVIF quality override (0 = smallest, 100 = best).
    ///
    /// Controls the AV1 quantizer level. Higher values produce
    /// larger files with better visual quality.
    /// When set (>= 0), takes precedence over the generic `quality` field.
    #[param(range(0..=100), default = -1, step = 1)]
    #[param(unit = "", section = "Main", label = "AVIF Quality")]
    #[kv("avif.quality")]
    pub avif_quality: i32,

    /// Encoding speed (1 = slowest/best compression, 10 = fastest).
    ///
    /// Higher values trade compression efficiency for encoding speed.
    /// Speed 6 is a good default balancing quality and throughput.
    #[param(range(1..=10), default = 6, step = 1)]
    #[param(unit = "", section = "Main", label = "Speed")]
    #[kv("avif.speed")]
    pub speed: i32,

    /// Enable lossless encoding.
    ///
    /// When true, produces pixel-perfect output at the cost of
    /// larger file sizes. Ignores the quality setting.
    #[param(default = false)]
    #[param(section = "Main", label = "Lossless")]
    #[kv("avif.lossless")]
    pub lossless: bool,

    /// Alpha channel quality (0 = smallest, 100 = best).
    ///
    /// Controls the quality of the alpha plane, encoded as a
    /// separate AV1 frame. Only relevant for images with transparency.
    #[param(range(0..=100), default = 80, step = 1)]
    #[param(unit = "", section = "Advanced", label = "Alpha Quality")]
    #[kv("avif.alpha_quality")]
    pub alpha_quality: i32,
}

impl Default for EncodeAvif {
    fn default() -> Self {
        Self {
            quality: -1,
            avif_quality: -1,
            speed: 6,
            lossless: false,
            alpha_quality: 80,
        }
    }
}

/// Register all AVIF zennode definitions with a registry.
pub fn register(registry: &mut NodeRegistry) {
    registry.register(&ENCODE_AVIF_NODE);
}

/// All AVIF zennode definitions.
pub static ALL: &[&dyn NodeDef] = &[&ENCODE_AVIF_NODE];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_metadata() {
        let schema = ENCODE_AVIF_NODE.schema();
        assert_eq!(schema.id, "zenavif.encode");
        assert_eq!(schema.group, NodeGroup::Encode);
        assert_eq!(schema.role, NodeRole::Encode);
        assert!(schema.tags.contains(&"avif"));
        assert!(schema.tags.contains(&"codec"));
        assert!(schema.tags.contains(&"lossy"));
        assert!(schema.tags.contains(&"encode"));
        assert!(schema.tags.contains(&"hdr"));
    }

    #[test]
    fn param_names() {
        let schema = ENCODE_AVIF_NODE.schema();
        let names: Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(names.contains(&"quality"));
        assert!(names.contains(&"avif_quality"));
        assert!(names.contains(&"speed"));
        assert!(names.contains(&"lossless"));
        assert!(names.contains(&"alpha_quality"));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn defaults() {
        let node = ENCODE_AVIF_NODE.create_default().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(-1)));
        assert_eq!(node.get_param("avif_quality"), Some(ParamValue::I32(-1)));
        assert_eq!(node.get_param("speed"), Some(ParamValue::I32(6)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(false)));
        assert_eq!(node.get_param("alpha_quality"), Some(ParamValue::I32(80)));
    }

    #[test]
    fn from_kv_avif_quality() {
        let mut kv = KvPairs::from_querystring("avif.quality=90&avif.speed=3");
        let node = ENCODE_AVIF_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("avif_quality"), Some(ParamValue::I32(90)));
        assert_eq!(node.get_param("speed"), Some(ParamValue::I32(3)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn from_kv_generic_quality() {
        let mut kv = KvPairs::from_querystring("quality=80");
        let node = ENCODE_AVIF_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        // avif_quality remains unset
        assert_eq!(node.get_param("avif_quality"), Some(ParamValue::I32(-1)));
    }

    #[test]
    fn from_kv_both_qualities() {
        let mut kv = KvPairs::from_querystring("quality=80&avif.quality=90");
        let node = ENCODE_AVIF_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node.get_param("avif_quality"), Some(ParamValue::I32(90)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn from_kv_lossless() {
        let mut kv = KvPairs::from_querystring("avif.lossless=true");
        let node = ENCODE_AVIF_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(true)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn from_kv_alpha_quality() {
        let mut kv = KvPairs::from_querystring("avif.alpha_quality=50");
        let node = ENCODE_AVIF_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("alpha_quality"), Some(ParamValue::I32(50)));
    }

    #[test]
    fn from_kv_no_match() {
        let mut kv = KvPairs::from_querystring("w=800&h=600");
        let result = ENCODE_AVIF_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn json_round_trip() {
        let mut params = ParamMap::new();
        params.insert("quality".into(), ParamValue::I32(80));
        params.insert("avif_quality".into(), ParamValue::I32(92));
        params.insert("speed".into(), ParamValue::I32(4));
        params.insert("lossless".into(), ParamValue::Bool(true));
        params.insert("alpha_quality".into(), ParamValue::I32(95));

        let node = ENCODE_AVIF_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node.get_param("avif_quality"), Some(ParamValue::I32(92)));
        assert_eq!(node.get_param("speed"), Some(ParamValue::I32(4)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(true)));
        assert_eq!(node.get_param("alpha_quality"), Some(ParamValue::I32(95)));

        // Round-trip
        let exported = node.to_params();
        let node2 = ENCODE_AVIF_NODE.create(&exported).unwrap();
        assert_eq!(node2.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node2.get_param("avif_quality"), Some(ParamValue::I32(92)));
        assert_eq!(node2.get_param("speed"), Some(ParamValue::I32(4)));
        assert_eq!(node2.get_param("lossless"), Some(ParamValue::Bool(true)));
        assert_eq!(node2.get_param("alpha_quality"), Some(ParamValue::I32(95)));
    }

    #[test]
    fn downcast_to_concrete() {
        let node = ENCODE_AVIF_NODE.create_default().unwrap();
        let enc = node.as_any().downcast_ref::<EncodeAvif>().unwrap();
        assert_eq!(enc.quality, -1);
        assert_eq!(enc.avif_quality, -1);
        assert_eq!(enc.speed, 6);
        assert!(!enc.lossless);
        assert_eq!(enc.alpha_quality, 80);
    }

    #[test]
    fn registry_integration() {
        let mut registry = NodeRegistry::new();
        register(&mut registry);
        assert!(registry.get("zenavif.encode").is_some());

        // avif.quality triggers codec-specific path
        let result = registry.from_querystring("avif.quality=80&avif.speed=4");
        assert_eq!(result.instances.len(), 1);
        assert_eq!(result.instances[0].schema().id, "zenavif.encode");

        // generic quality also triggers the node
        let result2 = registry.from_querystring("quality=80");
        assert_eq!(result2.instances.len(), 1);
        assert_eq!(result2.instances[0].schema().id, "zenavif.encode");
    }
}
