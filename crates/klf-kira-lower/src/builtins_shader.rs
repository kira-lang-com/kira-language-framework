//! The `klf.shader.*` wrapper signatures (`KLF-KSL.md` section 6.6), apart
//! from the KIR builder's because a shader definition reaches only these.
//! `x` is a `String`, passed to the service as its bytes and length, and `k`
//! the `ShaderModule` a `ShaderLanguage`'s `lower` returns, and `o` the
//! `ShaderUnit` an out's `emit` reads.

pub(crate) static SHADER_WRAPPER_SPECS: &[(&str, &[&str])] = &[
    ("supx->i", &["klf.shader.module.new"]),
    ("i->k", &["klf.shader.module.finish"]),
    (
        "iiii->-",
        &[
            "klf.shader.span",
            "klf.shader.entry.workgroup_override",
            "klf.shader.stmt.store",
        ],
    ),
    (
        "ii->-",
        &[
            "klf.shader.push",
            "klf.shader.group.add",
            "klf.shader.option",
            "klf.shader.stmt.image_store",
            "klf.shader.stmt.else",
            "klf.shader.stmt.end_if",
            "klf.shader.stmt.loop",
            "klf.shader.stmt.continuing",
            "klf.shader.stmt.end_case",
            "klf.shader.stmt.end_switch",
            "klf.shader.stmt.break",
            "klf.shader.stmt.continue",
            "klf.shader.stmt.kill",
        ],
    ),
    ("ix->i", &["klf.shader.type"]),
    (
        "iiix->i",
        &[
            "klf.shader.type.array",
            "klf.shader.expr.swizzle",
            "klf.shader.expr.convert",
        ],
    ),
    ("ixixiii->-", &["klf.shader.type.member"]),
    ("ixx->i", &["klf.shader.type.struct"]),
    (
        "ixii->i",
        &["klf.shader.type.struct_explicit", "klf.shader.constant"],
    ),
    (
        "iix->i",
        &[
            "klf.shader.type.size",
            "klf.shader.type.align",
            "klf.shader.expr.math",
            "klf.shader.expr.image_sample",
        ],
    ),
    ("ixiii->i", &["klf.shader.override"]),
    ("ixxiii->i", &["klf.shader.global"]),
    ("iix->-", &["klf.shader.slot"]),
    ("ixix->-", &["klf.shader.param"]),
    ("ixix->i", &["klf.shader.function"]),
    ("ixxix->i", &["klf.shader.entry"]),
    ("iiiii->-", &["klf.shader.entry.workgroup"]),
    (
        "ii->i",
        &["klf.shader.function.finish", "klf.shader.expr.image_load"],
    ),
    ("ixx->-", &["klf.shader.group", "klf.shader.shader"]),
    ("iixx->i", &["klf.shader.expr.literal"]),
    (
        "iii->i",
        &[
            "klf.shader.expr.zero",
            "klf.shader.expr.compose",
            "klf.shader.expr.argument",
            "klf.shader.expr.global",
            "klf.shader.expr.local",
            "klf.shader.expr.constant",
            "klf.shader.expr.override",
            "klf.shader.expr.load",
            "klf.shader.expr.array_length",
            "klf.shader.stmt.call",
        ],
    ),
    (
        "iiii->i",
        &[
            "klf.shader.expr.splat",
            "klf.shader.expr.access",
            "klf.shader.expr.access_index",
            "klf.shader.expr.bitcast",
        ],
    ),
    (
        "iixi->i",
        &["klf.shader.expr.unary", "klf.shader.expr.relational"],
    ),
    ("iixii->i", &["klf.shader.expr.binary", "klf.shader.local"]),
    ("iiiii->i", &["klf.shader.expr.select"]),
    ("iixxi->i", &["klf.shader.expr.derivative"]),
    ("iiixi->i", &["klf.shader.expr.image_query"]),
    ("iiix->-", &["klf.shader.expr.name"]),
    ("iiixiii->i", &["klf.shader.stmt.atomic"]),
    ("iixx->-", &["klf.shader.stmt.barrier"]),
    (
        "iii->-",
        &[
            "klf.shader.stmt.if",
            "klf.shader.stmt.end_loop",
            "klf.shader.stmt.switch",
            "klf.shader.stmt.return",
        ],
    ),
    ("iixi->-", &["klf.shader.stmt.case"]),
    ("oxi->i", &["klf.shader.read.count"]),
    ("oxii->i", &["klf.shader.read.kind"]),
    (
        "oxiixi->i",
        &["klf.shader.read.int", "klf.shader.read.text"],
    ),
    ("ox->-", &["klf.shader.out.write"]),
    ("oxii->-", &["klf.shader.out.refuse"]),
    ("oxxii->-", &["klf.shader.out.refuse_at"]),
];
