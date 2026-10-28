//! ABI surface, freeze and apply (section 24.4).
//!
//! A module's interface evolves additively: a shape that was ever frozen is
//! never changed or removed. `freeze` computes the delta from the last frozen
//! record; `apply` stamps superseded shapes so old and new callers bind to
//! different symbols.

use std::collections::BTreeMap;

use crate::emit::canonical_text;
use crate::module::{AnnArg, AnnValue, Annotation, Item, Module};
use crate::sha256::{hex, sha256};

/// One frozen declaration shape: its path (the declaration name) and its
/// canonical signature text (with `@Abi` stamps removed).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Shape {
    pub path: String,
    pub text: String,
}

/// The exported surface of a module: the set of shapes it presents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Surface {
    pub shapes: Vec<Shape>,
}

impl Surface {
    #[must_use]
    pub fn from_module(module: &Module) -> Self {
        let mut shapes: Vec<Shape> = module
            .items
            .iter()
            .map(|item| Shape {
                path: crate::emit::item_path_of(item),
                text: shape_text(item),
            })
            .collect();
        shapes.sort();
        shapes.dedup();
        Self { shapes }
    }

    fn contains(&self, shape: &Shape) -> bool {
        self.shapes.iter().any(|s| s == shape)
    }
}

/// A single frozen ABI version and the shapes it introduced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbiRecord {
    pub uuid: String,
    pub added: Vec<Shape>,
    pub superseded: Vec<Shape>,
}

/// The ordered history of frozen ABI records for a module.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AbiHistory {
    pub records: Vec<AbiRecord>,
}

impl AbiHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every shape ever frozen, mapped to the UUID of the record that
    /// introduced it.
    #[must_use]
    pub fn frozen(&self) -> BTreeMap<Shape, String> {
        let mut map = BTreeMap::new();
        for record in &self.records {
            for shape in &record.added {
                map.entry(shape.clone())
                    .or_insert_with(|| record.uuid.clone());
            }
        }
        map
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AbiError {
    /// A frozen shape is missing from the current surface: it was changed or
    /// removed in place instead of being kept as a stamped forwarder.
    Broken { path: String, text: String },
}

impl std::fmt::Display for AbiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AbiError::Broken { path, text } => write!(
                f,
                "ridl.abi-broken: frozen shape for `{path}` was changed or removed: `{text}`"
            ),
        }
    }
}

impl std::error::Error for AbiError {}

/// The outcome of a freeze: either nothing changed, or a new record plus the
/// origin UUID of each superseded shape (so `apply` can stamp them).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Freeze {
    Unchanged,
    New {
        record: AbiRecord,
        superseded_origin: Vec<(Shape, String)>,
    },
}

/// Compute the freeze delta for `current` against `history`, minting `new_uuid`
/// for any newly added shapes.
///
/// # Errors
/// Returns [`AbiError::Broken`] when a previously frozen shape is absent from
/// the current surface.
pub fn freeze(current: &Surface, history: &AbiHistory, new_uuid: &str) -> Result<Freeze, AbiError> {
    let frozen = history.frozen();

    for shape in frozen.keys() {
        if !current.contains(shape) {
            return Err(AbiError::Broken {
                path: shape.path.clone(),
                text: shape.text.clone(),
            });
        }
    }

    let added: Vec<Shape> = current
        .shapes
        .iter()
        .filter(|s| !frozen.contains_key(*s))
        .cloned()
        .collect();

    if added.is_empty() {
        return Ok(Freeze::Unchanged);
    }

    let added_paths: std::collections::BTreeSet<&str> =
        added.iter().map(|s| s.path.as_str()).collect();
    let mut superseded = Vec::new();
    let mut superseded_origin = Vec::new();
    for (shape, origin) in &frozen {
        if added_paths.contains(shape.path.as_str()) {
            superseded.push(shape.clone());
            superseded_origin.push((shape.clone(), origin.clone()));
        }
    }

    Ok(Freeze::New {
        record: AbiRecord {
            uuid: new_uuid.to_owned(),
            added,
            superseded,
        },
        superseded_origin,
    })
}

