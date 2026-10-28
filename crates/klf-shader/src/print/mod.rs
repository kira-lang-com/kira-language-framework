//! Textual KSIR (section 18, Appendix A).

mod body;

use crate::Span;
use crate::arena::{Arena, Handle};
use crate::ir::{Expression, ImageQuery, Literal, SampleLevel};
use crate::module::{Function, Module, TargetSlot, WorkgroupExtent};
use crate::types::{AddressSpace, ArraySize, Binding, ImageClass, LayoutRule, Type, TypeInner};
use body::BodyPrinter;
use std::collections::HashMap;
use std::fmt::Write;

/// The canonical text of `module`.
#[must_use]
pub fn print(module: &Module) -> String {
    let mut printer = Printer {
        module,
        out: String::new(),
    };
    printer.module();
    printer.out
}

/// The KSIR spelling of `handle`, as the module writes it.
#[must_use]
pub fn type_text(module: &Module, handle: Handle<Type>) -> String {
    Printer {
        module,
        out: String::new(),
    }
    .ty(handle)
}

/// The KSIR spelling of a global expression, as the module writes it.
#[must_use]
pub fn expression_text(module: &Module, handle: Handle<Expression>) -> String {
    let Some(expression) = module.global_expressions.get(handle) else {
        return String::new();
    };
    Printer {
        module,
        out: String::new(),
    }
    .expression(expression, None)
}

struct Printer<'a> {
    module: &'a Module,
    out: String,
}

/// A float's shortest decimal that reads back to the same bits.
#[must_use]
pub fn float_text(value: f64, single: bool) -> String {
    let text = if single {
        format!("{:?}", value as f32)
    } else {
        format!("{value:?}")
    };
    if text.contains('.') || text.contains('e') || text.contains("inf") || text.contains("NaN") {
        text
    } else {
        format!("{text}.0")
    }
}

#[must_use]
pub fn literal_text(literal: Literal) -> String {
    match literal {
        Literal::Bool(value) => value.to_string(),
        Literal::I32(value) => format!("{value}i32"),
        Literal::U32(value) => format!("{value}u32"),
        Literal::I64(value) => format!("{value}i64"),
        Literal::U64(value) => format!("{value}u64"),
        Literal::F16(bits) => format!("{}f16", float_text(f64::from(half_to_f32(bits)), true)),
        Literal::F32(bits) => format!("{}f32", float_text(f64::from(f32::from_bits(bits)), true)),
        Literal::F64(bits) => format!("{}f64", float_text(f64::from_bits(bits), false)),
    }
}

/// Converts IEEE binary16 bits to `f32`.
#[must_use]
pub fn half_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let mantissa = u32::from(bits & 0x3ff);
    let value = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, _) => {
            let mut exponent = 127 - 15 + 1;
            let mut mantissa = mantissa;
            while mantissa & 0x400 == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            sign | (exponent << 23) | ((mantissa & 0x3ff) << 13)
        }
        (0x1f, _) => sign | 0x7f80_0000 | (mantissa << 13),
        _ => sign | ((exponent + 127 - 15) << 23) | (mantissa << 13),
    };
    f32::from_bits(value)
}

