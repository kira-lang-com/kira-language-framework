//! The C shims that pass aggregates by value (section 26.3).
//!
//! How a C struct crosses by value is the target ABI's to say: an `AArch64`
//! homogeneous float aggregate travels in vector registers, a small x86-64
//! one is split by eightbyte class, and a large one is copied to memory. KIR
//! passes aggregates by address, so a function whose C prototype names one by
//! value is reached through a shim written in C, and the C compiler applies
//! the ABI it defines. A foreign function `f` is called as `__klf_c_f`, which
//! takes the KIR signature and calls `f`; a function C calls, `g`, is entered
//! as `__klf_c_g`, which takes the C signature and calls `g` with addresses.
//! A call to a C variadic function goes through a shim too, the one for its
//! shape of arguments, since only C knows where a variadic argument goes. A
//! Linux system call is a shim that loads the call's number and arguments
//! into the registers the architecture's kernel entry reads.

use crate::{
    CPrototype, CType, Function, LayoutRule, Module, Type, TypeDeclaration, TypeKind, c_shim_symbol,
};
use std::collections::HashMap;
use std::fmt::Write as _;

/// The C source of every shim `module` needs, or `None` when nothing in it
/// crosses by value. `target` is the triple the shims are compiled for;
/// `symbol_prefix` is what the target's object format puts before a C name:
/// `_` on Mach-O, nothing elsewhere; `defined` is the object symbol of a
/// function the module defines.
///
/// # Errors
///
/// A record crossing by value that C cannot spell: one without C layout, or
/// holding a tuple, a vector or a type the module does not declare; and a
/// system call the target cannot make.
pub fn c_shim_source(
    module: &Module,
    target: &str,
    symbol_prefix: &str,
    defined: &dyn Fn(&str) -> String,
) -> Result<Option<String>, String> {
    let shimmed: Vec<(&Function, &CPrototype)> = module
        .functions
        .iter()
        .filter_map(|function| {
            function
                .c_prototype
                .as_ref()
                .map(|prototype| (function, prototype))
        })
        .filter(|(function, prototype)| function.foreign || prototype.needs_shim())
        .collect();
    if shimmed.is_empty() {
        return Ok(None);
    }
    let mut records = Records {
        module,
        names: HashMap::new(),
        text: String::new(),
    };
    for (_, prototype) in shimmed.iter().copied() {
        for ty in prototype.parameters.iter().chain([&prototype.result]) {
            if let CType::Value(name) = ty {
                records.declare(name.as_str())?;
            }
        }
    }
    let (declarations, calls) = foreign_declarations(&shimmed, &records, symbol_prefix)?;
    let mut functions = String::new();
    for (index, (function, prototype)) in shimmed.into_iter().enumerate() {
        let name = function.name.as_str();
        let symbol = if function.foreign {
            crate::c_symbol(name).to_owned()
        } else {
            defined(name)
        };
        // A foreign crossing calls the declaration its symbol was given, and a
        // system call names the kernel entry itself, so it has none.
        let callee = if function.foreign && prototype.syscall.is_none() {
            calls[&symbol].spelled(prototype, &records)
        } else if function.foreign {
            String::new()
        } else {
            format!("klf_kir{index}")
        };
        let shim = Shim {
            records: &records,
            prototype,
            index,
            prefix: symbol_prefix,
            name,
            symbol: &symbol,
            callee,
        };
        if let Some(syscall) = &prototype.syscall {
            shim.system_call(&mut functions, syscall.as_str(), target)?;
        } else if function.foreign {
            shim.call_foreign(&mut functions);
        } else {
            shim.enter_from_c(&mut functions);
        }
    }
    Ok(Some(format!(
        "{FIXED_WIDTH}\n{}\n{declarations}{functions}",
        records.text
    )))
}

