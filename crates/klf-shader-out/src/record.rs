//! One record of a table, and the fields each table may have. A field a
//! table lists but a record lacks reads as absent; a field the table does
//! not list is a misuse, so a misspelt name fails rather than reading -1.

use klf_shader::Handle;

#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    pub(crate) kind: String,
    ints: Vec<(&'static str, Vec<i64>)>,
    texts: Vec<(&'static str, Vec<String>)>,
}

impl Record {
    pub(crate) fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ..Self::default()
        }
    }

    pub(crate) fn int(&mut self, field: &'static str, value: impl Into<i64>) -> &mut Self {
        self.ints.push((field, vec![value.into()]));
        self
    }

    pub(crate) fn ints(&mut self, field: &'static str, values: Vec<i64>) -> &mut Self {
        self.ints.push((field, values));
        self
    }

    pub(crate) fn flag(&mut self, field: &'static str, value: bool) -> &mut Self {
        self.int(field, i64::from(value))
    }

    /// A handle field, absent when `handle` is.
    pub(crate) fn handle<T>(
        &mut self,
        field: &'static str,
        handle: Option<Handle<T>>,
    ) -> &mut Self {
        if let Some(handle) = handle {
            self.int(field, handle.index() as i64);
        }
        self
    }

    pub(crate) fn text(&mut self, field: &'static str, value: impl Into<String>) -> &mut Self {
        self.texts.push((field, vec![value.into()]));
        self
    }

    pub(crate) fn texts(&mut self, field: &'static str, values: Vec<String>) -> &mut Self {
        self.texts.push((field, values));
        self
    }

    pub(crate) fn optional_text(&mut self, field: &'static str, value: Option<&str>) -> &mut Self {
        if let Some(value) = value {
            self.text(field, value);
        }
        self
    }

    pub(crate) fn finish(&mut self) -> Self {
        std::mem::take(self)
    }

    pub(crate) fn ints_of(&self, field: &str) -> &[i64] {
        self.ints
            .iter()
            .find(|(name, _)| *name == field)
            .map_or(&[], |(_, values)| values.as_slice())
    }

    pub(crate) fn texts_of(&self, field: &str) -> &[String] {
        self.texts
            .iter()
            .find(|(name, _)| *name == field)
            .map_or(&[], |(_, values)| values.as_slice())
    }
}

/// Every table, in the order section 6.7 lists them.
pub const TABLES: &[&str] = &[
    "unit",
    "groups",
    "types",
    "members",
    "constants",
    "overrides",
    "globals",
    "functions",
    "arguments",
    "locals",
    "expressions",
    "statements",
    "values",
];

/// The fields of each table, integer and text alike.
pub const FIELDS: &[(&str, &[&str])] = &[
    (
        "unit",
        &[
            "name",
            "shader",
            "target",
            "lang",
            "stage",
            "entries",
            "options",
            "debug",
            "capabilities",
            "files",
        ],
    ),
    ("groups", &["name", "globals"]),
    (
        "types",
        &[
            "name",
            "scalar",
            "lanes",
            "columns",
            "rows",
            "base",
            "space",
            "access",
            "length",
            "length_override",
            "stride",
            "size",
            "align",
            "rule",
            "host_shared",
            "dimension",
            "arrayed",
            "class",
            "multisampled",
            "format",
            "comparison",
            "size_std140",
            "align_std140",
            "size_std430",
            "align_std430",
            "size_msl",
            "align_msl",
        ],
    ),
    (
        "members",
        &[
            "name",
            "type",
            "offset",
            "size",
            "align",
            "builtin",
            "location",
            "interpolation",
            "sampling",
            "blend_src",
        ],
    ),
    ("constants", &["name", "type", "init", "value"]),
    ("overrides", &["name", "type", "id", "init", "value"]),
    (
        "globals",
        &[
            "name",
            "type",
            "space",
            "access",
            "init",
            "group",
            "binding",
            "used",
            "read",
            "written",
            "resource",
            "slot",
            "slot_space",
            "slot_class",
            "slot_name",
            "count_slot",
            "argument",
            "vertex_argument",
            "paired_sampler",
            "value",
        ],
    ),
    (
        "functions",
        &[
            "name",
            "result",
            "result_builtin",
            "result_location",
            "result_interpolation",
            "result_sampling",
            "result_blend_src",
            "body",
            "arguments",
            "locals",
            "expressions",
            "stage",
            "workgroup",
            "workgroup_override",
            "early_depth_test",
            "calls",
            "uses",
            "may_kill",
        ],
    ),
    (
        "arguments",
        &[
            "name",
            "type",
            "builtin",
            "location",
            "interpolation",
            "sampling",
            "blend_src",
        ],
    ),
    ("locals", &["name", "type", "init"]),
    (
        "expressions",
        &[
            "type",
            "shaped",
            "ref_count",
            "bake",
            "emits",
            "line",
            "named",
            "literal",
            "scalar",
            "float",
            "bits",
            "finite",
            "constant",
            "override",
            "items",
            "base",
            "index",
            "value",
            "vector",
            "component",
            "pattern",
            "lanes",
            "argument",
            "global",
            "local",
            "pointer",
            "image",
            "sampler",
            "coordinate",
            "array_index",
            "offset",
            "level",
            "level_value",
            "gradient_x",
            "gradient_y",
            "depth_ref",
            "gather",
            "sample",
            "query",
            "op",
            "expr",
            "left",
            "right",
            "condition",
            "accept",
            "reject",
            "axis",
            "control",
            "fun",
            "function",
        ],
    ),
    (
        "statements",
        &[
            "line",
            "file",
            "first",
            "last",
            "body",
            "condition",
            "accept",
            "reject",
            "selector",
            "case_block",
            "case_value",
            "case_fall",
            "case_ends",
            "continuing",
            "break_if",
            "value",
            "pointer",
            "image",
            "coordinate",
            "array_index",
            "fun",
            "compare",
            "result",
            "function",
            "arguments",
            "flags",
        ],
    ),
    (
        "values",
        &[
            "type", "items", "literal", "scalar", "float", "bits", "finite",
        ],
    ),
];
