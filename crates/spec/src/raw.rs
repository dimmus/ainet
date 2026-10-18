//! The YAML shapes of a RigSpec and an AgentSpec, as written.
//!
//! Every field is optional here so that validation can report all problems
//! at once instead of stopping at the first missing key. Scalars that people
//! write unquoted (`version: 0.2`, `id: 1`) are read as text. The rig, pod,
//! member and edge levels keep any keys they don't know in `extra`, so they
//! can be reported; everything below them ignores unknown keys, as OpenRig
//! does.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{self, IgnoredAny, Visitor};

/// A scalar read as text: strings, numbers and booleans all become strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text(pub String);

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Text, E> {
                Ok(Text(v.to_string()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Text, E> {
                Ok(Text(v))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Text, E> {
                Ok(Text(v.to_string()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Text, E> {
                Ok(Text(v.to_string()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Text, E> {
                Ok(Text(v.to_string()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Text, E> {
                Ok(Text(v.to_string()))
            }
        }
        d.deserialize_any(V)
    }
}

/// The trimmed text of an optional scalar, `None` when absent or blank.
pub fn text(v: &Option<Text>) -> Option<&str> {
    v.as_ref().map(|t| t.0.trim()).filter(|s| !s.is_empty())
}

/// Keys a level didn't recognise, or knows but does not model: a scalar's
/// text is kept (a `permission_policy`), anything else only its key.
pub type Extra = BTreeMap<String, Loose>;

/// A value of a key ainet does not model: a scalar as text, else nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loose(pub Option<String>);

impl<'de> Deserialize<'de> for Loose {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Loose;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Loose, E> {
                Ok(Loose(Some(v.to_string())))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Loose, E> {
                Ok(Loose(Some(v.to_string())))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Loose, E> {
                Ok(Loose(Some(v.to_string())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Loose, E> {
                Ok(Loose(Some(v.to_string())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Loose, E> {
                Ok(Loose(Some(v.to_string())))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Loose, E> {
                Ok(Loose(None))
            }
            fn visit_none<E: de::Error>(self) -> Result<Loose, E> {
                Ok(Loose(None))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Loose, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Loose(None))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Loose, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Loose(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// The trimmed text of a scalar kept in `extra`, `None` when absent,
/// blank or not a scalar.
pub fn loose<'a>(extra: &'a Extra, key: &str) -> Option<&'a str> {
    extra
        .get(key)
        .and_then(|v| v.0.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[derive(Debug, Default, Deserialize)]
pub struct RawRig {
    pub version: Option<Text>,
    pub name: Option<Text>,
    pub summary: Option<Text>,
    pub culture_file: Option<Text>,
    pub startup: Option<RawStartup>,
    pub pods: Option<Vec<RawPod>>,
    pub edges: Option<Vec<RawEdge>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawPod {
    pub id: Option<Text>,
    // Known, so not reported as unknown keys, but nothing reads them.
    #[serde(rename = "label")]
    pub _label: Option<IgnoredAny>,
    #[serde(rename = "summary")]
    pub _summary: Option<IgnoredAny>,
    pub startup: Option<RawStartup>,
    pub members: Option<Vec<RawMember>>,
    pub edges: Option<Vec<RawEdge>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawMember {
    pub id: Option<Text>,
    pub agent_ref: Option<Text>,
    pub profile: Option<Text>,
    pub runtime: Option<Text>,
    pub cwd: Option<Text>,
    pub label: Option<Text>,
    pub model: Option<Text>,
    pub effort: Option<Text>,
    pub role: Option<Text>,
    pub restore_policy: Option<Text>,
    pub startup: Option<RawStartup>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawEdge {
    pub kind: Option<Text>,
    pub from: Option<Text>,
    pub to: Option<Text>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawStartup {
    #[serde(default)]
    pub files: Option<Vec<RawFile>>,
    #[serde(default)]
    pub actions: Option<Vec<RawAction>>,
}

impl RawStartup {
    pub fn files(&self) -> &[RawFile] {
        self.files.as_deref().unwrap_or_default()
    }

    pub fn actions(&self) -> &[RawAction] {
        self.actions.as_deref().unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawFile {
    pub path: Option<Text>,
    pub delivery_hint: Option<Text>,
    pub required: Option<bool>,
    pub applies_on: Option<Vec<Text>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawAction {
    #[serde(rename = "type")]
    pub kind: Option<Text>,
    pub value: Option<Text>,
    pub phase: Option<Text>,
    pub idempotent: Option<bool>,
    pub applies_on: Option<Vec<Text>>,
}

/// `agent.yaml`.
#[derive(Debug, Default, Deserialize)]
pub struct RawAgent {
    pub name: Option<Text>,
    pub defaults: Option<RawDefaults>,
    pub imports: Option<Vec<RawImport>>,
    pub profiles: Option<BTreeMap<String, RawProfile>>,
    pub resources: Option<RawResources>,
    pub startup: Option<RawStartup>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawDefaults {
    pub runtime: Option<Text>,
    pub model: Option<Text>,
    pub effort: Option<Text>,
    pub lifecycle: Option<RawLifecycle>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawLifecycle {
    pub restore_policy: Option<Text>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawImport {
    #[serde(rename = "ref")]
    pub reference: Option<Text>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawProfile {
    pub preferences: Option<RawPreferences>,
    pub uses: Option<RawUses>,
    pub startup: Option<RawStartup>,
    pub lifecycle: Option<RawLifecycle>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawPreferences {
    pub runtime: Option<Text>,
    pub model: Option<Text>,
    pub effort: Option<Text>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawUses {
    pub skills: Option<Vec<Text>>,
    pub guidance: Option<Vec<Text>>,
    pub subagents: Option<Vec<Text>>,
    pub plugins: Option<Vec<Text>>,
    pub runtime_resources: Option<Vec<Text>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawResources {
    pub skills: Option<Vec<RawResource>>,
    pub guidance: Option<Vec<RawResource>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawResource {
    pub id: Option<Text>,
    pub path: Option<Text>,
}

/// The items of an optional list of scalars, trimmed, blanks dropped.
pub fn list(v: &Option<Vec<Text>>) -> Vec<&str> {
    v.iter()
        .flatten()
        .map(|t| t.0.trim())
        .filter(|s| !s.is_empty())
        .collect()
}