/// The C source declaring the symbol of every foreign crossing, one
/// declaration per distinct symbol, and the C name each is declared under.
///
/// One declaration per symbol is not tidiness. A C function with a variadic
/// tail is reached through one shim per shape of arguments, and declaring the
/// same symbol under a name for each shape makes the C compiler treat the
/// calls as siblings of one function and drop what a variadic call has to
/// name: on amd64 `al` says how many arguments may be taken from the register
/// save area, so a `double` the shim passed in `xmm0` is read from the
/// overflow area instead, out of whatever the stack happened to hold. The
/// read is silent and the answer is a plausible wrong number.
///
/// A symbol with no variadic tail reached by several shapes is a trampoline,
/// as `objc_msgSend` is: one entry that each caller treats as the function of
/// its own shape. C reaches one by casting it, so it is declared once with no
/// shape of its own and every shim calls it through a pointer of its shape.
fn foreign_declarations(
    shimmed: &[(&Function, &CPrototype)],
    records: &Records<'_>,
    prefix: &str,
) -> Result<(String, HashMap<String, Callee>), String> {
    let mut reached: Vec<(String, Vec<&CPrototype>)> = Vec::new();
    for (function, prototype) in shimmed {
        // A system call is its own shim: the symbol names the kernel entry and
        // the shape names the call, so two shapes of one symbol are two
        // different ABIs rather than one declaration spelled twice. It is
        // reached by no aliased declaration here.
        if !function.foreign || prototype.syscall.is_some() {
            continue;
        }
        let symbol = crate::c_symbol(function.name.as_str()).to_owned();
        match reached.iter_mut().find(|(name, _)| *name == symbol) {
            Some((_, prototypes)) => prototypes.push(prototype),
            None => reached.push((symbol, vec![prototype])),
        }
    }
    let mut source = String::new();
    let mut calls = HashMap::new();
    for (symbol, prototypes) in reached {
        let variadic = prototypes
            .iter()
            .any(|prototype| prototype.variadic.is_some());
        let spell = |prototype: &CPrototype| {
            let fixed = prototype.variadic.unwrap_or(prototype.parameters.len());
            let mut types: Vec<String> = prototype.parameters[..fixed]
                .iter()
                .map(|ty| records.of(ty))
                .collect();
            if variadic {
                types.push("...".to_owned());
            }
            format!("{}|{}", records.of(&prototype.result), list(&types, "void"))
        };
        let first = spell(prototypes[0]);
        let name = format!("klf_foreign{}", calls.len());
        if prototypes[1..]
            .iter()
            .any(|prototype| spell(prototype) != first)
        {
            if variadic {
                return Err(format!(
                    "`{symbol}` is reached by shapes one C prototype cannot spell"
                ));
            }
            let _ = writeln!(source, "void {name}(void) __asm__(\"{prefix}{symbol}\");");
            calls.insert(symbol, Callee::Cast(name));
            continue;
        }
        let (result, types) = first.split_once('|').expect("the spelling has both halves");
        let _ = writeln!(source, "{result} {name}({types}) __asm__(\"{prefix}{symbol}\");");
        calls.insert(symbol, Callee::Declared(name));
    }
    Ok((source, calls))
}

/// How a shim names the foreign symbol it calls.
enum Callee {
    /// The declaration of the symbol's one shape.
    Declared(String),
    /// The shapeless declaration of a trampoline, cast to each caller's shape.
    Cast(String),
}

impl Callee {
    fn spelled(&self, prototype: &CPrototype, records: &Records<'_>) -> String {
        match self {
            Self::Declared(name) => name.clone(),
            Self::Cast(name) => {
                let types: Vec<String> =
                    prototype.parameters.iter().map(|ty| records.of(ty)).collect();
                format!(
                    "(({} (*)({})){name})",
                    records.of(&prototype.result),
                    list(&types, "void")
                )
            }
        }
    }
}

/// The fixed-width integer types, from the compiler's own predefined macros:
/// a shim includes no header, so it compiles for a target with no C library.
const FIXED_WIDTH: &str = "typedef __INT8_TYPE__ int8_t;
typedef __INT16_TYPE__ int16_t;
typedef __INT32_TYPE__ int32_t;
typedef __INT64_TYPE__ int64_t;
typedef __UINT8_TYPE__ uint8_t;
typedef __UINT16_TYPE__ uint16_t;
typedef __UINT32_TYPE__ uint32_t;
typedef __UINT64_TYPE__ uint64_t;
";

/// The C record each KIR type crossing by value is spelled as, declared once,
/// after the records it holds.
struct Records<'a> {
    module: &'a Module,
    names: HashMap<String, String>,
    text: String,
}