impl Printer<'_> {
    fn loc(&self, span: Span) -> String {
        if !span.is_known() {
            return String::new();
        }
        let path = self
            .module
            .files
            .get(span.file as usize)
            .map_or("", String::as_str);
        format!(" !loc({path:?}:{}:{})", span.line, span.column)
    }

    fn constant(&mut self, handle: Handle<crate::Constant>) {
        let module = self.module;
        let constant = &module.constants[handle];
        let _ = writeln!(
            self.out,
            "const @{}: {} = %g{}{}",
            constant.name,
            self.ty(constant.ty),
            constant.init.raw(),
            self.loc(module.constants.span(handle))
        );
    }

    fn override_item(&mut self, handle: Handle<crate::Override>) {
        let module = self.module;
        let item = &module.overrides[handle];
        let _ = write!(self.out, "override @{}: {}", item.name, self.ty(item.ty));
        if let Some(id) = item.id {
            let _ = write!(self.out, " id({id})");
        }
        if let Some(init) = item.init {
            let _ = write!(self.out, " = %g{}", init.raw());
        }
        let _ = writeln!(self.out, "{}", self.loc(module.overrides.span(handle)));
    }

    fn module(&mut self) {
        let module = self.module;
        let capabilities: Vec<&str> = module
            .capabilities
            .iter()
            .map(|capability| capability.name())
            .collect();
        let _ = writeln!(
            self.out,
            "ksir 1\nmodule @{} lang {:?} capabilities({})",
            module.name,
            module.lang,
            capabilities.join(", ")
        );
        for (handle, ty) in module.types.iter() {
            if let (
                Some(name),
                TypeInner::Struct {
                    rule,
                    members,
                    size,
                    align,
                },
            ) = (&ty.name, &ty.inner)
            {
                let _ = write!(self.out, "\ntype %{name} = struct {}", rule.name());
                if *rule != LayoutRule::None {
                    let _ = write!(self.out, " size {size} align {align}");
                }
                self.out.push_str(" {");
                for (index, member) in members.iter().enumerate() {
                    let separator = if index + 1 < members.len() { "," } else { "" };
                    let _ = write!(self.out, "\n  {}: {}", member.name, self.ty(member.ty));
                    if *rule != LayoutRule::None {
                        let _ = write!(
                            self.out,
                            " offset {} size {} align {}",
                            member.offset, member.size, member.align
                        );
                    }
                    if let Some(binding) = &member.binding {
                        let _ = write!(self.out, " {}", binding_text(binding));
                    }
                    self.out.push_str(separator);
                }
                let _ = writeln!(self.out, "\n}}{}", self.loc(module.types.span(handle)));
            }
        }
        if !module.global_expressions.is_empty() {
            self.out.push('\n');
        }
        // Each constant and override follows its initializer, so a later
        // expression can name it; arena order is kept within each kind.
        let constants: Vec<_> = module.constants.iter().map(|(handle, _)| handle).collect();
        let overrides: Vec<_> = module.overrides.iter().map(|(handle, _)| handle).collect();
        let (mut constant, mut item) = (0, 0);
        for (handle, expression) in module.global_expressions.iter() {
            while let Some(next) = constants.get(constant)
                && module.constants[*next].init.raw() < handle.raw()
            {
                self.constant(*next);
                constant += 1;
            }
            while let Some(next) = overrides.get(item)
                && module.overrides[*next]
                    .init
                    .is_none_or(|init| init.raw() < handle.raw())
            {
                self.override_item(*next);
                item += 1;
            }
            let _ = writeln!(
                self.out,
                "%g{} = {}{}",
                handle.raw(),
                self.expression(expression, None),
                self.loc(module.global_expressions.span(handle))
            );
        }
        for next in &constants[constant..] {
            self.constant(*next);
        }
        for next in &overrides[item..] {
            self.override_item(*next);
        }
        if !module.globals.is_empty() {
            self.out.push('\n');
        }
        for (handle, global) in module.globals.iter() {
            let _ = write!(
                self.out,
                "global @{}: {} space({})",
                global.name,
                self.ty(global.ty),
                space_text(global.space)
            );
            if let Some(binding) = global.binding {
                let _ = write!(self.out, " binding({}, {})", binding.group, binding.binding);
            }
            for slot in &global.slots {
                let _ = write!(self.out, " slot({})", slot_text(*slot));
            }
            if let Some(init) = global.init {
                let _ = write!(self.out, " = %g{}", init.raw());
            }
            let _ = writeln!(self.out, "{}", self.loc(module.globals.span(handle)));
        }
        for shader in &module.shaders {
            let _ = writeln!(
                self.out,
                "\nshader @{} {} {{",
                shader.name,
                shader.kind.name()
            );
            for group in &shader.groups {
                let globals: Vec<String> = group
                    .globals
                    .iter()
                    .map(|global| format!("@{}", module.globals[*global].name))
                    .collect();
                let _ = writeln!(
                    self.out,
                    "  group @{} class({}) {{ {} }}",
                    group.name,
                    group.class.name(),
                    globals.join(", ")
                );
            }
            let entries: Vec<String> = shader
                .entries
                .iter()
                .map(|entry| format!("@{}", module.entry_points[*entry].name))
                .collect();
            let _ = writeln!(self.out, "  entries {{ {} }}", entries.join(", "));
            if !shader.options.is_empty() {
                let options: Vec<String> = shader
                    .options
                    .iter()
                    .map(|item| format!("@{}", module.overrides[*item].name))
                    .collect();
                let _ = writeln!(self.out, "  options {{ {} }}", options.join(", "));
            }
            let _ = writeln!(self.out, "}}{}", self.loc(shader.span));
        }
        for (handle, function) in module.functions.iter() {
            let header = format!("func @{}", function.name);
            self.function(&header, function, "", module.functions.span(handle));
        }
        for entry in &module.entry_points {
            let header = format!("entry {} @{}", entry.stage.name(), entry.name);
            let mut suffix = String::new();
            if let Some(size) = &entry.workgroup_size {
                let extents: Vec<String> = size
                    .iter()
                    .map(|extent| match extent {
                        WorkgroupExtent::Fixed(value) => value.to_string(),
                        WorkgroupExtent::Override(expression) => format!("%g{}", expression.raw()),
                    })
                    .collect();
                let _ = write!(suffix, " workgroup({})", extents.join(", "));
            }
            if let Some(test) = entry.early_depth_test {
                let _ = write!(suffix, " early_depth({})", test.name());
            }
            self.function(&header, &entry.function, &suffix, entry.span);
        }
    }

    fn ty(&self, handle: Handle<Type>) -> String {
        let Some(ty) = self.module.types.get(handle) else {
            return format!("%<missing {}>", handle.raw());
        };
        if let Some(name) = &ty.name {
            return format!("%{name}");
        }
        match &ty.inner {
            TypeInner::Scalar(scalar) => scalar.text(),
            TypeInner::Vector { size, scalar } => format!("vec{size}<{}>", scalar.text()),
            TypeInner::Matrix {
                columns,
                rows,
                scalar,
            } => format!("mat{columns}x{rows}<{}>", scalar.text()),
            TypeInner::Atomic(scalar) => format!("atomic<{}>", scalar.text()),
            TypeInner::Pointer { base, space } => {
                format!("ptr<{}, {}>", space_text(*space), self.ty(*base))
            }
            TypeInner::Array { base, size, stride } => {
                let size = match size {
                    ArraySize::Constant(count) => count.to_string(),
                    ArraySize::Override(item) => format!("@{}", self.module.overrides[*item].name),
                    ArraySize::Runtime => "runtime".to_owned(),
                };
                format!("array<{}, {size}, stride {stride}>", self.ty(*base))
            }
            TypeInner::Struct { .. } => format!("%_{}", handle.raw()),
            TypeInner::Image {
                dim,
                arrayed,
                class,
            } => {
                let arrayed = if *arrayed { ", arrayed" } else { "" };
                let class = match class {
                    ImageClass::Sampled { kind, multisampled } => {
                        let multisampled = if *multisampled { ", multisampled" } else { "" };
                        format!("sampled<{}{multisampled}>", kind.name())
                    }
                    ImageClass::Depth { multisampled: true } => "depth<multisampled>".to_owned(),
                    ImageClass::Depth {
                        multisampled: false,
                    } => "depth".to_owned(),
                    ImageClass::Storage { format, access } => {
                        format!("storage<{}, {}>", format.name(), access.words().join(" "))
                    }
                };
                format!("image<{}{arrayed}, {class}>", dim.name())
            }
            TypeInner::Sampler { comparison: true } => "sampler<comparison>".to_owned(),
            TypeInner::Sampler { comparison: false } => "sampler".to_owned(),
        }
    }

    fn function(&mut self, header: &str, function: &Function, suffix: &str, span: Span) {
        let arguments: Vec<String> = function
            .arguments
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                let name = argument.name.clone().unwrap_or_else(|| format!("_{index}"));
                let binding = argument
                    .binding
                    .as_ref()
                    .map(|binding| format!(" {}", binding_text(binding)))
                    .unwrap_or_default();
                format!("%{name}: {}{binding}", self.ty(argument.ty))
            })
            .collect();
        let _ = write!(self.out, "\n{header}({})", arguments.join(", "));
        if let Some(result) = &function.result {
            let _ = write!(self.out, " -> {}", self.ty(result.ty));
            if let Some(binding) = &result.binding {
                let _ = write!(self.out, " {}", binding_text(binding));
            }
        }
        let _ = writeln!(self.out, "{suffix} {{{}", self.loc(span));
        let names = local_names(&function.locals);
        for (handle, local) in function.locals.iter() {
            let _ = write!(
                self.out,
                "  local ${}: {}",
                names[handle.index()],
                self.ty(local.ty)
            );
            if let Some(init) = local.init {
                let _ = write!(self.out, " = %{}", init.raw());
            }
            let _ = writeln!(self.out, "{}", self.loc(function.locals.span(handle)));
        }
        let mut body = BodyPrinter {
            printer: self,
            function,
            names,
            next: 0,
            out: String::new(),
        };
        body.block(&function.body, 1);
        body.pending(function.expressions.len() as u32, 1);
        let out = body.out;
        self.out.push_str(&out);
        self.out.push_str("}\n");
    }

    fn expression(&self, expression: &Expression, locals: Option<&[String]>) -> String {
        let module = self.module;
        let e = |handle: &Handle<Expression>| {
            if locals.is_some() {
                format!("%{}", handle.raw())
            } else {
                format!("%g{}", handle.raw())
            }
        };
        let list =
            |handles: &[Handle<Expression>]| handles.iter().map(e).collect::<Vec<_>>().join(", ");
        match expression {
            Expression::Literal(literal) => format!("literal {}", literal_text(*literal)),
            Expression::Constant(constant) => {
                format!("constant @{}", module.constants[*constant].name)
            }
            Expression::Override(item) => format!("override @{}", module.overrides[*item].name),
            Expression::ZeroValue(ty) => format!("zero {}", self.ty(*ty)),
            Expression::Compose { ty, components } => {
                format!("compose {}, [{}]", self.ty(*ty), list(components))
            }
            Expression::Access { base, index } => format!("access {}, {}", e(base), e(index)),
            Expression::AccessIndex { base, index } => format!("access_index {}, {index}", e(base)),
            Expression::Splat { size, value } => format!("splat {size}, {}", e(value)),
            Expression::Swizzle { vector, pattern } => {
                let components: Vec<&str> = pattern
                    .iter()
                    .map(|component| ["x", "y", "z", "w"][usize::from(*component)])
                    .collect();
                format!("swizzle {}, [{}]", e(vector), components.join(", "))
            }
            Expression::FunctionArgument(index) => format!("arg {index}"),
            Expression::GlobalVariable(global) => {
                format!("global @{}", module.globals[*global].name)
            }
            Expression::LocalVariable(local) => format!(
                "local ${}",
                locals
                    .and_then(|names| names.get(local.index()))
                    .map_or("?", String::as_str)
            ),
            Expression::Load { pointer } => format!("load {}", e(pointer)),
            Expression::ImageSample {
                image,
                sampler,
                gather,
                coordinate,
                array_index,
                offset,
                level,
                depth_ref,
            } => {
                let mut text = format!(
                    "image_sample {}, {}, {}",
                    e(image),
                    e(sampler),
                    e(coordinate)
                );
                if let Some(index) = array_index {
                    let _ = write!(text, ", array_index({})", e(index));
                }
                if let Some(offset) = offset {
                    let _ = write!(text, ", offset({})", e(offset));
                }
                let level = match level {
                    SampleLevel::Auto => "auto".to_owned(),
                    SampleLevel::Zero => "zero".to_owned(),
                    SampleLevel::Exact(value) => format!("exact, {}", e(value)),
                    SampleLevel::Bias(value) => format!("bias, {}", e(value)),
                    SampleLevel::Gradient { x, y } => format!("gradient, {}, {}", e(x), e(y)),
                };
                let _ = write!(text, ", level({level})");
                if let Some(depth) = depth_ref {
                    let _ = write!(text, ", depth_ref({})", e(depth));
                }
                if let Some(component) = gather {
                    let _ = write!(
                        text,
                        ", gather({})",
                        ["x", "y", "z", "w"][usize::from(*component)]
                    );
                }
                text
            }
            Expression::ImageLoad {
                image,
                coordinate,
                array_index,
                sample,
                level,
            } => {
                let mut text = format!("image_load {}, {}", e(image), e(coordinate));
                for (name, operand) in [
                    ("array_index", array_index),
                    ("sample", sample),
                    ("level", level),
                ] {
                    if let Some(operand) = operand {
                        let _ = write!(text, ", {name}({})", e(operand));
                    }
                }
                text
            }
            Expression::ImageQuery { image, query } => {
                let query = match query {
                    ImageQuery::Size { level: Some(level) } => format!("size({})", e(level)),
                    ImageQuery::Size { level: None } => "size".to_owned(),
                    ImageQuery::NumLevels => "levels".to_owned(),
                    ImageQuery::NumLayers => "layers".to_owned(),
                    ImageQuery::NumSamples => "samples".to_owned(),
                };
                format!("image_query {}, {query}", e(image))
            }
            Expression::Unary { op, expr } => format!("unary {} {}", op.name(), e(expr)),
            Expression::Binary { op, left, right } => {
                format!("binary {} {}, {}", op.name(), e(left), e(right))
            }
            Expression::Select {
                condition,
                accept,
                reject,
            } => {
                format!("select {}, {}, {}", e(condition), e(accept), e(reject))
            }
            Expression::Derivative {
                axis,
                control,
                expr,
            } => {
                format!("derivative {} {} {}", axis.name(), control.name(), e(expr))
            }
            Expression::Relational { fun, argument } => {
                format!("relational {} {}", fun.name(), e(argument))
            }
            Expression::Math { fun, args } => format!("math {} {}", fun.name(), list(args)),
            Expression::Convert { expr, kind, width } => match width {
                Some(width) => format!("convert {}, {}, {width}", e(expr), kind.name()),
                None => format!("convert {}, {}", e(expr), kind.name()),
            },
            Expression::Bitcast { expr, ty } => format!("bitcast {}, {}", e(expr), self.ty(*ty)),
            Expression::CallResult(function) => {
                format!("call_result @{}", module.functions[*function].name)
            }
            Expression::AtomicResult { ty } => format!("atomic_result {}", self.ty(*ty)),
            Expression::WorkgroupUniformLoadResult { ty } => {
                format!("workgroup_uniform_load_result {}", self.ty(*ty))
            }
            Expression::ArrayLength(pointer) => format!("array_length {}", e(pointer)),
        }
    }
}