/// Stamp each superseded shape in `module` with `@Abi(version: "<origin>")`,
/// leaving new shapes unstamped. Returns the number of items stamped.
pub fn apply(module: &mut Module, superseded_origin: &[(Shape, String)]) -> usize {
    let mut stamped = 0;
    for item in &mut module.items {
        let path = crate::emit::item_path_of(item);
        let text = shape_text(item);
        if already_stamped(item) {
            continue;
        }
        if let Some((_, origin)) = superseded_origin
            .iter()
            .find(|(s, _)| s.path == path && s.text == text)
        {
            item.annotations.push(abi_annotation(origin));
            stamped += 1;
        }
    }
    stamped
}

fn abi_annotation(uuid: &str) -> Annotation {
    Annotation {
        name: "Abi".to_owned(),
        args: vec![AnnArg {
            label: Some("version".to_owned()),
            value: AnnValue::String(uuid.to_owned()),
        }],
    }
}

fn already_stamped(item: &Item) -> bool {
    item.annotations.iter().any(|a| a.name == "Abi")
}

/// Mint a version-4 UUID as 32 lowercase hex digits, seeded from the system
/// clock and process id (no external RNG). The version and variant nibbles are
/// set per RFC 4122.
#[must_use]
pub fn mint_uuid_v4() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seed = format!("{now}-{}", std::process::id());
    let mut bytes = sha256(seed.as_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    hex(&bytes[..16])
}

/// Serialize a freeze record to the `.ridlsig` text format.
#[must_use]
pub fn record_to_text(record: &AbiRecord, superseded_origin: &[(Shape, String)]) -> String {
    let mut out = format!("ridlsig 1\nuuid {}\nadded\n", record.uuid);
    for shape in &record.added {
        out.push_str(&shape.text);
        out.push('\n');
    }
    out.push_str("superseded\n");
    for (shape, origin) in superseded_origin {
        out.push_str(origin);
        out.push(' ');
        out.push_str(&shape.text);
        out.push('\n');
    }
    out
}

/// Load the `added` shapes of a `.ridlsig` record into a partial [`AbiRecord`].
/// `parse_shape` recovers each shape's path from its canonical text.
///
/// # Errors
/// Returns the offending line when the header or a shape line is malformed.
pub fn record_from_text(
    text: &str,
    mut parse_shape: impl FnMut(&str) -> Option<String>,
) -> Result<AbiRecord, String> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    if !header.starts_with("ridlsig") {
        return Err(format!("bad .ridlsig header: `{header}`"));
    }
    let uuid_line = lines.next().unwrap_or_default();
    let uuid = uuid_line
        .strip_prefix("uuid ")
        .ok_or_else(|| format!("expected uuid line, got `{uuid_line}`"))?
        .trim()
        .to_owned();
    let mut added = Vec::new();
    let mut in_added = false;
    for line in lines {
        match line.trim() {
            "added" => in_added = true,
            "superseded" => in_added = false,
            "" => {}
            shape if in_added => {
                let path = parse_shape(shape)
                    .ok_or_else(|| format!("cannot recover path of shape `{shape}`"))?;
                added.push(Shape {
                    path,
                    text: shape.to_owned(),
                });
            }
            _ => {}
        }
    }
    Ok(AbiRecord {
        uuid,
        added,
        superseded: Vec::new(),
    })
}

/// The canonical shape text of a single item, with any `@Abi` stamp removed so
/// stamping does not change a shape's identity.
fn shape_text(item: &Item) -> String {
    let mut clone = item.clone();
    clone.annotations.retain(|a| a.name != "Abi");
    let mut module = Module::new("");
    module.items.push(clone);
    canonical_text(&module)
}