impl Records<'_> {
    fn declare(&mut self, name: &str) -> Result<String, String> {
        if let Some(spelled) = self.names.get(name) {
            return Ok(spelled.clone());
        }
        let declaration: &TypeDeclaration = self
            .module
            .types
            .iter()
            .find(|declaration| declaration.name.as_str() == name)
            .ok_or_else(|| format!("`%{name}` crosses the C seam but is not declared"))?;
        if declaration.layout != LayoutRule::C
            || !declaration.variants.is_empty()
            || !declaration.generics.is_empty()
        {
            return Err(format!(
                "`%{name}` crosses the C seam by value without C layout"
            ));
        }
        let union = declaration.kind == TypeKind::Explicit;
        if union
            && declaration
                .fields
                .iter()
                .any(|field| field.offset.unwrap_or(0) != 0)
        {
            return Err(format!(
                "`%{name}` places fields C cannot: only a union's all start at 0"
            ));
        }
        let mut fields = String::new();
        for (index, field) in declaration.fields.iter().enumerate() {
            let (spelled, extent) = self.field(&field.ty, name)?;
            let _ = writeln!(fields, "    {spelled} f{index}{extent};");
        }
        let keyword = if union { "union" } else { "struct" };
        let spelled = format!("{keyword} klf_t{}", self.names.len());
        let _ = writeln!(self.text, "{spelled} {{\n{fields}}};");
        self.names.insert(name.to_owned(), spelled.clone());
        Ok(spelled)
    }

    /// A member's C type and the `[n]` extents after its name.
    fn field(&mut self, ty: &Type, owner: &str) -> Result<(String, String), String> {
        Ok(match ty {
            Type::Integer(1 | 8) => ("uint8_t".to_owned(), String::new()),
            Type::Integer(bits @ (16 | 32 | 64)) => (format!("uint{bits}_t"), String::new()),
            Type::Float(32) => ("float".to_owned(), String::new()),
            Type::Float(64) => ("double".to_owned(), String::new()),
            Type::Pointer
            | Type::Address(_)
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Function(_) => ("void *".to_owned(), String::new()),
            Type::Nominal(name, arguments) if arguments.is_empty() => {
                (self.declare(name.as_str())?, String::new())
            }
            Type::Array(count, element) => {
                let (spelled, extent) = self.field(element, owner)?;
                (spelled, format!("[{count}]{extent}"))
            }
            other => return Err(format!("`%{owner}` holds `{other}`, which C cannot spell")),
        })
    }

    fn of(&self, ty: &CType) -> String {
        match ty {
            CType::Void => "void".to_owned(),
            CType::Bool => "_Bool".to_owned(),
            CType::Signed(bits) => format!("int{bits}_t"),
            CType::Unsigned(bits) => format!("uint{bits}_t"),
            CType::Float(32) => "float".to_owned(),
            CType::Float(_) => "double".to_owned(),
            CType::Pointer => "void *".to_owned(),
            CType::Value(name) => self.names[name.as_str()].clone(),
        }
    }

    /// How KIR carries `ty`: a by-value aggregate as its address.
    fn carried(&self, ty: &CType) -> String {
        match ty {
            CType::Value(_) => "void *".to_owned(),
            other => self.of(other),
        }
    }
}

/// A KIR function name as its source spells it, `read#Platform` as
/// `Platform.read`.
fn declared_name(name: &str) -> String {
    match name.split_once('#') {
        Some((function, module)) if module.starts_with(|c: char| c.is_ascii_uppercase()) => {
            format!("{module}.{function}")
        }
        _ => name.to_owned(),
    }
}

struct Shim<'a> {
    records: &'a Records<'a>,
    prototype: &'a CPrototype,
    index: usize,
    prefix: &'a str,
    name: &'a str,
    symbol: &'a str,
    /// The C name this shim calls: the declaration shared by every shape of a
    /// foreign symbol, or `klf_kir{index}` when C calls into the module.
    callee: String,
}