/// Each local's printed name: its source name, with `.N` on the `N`th
/// repeat, or `_N` when it has none.
fn local_names(locals: &Arena<crate::module::LocalVariable>) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    locals
        .iter()
        .map(|(handle, local)| match &local.name {
            Some(name) => {
                let count = seen.entry(name.clone()).or_default();
                *count += 1;
                if *count == 1 {
                    name.clone()
                } else {
                    format!("{name}.{}", *count - 1)
                }
            }
            None => format!("_{}", handle.raw()),
        })
        .collect()
}

#[must_use]
pub fn space_text(space: AddressSpace) -> String {
    match space {
        AddressSpace::Function => "function".to_owned(),
        AddressSpace::Private => "private".to_owned(),
        AddressSpace::Workgroup => "workgroup".to_owned(),
        AddressSpace::Uniform => "uniform".to_owned(),
        AddressSpace::Storage { access } => format!("storage<{}>", access.words().join(" ")),
        AddressSpace::Handle => "handle".to_owned(),
        AddressSpace::Immediate => "immediate".to_owned(),
    }
}

#[must_use]
pub fn binding_text(binding: &Binding) -> String {
    match binding {
        Binding::BuiltIn(builtin) => format!("@builtin({})", builtin.name()),
        Binding::Location {
            location,
            interpolation,
            sampling,
            blend_src,
        } => {
            let mut text = match location {
                Some(location) => format!("@location({location})"),
                None => "@location(auto)".to_owned(),
            };
            if let Some(interpolation) = interpolation {
                let _ = write!(text, " @interpolate({})", interpolation.name());
            }
            if let Some(sampling) = sampling {
                let _ = write!(text, " @sampling({})", sampling.name());
            }
            if let Some(source) = blend_src {
                let _ = write!(text, " @blend_src({source})");
            }
            text
        }
    }
}

fn slot_text(slot: TargetSlot) -> String {
    match slot {
        TargetSlot::Universal(value) => format!("universal {value}"),
        TargetSlot::MslBuffer(value) => format!("msl.buffer {value}"),
        TargetSlot::MslTexture(value) => format!("msl.texture {value}"),
        TargetSlot::MslSampler(value) => format!("msl.sampler {value}"),
        TargetSlot::Hlsl {
            class,
            register,
            space,
        } => format!("hlsl {} {register} {space}", class.name()),
        TargetSlot::Glsl(value) => format!("glsl {value}"),
    }
}