impl Shim<'_> {
    /// The KIR-shaped entry, `__klf_c_f`, calling the foreign `f` by value.
    fn call_foreign(&self, out: &mut String) {
        let records = self.records;
        let result = &self.prototype.result;
        let destination = matches!(result, CType::Value(_));
        let mut carried: Vec<String> = Vec::new();
        if destination {
            carried.push("void *out".to_owned());
        }
        let mut arguments = Vec::new();
        for (position, ty) in self.prototype.parameters.iter().enumerate() {
            carried.push(format!("{} a{position}", records.carried(ty)));
            arguments.push(match ty {
                CType::Value(_) => format!("*({} *)a{position}", records.of(ty)),
                _ => format!("a{position}"),
            });
        }
        let shim_result = if destination {
            "void".to_owned()
        } else {
            records.of(result)
        };
        let call = format!("{}({})", self.callee, arguments.join(", "));
        let body = match result {
            CType::Value(_) => format!("*({} *)out = {call};", records.of(result)),
            CType::Void => format!("{call};"),
            _ => format!("return {call};"),
        };
        let _ = writeln!(
            out,
            "{shim_result} klf_shim{}({}) __asm__(\"{}{}\");\n{shim_result} klf_shim{}({}) {{\n    {body}\n}}\n",
            self.index,
            list(&carried, "void"),
            self.prefix,
            c_shim_symbol(self.name),
            self.index,
            list(&carried, "void"),
        );
    }

    /// The KIR-shaped entry, `__klf_c_f`, making the system call `f` names:
    /// its number and up to six arguments in the registers the kernel entry
    /// reads, and its result from the register the kernel leaves it in.
    fn system_call(&self, out: &mut String, name: &str, target: &str) -> Result<(), String> {
        let arch = target.split('-').next().unwrap_or_default();
        // Tessera has Linux behind it, so its system calls are Linux's
        // (KLF-RIDL 5.8).
        let kernel = klf_target::is_tessera(target)
            || ["linux", "android"]
                .iter()
                .any(|system| target.contains(system));
        let refused = || {
            format!(
                "`@FFI.Syscall` `{name}` names no system call on `{target}`\n  = note: `{}` is \
                 declared as the Linux system call `{name}`, and `{target}` has no such call\n  = \
                 help: build for a Linux or Tessera target, or leave the declaration out of what \
                 this target builds",
                declared_name(self.name)
            )
        };
        if !kernel {
            return Err(refused());
        }
        let number = crate::linux_syscall(arch, name).ok_or_else(refused)?;
        let (number_register, registers, result_register, entry, clobbers) =
            syscall_convention(arch).ok_or_else(refused)?;
        let records = self.records;
        if self.prototype.parameters.len() > registers.len() {
            return Err(format!(
                "`@FFI.Syscall` `{name}` takes more arguments than a system call can"
            ));
        }
        let mut carried = Vec::new();
        let mut body = format!("    register long n __asm__(\"{number_register}\") = {number};\n");
        let mut held = vec![("n".to_owned(), number_register)];
        for (position, ty) in self.prototype.parameters.iter().enumerate() {
            carried.push(format!("{} a{position}", records.of(ty)));
            let _ = writeln!(
                body,
                "    register long r{position} __asm__(\"{}\") = (long)a{position};",
                registers[position]
            );
            held.push((format!("r{position}"), registers[position]));
        }
        let output = if let Some(at) = held
            .iter()
            .position(|(_, register)| *register == result_register)
        {
            format!("\"+r\"({})", held.remove(at).0)
        } else {
            let _ = writeln!(body, "    register long r __asm__(\"{result_register}\");");
            held.push(("r".to_owned(), result_register));
            "\"=r\"(r)".to_owned()
        };
        let result_variable = if result_register == number_register {
            "n"
        } else if self.prototype.parameters.is_empty() {
            "r"
        } else {
            "r0"
        };
        let inputs: Vec<String> = held
            .iter()
            .filter(|(variable, _)| variable != "r")
            .map(|(variable, _)| format!("\"r\"({variable})"))
            .collect();
        let _ = writeln!(
            body,
            "    __asm__ volatile(\"{entry}\" : {output} : {} : {clobbers});",
            inputs.join(", ")
        );
        let result = &self.prototype.result;
        match result {
            CType::Void => {}
            CType::Pointer => {
                let _ = writeln!(body, "    return (void *){result_variable};");
            }
            _ => {
                let _ = writeln!(
                    body,
                    "    return ({}){result_variable};",
                    records.of(result)
                );
            }
        }
        let spelled = records.of(result);
        let _ = writeln!(
            out,
            "{spelled} klf_shim{}({}) __asm__(\"{}{}\");\n{spelled} klf_shim{}({}) {{\n{body}}}\n",
            self.index,
            list(&carried, "void"),
            self.prefix,
            c_shim_symbol(self.name),
            self.index,
            list(&carried, "void"),
        );
        Ok(())
    }

    /// The C-shaped entry, `__klf_c_g`, calling the KIR function `g` with
    /// the addresses of what C passed by value.
    fn enter_from_c(&self, out: &mut String) {
        let records = self.records;
        let result = &self.prototype.result;
        let destination = matches!(result, CType::Value(_));
        let mut carried: Vec<String> = Vec::new();
        let mut arguments = Vec::new();
        if destination {
            carried.push("void *".to_owned());
            arguments.push("&out".to_owned());
        }
        let mut parameters = Vec::new();
        for (position, ty) in self.prototype.parameters.iter().enumerate() {
            carried.push(records.carried(ty));
            parameters.push(format!("{} a{position}", records.of(ty)));
            arguments.push(match ty {
                CType::Value(_) => format!("&a{position}"),
                _ => format!("a{position}"),
            });
        }
        let kir_result = if destination {
            "void".to_owned()
        } else {
            records.of(result)
        };
        let _ = writeln!(
            out,
            "{kir_result} klf_kir{}({}) __asm__(\"{}{}\");",
            self.index,
            list(&carried, "void"),
            self.prefix,
            self.symbol
        );
        let call = format!("{}({})", self.callee, arguments.join(", "));
        let body = match result {
            CType::Value(_) => format!("{} out;\n    {call};\n    return out;", records.of(result)),
            CType::Void => format!("{call};"),
            _ => format!("return {call};"),
        };
        let spelled = records.of(result);
        let _ = writeln!(
            out,
            "{spelled} klf_entry{}({}) __asm__(\"{}{}\");\n{spelled} klf_entry{}({}) {{\n    {body}\n}}\n",
            self.index,
            list(&parameters, "void"),
            self.prefix,
            c_shim_symbol(self.name),
            self.index,
            list(&parameters, "void"),
        );
    }
}

fn list(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        empty.to_owned()
    } else {
        items.join(", ")
    }
}

/// The registers and entry instruction of the Linux system-call convention on
/// `arch`: the number register, the six argument registers, the result
/// register, the trap instruction and the clobber list.
type SyscallConvention = (
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
    &'static str,
);

fn syscall_convention(arch: &str) -> Option<SyscallConvention> {
    match arch {
        "aarch64" => Some((
            "x8",
            &["x0", "x1", "x2", "x3", "x4", "x5"],
            "x0",
            "svc #0",
            "\"memory\"",
        )),
        "x86_64" => Some((
            "rax",
            &["rdi", "rsi", "rdx", "r10", "r8", "r9"],
            "rax",
            "syscall",
            "\"rcx\", \"r11\", \"memory\"",
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::c_shim_source;
    use crate::parse_module;

    fn shims(declaration: &str, target: &str) -> Result<Option<String>, String> {
        let text = format!(
            "module @Sys lang \"kira\" target \"{target}\" stage canonical\n\n{declaration}\n"
        );
        let module = parse_module(&text).map_err(|error| format!("{error:?}"))?;
        c_shim_source(&module, target, "", &|name| name.to_owned())
    }

    const WRITE: &str = "func private foreign @__klf_syscall_write(%fd: i64, %text: ptr, %count: i64) -> i64 engine(native) c(syscall write: s64, ptr, s64 -> s64)";

    #[test]
    fn a_system_call_is_numbered_for_the_architecture() {
        let aarch64 = shims(WRITE, "aarch64-unknown-linux-gnu").unwrap().unwrap();
        assert!(aarch64.contains("__asm__(\"x8\") = 64;"), "{aarch64}");
        assert!(aarch64.contains("\"svc #0\" : \"+r\"(r0)"), "{aarch64}");
        let x86_64 = shims(WRITE, "x86_64-unknown-linux-gnu").unwrap().unwrap();
        assert!(x86_64.contains("__asm__(\"rax\") = 1;"), "{x86_64}");
        assert!(x86_64.contains("\"syscall\" : \"+r\"(n)"), "{x86_64}");
        assert!(!x86_64.contains("#include"), "{x86_64}");
    }

    #[test]
    fn one_c_symbol_is_declared_once_however_many_shapes_reach_it() {
        // A variadic C function is reached through one shim per shape of
        // arguments, and each shim calls the same C symbol. Declaring that
        // symbol once per shape makes the C compiler treat the calls as
        // siblings of one function, and a sibling call drops what a variadic
        // call has to name: on amd64 `al` says how many arguments may be taken
        // from the register save area, so a `double` the shim handed over in
        // `xmm0` was read out of the overflow area instead, silently.
        let source = shims(
            "func private foreign @tally#0(%kinds: ptr) -> i64 engine(native) c(ptr, ... -> s64)\n\
             func private foreign @tally#1(%kinds: ptr, %a: f64, %b: f64) -> i64 engine(native) c(ptr, ..., f64, f64 -> s64)\n\
             func private foreign @tally#2(%kinds: ptr, %a: i64) -> i64 engine(native) c(ptr, ..., s64 -> s64)",
            "x86_64-unknown-linux-gnu",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            source.matches("__asm__(\"tally\")").count(),
            1,
            "the symbol is declared once: {source}"
        );
        assert!(
            source.contains("klf_foreign0(void *, ...) __asm__(\"tally\");"),
            "declared variadic, since one of its shapes is: {source}"
        );
        assert!(
            source.contains("klf_shim1(void * a0, double a1, double a2)"),
            "each shape keeps its own entry: {source}"
        );
        assert!(
            source.contains("klf_shim2(void * a0, int64_t a1)"),
            "and its own promoted arguments: {source}"
        );
        assert_eq!(
            source.matches("klf_foreign0(a0").count(),
            3,
            "every shape calls the one declaration: {source}"
        );
    }

    #[test]
    fn a_trampoline_is_cast_to_the_shape_of_each_call() {
        // `objc_msgSend` has no shape of its own: each caller treats it as the
        // function of its own arguments, and C reaches it through a cast.
        let source = shims(
            "func private foreign @objc_msgSend#0(%object: ptr, %selector: ptr) -> ptr engine(native) c(ptr, ptr -> ptr)\n\
             func private foreign @objc_msgSend#1(%object: ptr, %selector: ptr, %value: f64) -> unit engine(native) c(ptr, ptr, f64 -> void)",
            "aarch64-apple-darwin",
        )
        .unwrap()
        .unwrap();
        assert!(
            source.contains("void klf_foreign0(void) __asm__(\"objc_msgSend\");"),
            "declared once, with no shape: {source}"
        );
        assert!(
            source.contains("((void * (*)(void *, void *))klf_foreign0)(a0, a1)"),
            "the first shape casts to its own: {source}"
        );
        assert!(
            source.contains("((void (*)(void *, void *, double))klf_foreign0)(a0, a1, a2)"),
            "and so does the second: {source}"
        );
    }

    #[test]
    fn a_variadic_symbol_reached_by_two_fixed_shapes_is_refused() {
        let error = shims(
            "func private foreign @tally#0(%kinds: ptr) -> i64 engine(native) c(ptr, ... -> s64)\n\
             func private foreign @tally#1(%kinds: ptr, %a: i64) -> i64 engine(native) c(ptr, s64, ... -> s64)",
            "x86_64-unknown-linux-gnu",
        )
        .unwrap_err();
        assert!(error.contains("one C prototype cannot spell"), "{error}");
    }

    #[test]
    fn a_target_without_the_call_refuses_it() {
        let error = shims(WRITE, "aarch64-apple-darwin").unwrap_err();
        assert!(
            error.contains("names no system call on `aarch64-apple-darwin`"),
            "{error}"
        );
        let fork = "func private foreign @__klf_syscall_fork() -> i64 engine(native) c(syscall fork: -> s64)";
        assert!(shims(fork, "x86_64-unknown-linux-gnu").is_ok());
        assert!(
            shims(fork, "aarch64-unknown-linux-gnu")
                .unwrap_err()
                .contains("`fork`")
        );
    }
}
