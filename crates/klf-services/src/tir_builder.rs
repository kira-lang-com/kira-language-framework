use std::fmt::Write as _;

use tir_format::{ModuleHeader, TextModule, parse_text_module};

#[derive(Debug)]
pub enum TirModuleBuildError<E> {
    Parse(tir_format::TextParseError),
    Validation(E),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TirFunctionParameter {
    pub name: String,
    pub ty: String,
    pub context: bool,
    pub convention: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TirFunctionSignature {
    pub linkage: String,
    pub path: String,
    pub generic_parameters: Vec<String>,
    pub parameters: Vec<TirFunctionParameter>,
    pub result_type: String,
    pub failure_type: Option<String>,
    pub attributes: Vec<String>,
}

impl TirFunctionSignature {
    #[must_use]
    pub fn render(&self) -> String {
        let generics = angle_list(
            &self
                .generic_parameters
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let parameters = self
            .parameters
            .iter()
            .map(|parameter| {
                let context = if parameter.context { "context " } else { "" };
                let convention = parameter
                    .convention
                    .as_deref()
                    .map_or_else(String::new, |convention| format!("{convention} "));
                format!(
                    "{}: {context}{convention}{}",
                    value_reference(&parameter.name),
                    parameter.ty
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let failure = self
            .failure_type
            .as_deref()
            .map_or_else(String::new, |ty| format!(" fails {ty}"));
        let attributes = if self.attributes.is_empty() {
            String::new()
        } else {
            format!(" {}", self.attributes.join(" "))
        };
        format!(
            "func {} {}{}({parameters}) -> {}{failure}{attributes}",
            self.linkage,
            symbol_reference(&self.path),
            generics,
            self.result_type
        )
    }
}

/// Builds a TIR text module directly from its header and TIR item text.
///
/// This is the module-level producer seam for KLF frontends. Item text is
/// parsed as TIR when `finish` is called, so malformed items are rejected
/// before they reach verification or the TIR compiler.
#[derive(Clone, Debug)]
pub struct TirModuleBuilder {
    header: ModuleHeader,
    items: Vec<String>,
}

impl TirModuleBuilder {
    #[must_use]
    pub fn new(header: ModuleHeader) -> Self {
        Self {
            header,
            items: Vec::new(),
        }
    }

    /// Adds one complete TIR top-level item. The item may span multiple lines.
    pub fn add_item(&mut self, source: impl Into<String>) {
        self.items.push(source.into());
    }

    /// Adds a function assembled by [`TirFunctionBuilder`].
    pub fn add_function(&mut self, function: TirFunctionBuilder) {
        self.add_item(function.finish());
    }

    /// Parses the assembled text using the normative TIR text parser.
    pub fn finish(self) -> Result<TextModule, tir_format::TextParseError> {
        let mut source = render_header(&self.header);
        for item in self.items {
            source.push_str(item.trim());
            source.push('\n');
        }
        parse_text_module(&source)
    }

    /// Parses the assembled module, then applies the caller's semantic checks.
    pub fn finish_with<E>(
        self,
        validate: impl FnOnce(&TextModule) -> Result<(), E>,
    ) -> Result<TextModule, TirModuleBuildError<E>> {
        let module = self.finish().map_err(TirModuleBuildError::Parse)?;
        validate(&module).map_err(TirModuleBuildError::Validation)?;
        Ok(module)
    }

    /// Parses and semantically verifies the assembled module with default control-flow options.
    pub fn finish_verified(
        self,
    ) -> Result<TextModule, TirModuleBuildError<Vec<tir_diag::Diagnostic>>> {
        self.finish_verified_with_options(tir_verify::ControlFlowOptions::default())
    }

    /// Parses and semantically verifies the assembled module.
    pub fn finish_verified_with_options(
        self,
        options: tir_verify::ControlFlowOptions,
    ) -> Result<TextModule, TirModuleBuildError<Vec<tir_diag::Diagnostic>>> {
        self.finish_with(|module| {
            let diagnostics = tir_verify::verify_text_module(module, options);
            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == tir_diag::Severity::Error)
            {
                Err(diagnostics)
            } else {
                Ok(())
            }
        })
    }
}

/// Assembles a TIR function item from its declaration header and blocks.
/// Instruction text is emitted as TIR and checked when the module is parsed.
#[derive(Clone, Debug)]
pub struct TirFunctionBuilder {
    header: String,
    blocks: Vec<TirBlockBuilder>,
    declaration: bool,
}

impl TirFunctionBuilder {
    /// Starts a function definition. `header` is the TIR text before `{`.
    #[must_use]
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            blocks: Vec::new(),
            declaration: false,
        }
    }

    /// Starts a body-less function declaration.
    #[must_use]
    pub fn declaration(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            blocks: Vec::new(),
            declaration: true,
        }
    }

    #[must_use]
    pub fn from_signature(signature: &TirFunctionSignature) -> Self {
        Self::new(signature.render())
    }

    #[must_use]
    pub fn declaration_from_signature(signature: &TirFunctionSignature) -> Self {
        Self::declaration(signature.render())
    }

    pub fn add_block(&mut self, block: TirBlockBuilder) {
        self.blocks.push(block);
    }

    #[must_use]
    pub fn finish(self) -> String {
        if self.declaration {
            return self.header.trim().to_owned();
        }
        let mut output = String::new();
        output.push_str(self.header.trim());
        output.push_str(" {\n");
        for block in self.blocks {
            output.push_str(&block.finish());
        }
        output.push('}');
        output
    }
}

macro_rules! unary_instruction_methods {
    ($($method:ident => $opcode:literal),+ $(,)?) => {
        $(
            #[must_use]
            pub fn $method(
                &mut self,
                result: &str,
                modifiers: Option<&str>,
                value: &str,
            ) -> String {
                self.integer_unary(result, $opcode, modifiers, value)
            }
        )+
    };
}

macro_rules! binary_instruction_methods {
    ($($method:ident => $opcode:literal),+ $(,)?) => {
        $(
            #[must_use]
            pub fn $method(
                &mut self,
                result: &str,
                modifiers: Option<&str>,
                lhs: &str,
                rhs: &str,
            ) -> String {
                self.integer_binary(result, $opcode, modifiers, lhs, rhs)
            }
        )+
    };
}

macro_rules! ternary_instruction_methods {
    ($($method:ident => $opcode:literal),+ $(,)?) => {
        $(
            #[must_use]
            pub fn $method(
                &mut self,
                result: &str,
                modifiers: Option<&str>,
                first: &str,
                second: &str,
                third: &str,
            ) -> String {
                let result = value_reference(result);
                let modifiers = modifier_group(modifiers);
                self.add_instruction(format!(
                    "{result} = {}{modifiers} {}, {}, {}",
                    $opcode,
                    value_reference(first),
                    value_reference(second),
                    value_reference(third)
                ));
                result
            }
        )+
    };
}

/// Assembles one labeled TIR block and its instruction lines.
#[derive(Clone, Debug)]
pub struct TirBlockBuilder {
    header: String,
    instructions: Vec<String>,
}

impl TirBlockBuilder {
    /// Starts a block from its TIR label line, such as `bb0(%x: i64):`.
    #[must_use]
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            instructions: Vec::new(),
        }
    }

    /// Adds one TIR instruction line. The module parser and verifier check it.
    pub fn add_instruction(&mut self, instruction: impl Into<String>) {
        self.instructions.push(instruction.into());
    }

    /// Emits a spec-defined instruction whose operands are not covered by a
    /// dedicated helper. Each modifier is written as its own bracketed group;
    /// operand strings are preserved as TIR text and separated with commas.
    pub fn emit_instruction(
        &mut self,
        results: &[&str],
        opcode: &str,
        modifiers: &[&str],
        operands: &[&str],
    ) -> Vec<String> {
        let results = results
            .iter()
            .map(|result| value_reference(result))
            .collect::<Vec<_>>();
        let result_prefix = if results.is_empty() {
            String::new()
        } else {
            format!("{} = ", results.join(", "))
        };
        let modifiers = modifier_groups(modifiers);
        let operands = if operands.is_empty() {
            String::new()
        } else {
            format!(" {}", operands.join(", "))
        };
        self.add_instruction(format!("{result_prefix}{opcode}{modifiers}{operands}"));
        results
    }

    pub fn emit_effect(&mut self, opcode: &str, modifiers: &[&str], operands: &[&str]) {
        self.emit_instruction(&[], opcode, modifiers, operands);
    }

    #[must_use]
    pub fn const_null(&mut self, result: &str, address_space: Option<&str>) -> String {
        let result = value_reference(result);
        let address_space = address_space.map_or_else(String::new, |space| format!(" {space}"));
        self.add_instruction(format!("{result} = const.null{address_space}"));
        result
    }

    #[must_use]
    pub fn const_poison(&mut self, result: &str, ty: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = const.poison {ty}"));
        result
    }

    #[must_use]
    pub fn const_int(&mut self, result: &str, ty: &str, value: i128) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = const.int {ty}, {value}"));
        result
    }

    #[must_use]
    pub fn const_float(&mut self, result: &str, ty: &str, literal: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = const.float {ty}, {literal}"));
        result
    }

    #[must_use]
    pub fn const_bool(&mut self, result: &str, value: bool) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = const.bool {value}"));
        result
    }

    #[must_use]
    pub fn const_zero(&mut self, result: &str, ty: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = const.zero {ty}"));
        result
    }

    #[must_use]
    pub fn const_string(&mut self, address: &str, length: &str, value: &str) -> (String, String) {
        let address = value_reference(address);
        let length = value_reference(length);
        let mut literal = String::new();
        push_string(&mut literal, value);
        self.add_instruction(format!("{address}, {length} = const.string {literal}"));
        (address, length)
    }

    /// Emits arbitrary bytes using the normative `const.bytes` `b"..."` form.
    #[must_use]
    pub fn const_bytes(&mut self, result: &str, ty: &str, bytes: &[u8]) -> String {
        let result = value_reference(result);
        let mut literal = String::from("b\"");
        for byte in bytes {
            match *byte {
                b'\\' => literal.push_str("\\\\"),
                b'"' => literal.push_str("\\\""),
                b'\n' => literal.push_str("\\n"),
                b'\r' => literal.push_str("\\r"),
                b'\t' => literal.push_str("\\t"),
                0 => literal.push_str("\\0"),
                0x20..=0x7e => literal.push(char::from(*byte)),
                value => {
                    let _ = write!(literal, "\\x{value:02X}");
                }
            }
        }
        literal.push('"');
        self.add_instruction(format!("{result} = const.bytes {ty}, {literal}"));
        result
    }

    #[must_use]
    pub fn const_vec(&mut self, result: &str, ty: &str, elements: &[&str]) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = const.vec {ty}, [{}]",
            elements.join(", ")
        ));
        result
    }

    #[must_use]
    pub fn const_aggregate(&mut self, result: &str, ty: &str, elements: &[&str]) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = const.aggregate {ty}({})",
            elements.join(", ")
        ));
        result
    }

    #[must_use]
    pub fn const_sampler(&mut self, result: &str, fields: &[(&str, &str)]) -> String {
        let result = value_reference(result);
        let fields = fields
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join(", ");
        self.add_instruction(format!("{result} = const.sampler {{{fields}}}"));
        result
    }

    #[must_use]
    pub fn func_ref(&mut self, result: &str, function: &str, generic_arguments: &[&str]) -> String {
        let result = value_reference(result);
        let function = symbol_reference(function);
        let arguments = if generic_arguments.is_empty() {
            String::new()
        } else {
            format!("<{}>", generic_arguments.join(", "))
        };
        self.add_instruction(format!("{result} = func.ref {function}{arguments}"));
        result
    }

    #[must_use]
    pub fn global_addr(&mut self, result: &str, global: &str) -> String {
        self.symbol_result(result, "global.addr", global)
    }

    #[must_use]
    pub fn tls_addr(&mut self, result: &str, global: &str) -> String {
        self.symbol_result(result, "tls.addr", global)
    }

    #[must_use]
    pub fn descriptor_ref(&mut self, result: &str, ty: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = descriptor.ref {ty}"));
        result
    }

    #[must_use]
    pub fn witness_ref(&mut self, result: &str, ty: &str, trait_name: &str) -> String {
        let result = value_reference(result);
        let trait_name = symbol_reference(trait_name);
        self.add_instruction(format!("{result} = witness.ref {ty}, {trait_name}"));
        result
    }

    #[must_use]
    pub fn block_addr(&mut self, result: &str, block: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = block.addr {}", block_reference(block)));
        result
    }

    #[must_use]
    pub fn gpu_entry_ref(&mut self, result: &str, entry: &str) -> String {
        self.symbol_result(result, "gpu.entry.ref", entry)
    }

    #[must_use]
    pub fn struct_new(&mut self, result: &str, ty: &str, fields: &[&str]) -> String {
        self.construct_aggregate(result, "struct.new", ty, fields)
    }

    #[must_use]
    pub fn struct_extract(&mut self, result: &str, value: &str, field: u32) -> String {
        self.extract_aggregate(result, "struct.extract", value, field.to_string())
    }

    #[must_use]
    pub fn struct_insert(
        &mut self,
        result: &str,
        value: &str,
        field: u32,
        replacement: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = struct.insert {}, {field}, {}",
            value_reference(value),
            value_reference(replacement)
        ));
        result
    }

    #[must_use]
    pub fn struct_destructure(&mut self, results: &[&str], value: &str) -> Vec<String> {
        self.multi_result_instruction(results, "struct.destructure", &[value_reference(value)])
    }

    #[must_use]
    pub fn tuple_new(&mut self, result: &str, fields: &[&str]) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = tuple.new ({})", value_list(fields)));
        result
    }

    #[must_use]
    pub fn tuple_extract(&mut self, result: &str, value: &str, field: u32) -> String {
        self.extract_aggregate(result, "tuple.extract", value, field.to_string())
    }

    #[must_use]
    pub fn tuple_destructure(&mut self, results: &[&str], value: &str) -> Vec<String> {
        self.multi_result_instruction(results, "tuple.destructure", &[value_reference(value)])
    }

    #[must_use]
    pub fn array_new(&mut self, result: &str, array_type: &str, elements: &[&str]) -> String {
        self.construct_aggregate(result, "array.new", array_type, elements)
    }

    #[must_use]
    pub fn array_extract(&mut self, result: &str, value: &str, index: &str) -> String {
        self.binary_value_instruction(result, "array.extract", value, index)
    }

    #[must_use]
    pub fn array_insert(
        &mut self,
        result: &str,
        value: &str,
        index: &str,
        element: &str,
    ) -> String {
        self.ternary_value_instruction(result, "array.insert", value, index, element)
    }

    #[must_use]
    pub fn enum_new(
        &mut self,
        result: &str,
        enum_type: &str,
        variant: &str,
        payload: &[&str],
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = enum.new {enum_type}, {variant}({})",
            value_list(payload)
        ));
        result
    }

    #[must_use]
    pub fn enum_tag(&mut self, result: &str, value: &str) -> String {
        self.unary_value_instruction(result, "enum.tag", value)
    }

    #[must_use]
    pub fn enum_take(&mut self, result: &str, value: &str, variant: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = enum.take {}, {variant}",
            value_reference(value)
        ));
        result
    }

    #[must_use]
    pub fn enum_project(&mut self, result: &str, value: &str, variant: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = enum.project {}, {variant}",
            value_reference(value)
        ));
        result
    }

    #[must_use]
    pub fn stack_alloc(
        &mut self,
        result: &str,
        ty: &str,
        address_space: Option<&str>,
        count: Option<&str>,
    ) -> String {
        let result = value_reference(result);
        let address_space = address_space.map_or_else(String::new, |space| format!(" <{space}>"));
        let count = count.map_or_else(String::new, |count| {
            format!(" [count {}]", value_reference(count))
        });
        self.add_instruction(format!("{result} = stack.alloc{count} {ty}{address_space}"));
        result
    }

    pub fn stack_dealloc(&mut self, address: &str) {
        self.add_instruction(format!("stack.dealloc {}", value_reference(address)));
    }

    #[must_use]
    pub fn field_addr(&mut self, result: &str, address: &str, field: u32) -> String {
        self.indexed_address(result, "field.addr", address, field.to_string(), None)
    }

    #[must_use]
    pub fn elem_addr(
        &mut self,
        result: &str,
        address: &str,
        index: &str,
        unchecked: bool,
    ) -> String {
        self.indexed_address(
            result,
            "elem.addr",
            address,
            value_reference(index),
            unchecked.then_some("unchecked"),
        )
    }

    #[must_use]
    pub fn enum_payload_addr(&mut self, result: &str, address: &str, variant: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = enum.payload.addr {}, {variant}",
            value_reference(address)
        ));
        result
    }

    pub fn enum_inject(&mut self, address: &str, variant: &str) {
        self.add_instruction(format!(
            "enum.inject {}, {variant}",
            value_reference(address)
        ));
    }

    #[must_use]
    pub fn addr_cast(&mut self, result: &str, address: &str, target_type: &str) -> String {
        self.cast_address(result, "addr.cast", address, "to", target_type)
    }

    #[must_use]
    pub fn addr_to_ptr(&mut self, result: &str, address: &str) -> String {
        self.unary_value_instruction(result, "addr.toptr", address)
    }

    #[must_use]
    pub fn ptr_to_addr(&mut self, result: &str, pointer: &str, target_type: &str) -> String {
        self.cast_address(result, "ptr.toaddr", pointer, "as", target_type)
    }

    #[must_use]
    pub fn ptr_offset(
        &mut self,
        result: &str,
        pointer: &str,
        bytes: &str,
        inbounds: bool,
    ) -> String {
        self.integer_binary(
            result,
            "ptr.offset",
            inbounds.then_some("inbounds"),
            pointer,
            bytes,
        )
    }

    #[must_use]
    pub fn ptr_diff(&mut self, result: &str, left: &str, right: &str) -> String {
        self.binary_value_instruction(result, "ptr.diff", left, right)
    }

    #[must_use]
    pub fn load(&mut self, result: &str, mode: Option<&str>, address: &str) -> String {
        self.address_operation(result, "load", mode, address)
    }

    #[must_use]
    pub fn load_borrow(&mut self, result: &str, address: &str) -> String {
        self.unary_value_instruction(result, "load.borrow", address)
    }

    pub fn store(&mut self, mode: Option<&str>, value: &str, address: &str) {
        let mode = modifier_group(mode);
        self.add_instruction(format!(
            "store{mode} {} to {}",
            value_reference(value),
            value_reference(address)
        ));
    }

    pub fn copy_addr(&mut self, source: &str, destination: &str, mode: Option<&str>) {
        let mode = modifier_group(mode);
        self.add_instruction(format!(
            "copy.addr {} to{mode} {}",
            value_reference(source),
            value_reference(destination)
        ));
    }

    pub fn move_addr(&mut self, source: &str, destination: &str, mode: Option<&str>) {
        let mode = modifier_group(mode);
        self.add_instruction(format!(
            "move.addr {} to{mode} {}",
            value_reference(source),
            value_reference(destination)
        ));
    }

    pub fn destroy_addr(&mut self, address: &str) {
        self.add_instruction(format!("destroy.addr {}", value_reference(address)));
    }

    #[must_use]
    pub fn raw_load(
        &mut self,
        result: &str,
        ty: &str,
        pointer: &str,
        alignment: Option<u32>,
        volatile: bool,
        nontemporal: bool,
    ) -> String {
        let result = value_reference(result);
        let mut suffix = String::new();
        if let Some(alignment) = alignment {
            let _ = write!(suffix, " [align {alignment}]");
        }
        if volatile {
            suffix.push_str(" [volatile]");
        }
        if nontemporal {
            suffix.push_str(" [nontemporal]");
        }
        self.add_instruction(format!(
            "{result} = raw.load {ty}, {}{suffix}",
            value_reference(pointer)
        ));
        result
    }

    pub fn raw_store(
        &mut self,
        value: &str,
        pointer: &str,
        alignment: Option<u32>,
        volatile: bool,
        nontemporal: bool,
    ) {
        let mut suffix = String::new();
        if let Some(alignment) = alignment {
            let _ = write!(suffix, " [align {alignment}]");
        }
        if volatile {
            suffix.push_str(" [volatile]");
        }
        if nontemporal {
            suffix.push_str(" [nontemporal]");
        }
        self.add_instruction(format!(
            "raw.store {}, {}{suffix}",
            value_reference(value),
            value_reference(pointer)
        ));
    }

    pub fn mem_copy(
        &mut self,
        destination: &str,
        source: &str,
        bytes: &str,
        volatile: bool,
        no_overlap: bool,
    ) {
        let mut suffix = String::new();
        if volatile {
            suffix.push_str(" [volatile]");
        }
        if no_overlap {
            suffix.push_str(" [nooverlap]");
        }
        self.add_instruction(format!(
            "mem.copy {}, {}, {}{suffix}",
            value_reference(destination),
            value_reference(source),
            value_reference(bytes)
        ));
    }

    pub fn mem_move(&mut self, destination: &str, source: &str, bytes: &str, volatile: bool) {
        let suffix = if volatile { " [volatile]" } else { "" };
        self.add_instruction(format!(
            "mem.move {}, {}, {}{suffix}",
            value_reference(destination),
            value_reference(source),
            value_reference(bytes)
        ));
    }

    pub fn mem_set(&mut self, destination: &str, byte: &str, bytes: &str, volatile: bool) {
        let suffix = if volatile { " [volatile]" } else { "" };
        self.add_instruction(format!(
            "mem.set {}, {}, {}{suffix}",
            value_reference(destination),
            value_reference(byte),
            value_reference(bytes)
        ));
    }

    pub fn prefetch(
        &mut self,
        pointer: &str,
        access: Option<&str>,
        locality: Option<u8>,
        kind: Option<&str>,
    ) {
        let mut suffix = String::new();
        if let Some(access) = access {
            let _ = write!(suffix, " [{access}]");
        }
        if let Some(locality) = locality {
            let _ = write!(suffix, " [locality {locality}]");
        }
        if let Some(kind) = kind {
            let _ = write!(suffix, " [{kind}]");
        }
        self.add_instruction(format!("prefetch {}{suffix}", value_reference(pointer)));
    }

    #[must_use]
    pub fn vec_splat(&mut self, result: &str, scalar: &str, vector_type: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = vec.splat {} to {vector_type}",
            value_reference(scalar)
        ));
        result
    }

    #[must_use]
    pub fn vec_extract(&mut self, result: &str, vector: &str, index: &str) -> String {
        self.binary_value_instruction(result, "vec.extract", vector, index)
    }

    #[must_use]
    pub fn vec_insert(&mut self, result: &str, vector: &str, index: &str, element: &str) -> String {
        self.ternary_value_instruction(result, "vec.insert", vector, index, element)
    }

    #[must_use]
    pub fn vec_shuffle(
        &mut self,
        result: &str,
        first: &str,
        second: &str,
        indices: &[u32],
    ) -> String {
        let result = value_reference(result);
        let indices = indices
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        self.add_instruction(format!(
            "{result} = vec.shuffle {}, {}, [{indices}]",
            value_reference(first),
            value_reference(second)
        ));
        result
    }

    #[must_use]
    pub fn vec_step(&mut self, result: &str, vector_type: &str) -> String {
        self.typed_result(result, "vec.step", vector_type)
    }

    #[must_use]
    pub fn vec_vscale(&mut self, result: &str) -> String {
        self.unit_result(result, "vec.vscale")
    }

    #[must_use]
    pub fn vec_reduce(
        &mut self,
        result: &str,
        operator: &str,
        vector: &str,
        mask: Option<&str>,
        reassociate: bool,
    ) -> String {
        let result = value_reference(result);
        let reassoc = if reassociate { " [reassoc]" } else { "" };
        let mask = mask.map_or_else(String::new, |mask| {
            format!(" [mask {}]", value_reference(mask))
        });
        self.add_instruction(format!(
            "{result} = vec.reduce{reassoc} {operator} {}{mask}",
            value_reference(vector)
        ));
        result
    }

    #[must_use]
    pub fn vec_load(
        &mut self,
        result: &str,
        vector_type: &str,
        pointer: &str,
        alignment: Option<u32>,
        mask_passthru: Option<(&str, &str)>,
    ) -> String {
        let result = value_reference(result);
        let mut suffix = String::new();
        if let Some(alignment) = alignment {
            let _ = write!(suffix, " [align {alignment}]");
        }
        if let Some((mask, passthru)) = mask_passthru {
            let _ = write!(
                suffix,
                " [mask {}, passthru {}]",
                value_reference(mask),
                value_reference(passthru)
            );
        }
        self.add_instruction(format!(
            "{result} = vec.load {vector_type}, {}{suffix}",
            value_reference(pointer)
        ));
        result
    }

    pub fn vec_store(
        &mut self,
        vector: &str,
        pointer: &str,
        alignment: Option<u32>,
        mask: Option<&str>,
    ) {
        let mut suffix = String::new();
        if let Some(alignment) = alignment {
            let _ = write!(suffix, " [align {alignment}]");
        }
        if let Some(mask) = mask {
            let _ = write!(suffix, " [mask {}]", value_reference(mask));
        }
        self.add_instruction(format!(
            "vec.store {}, {}{suffix}",
            value_reference(vector),
            value_reference(pointer)
        ));
    }

    #[must_use]
    pub fn vec_gather(
        &mut self,
        result: &str,
        vector_type: &str,
        address_operands: &[&str],
        mask_passthru: Option<(&str, &str)>,
    ) -> String {
        let result = value_reference(result);
        let mut suffix = String::new();
        if let Some((mask, passthru)) = mask_passthru {
            let _ = write!(
                suffix,
                " [mask {}, passthru {}]",
                value_reference(mask),
                value_reference(passthru)
            );
        }
        self.add_instruction(format!(
            "{result} = vec.gather {vector_type}, {}{suffix}",
            value_list(address_operands)
        ));
        result
    }

    pub fn vec_scatter(&mut self, vector: &str, address_operands: &[&str], mask: Option<&str>) {
        let mask = mask.map_or_else(String::new, |mask| {
            format!(" [mask {}]", value_reference(mask))
        });
        self.add_instruction(format!(
            "vec.scatter {}, {}{mask}",
            value_reference(vector),
            value_list(address_operands)
        ));
    }

    #[must_use]
    pub fn vec_compress(
        &mut self,
        result: &str,
        vector: &str,
        mask: &str,
        passthru: &str,
    ) -> String {
        self.ternary_value_instruction(result, "vec.compress", vector, mask, passthru)
    }

    #[must_use]
    pub fn vec_expand(&mut self, result: &str, vector: &str, mask: &str, passthru: &str) -> String {
        self.ternary_value_instruction(result, "vec.expand", vector, mask, passthru)
    }

    #[must_use]
    pub fn vec_dot(
        &mut self,
        result: &str,
        kind: &str,
        accumulator: &str,
        a: &str,
        b: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = vec.dot {kind} {}, {}, {}",
            value_reference(accumulator),
            value_reference(a),
            value_reference(b)
        ));
        result
    }

    #[must_use]
    pub fn vec_zip(&mut self, result: &str, a: &str, b: &str, half: Option<&str>) -> String {
        let result = value_reference(result);
        let half = half.map_or_else(String::new, |half| format!(" {half}"));
        self.add_instruction(format!(
            "{result} = vec.zip {}, {}{half}",
            value_reference(a),
            value_reference(b)
        ));
        result
    }

    #[must_use]
    pub fn vec_unzip(&mut self, result: &str, a: &str, b: &str, parity: Option<&str>) -> String {
        let result = value_reference(result);
        let parity = parity.map_or_else(String::new, |parity| format!(" {parity}"));
        self.add_instruction(format!(
            "{result} = vec.unzip {}, {}{parity}",
            value_reference(a),
            value_reference(b)
        ));
        result
    }

    #[must_use]
    pub fn vec_reverse(&mut self, result: &str, vector: &str) -> String {
        self.unary_value_instruction(result, "vec.reverse", vector)
    }

    #[must_use]
    pub fn vec_slide(
        &mut self,
        result: &str,
        a: &str,
        b: &str,
        amount: &str,
        direction: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = vec.slide {}, {}, {} {direction}",
            value_reference(a),
            value_reference(b),
            value_reference(amount)
        ));
        result
    }

    #[must_use]
    pub fn vec_concat(&mut self, result: &str, a: &str, b: &str) -> String {
        self.binary_value_instruction(result, "vec.concat", a, b)
    }

    #[must_use]
    pub fn vec_split(&mut self, result: &str, vector: &str, half: Option<&str>) -> String {
        let result = value_reference(result);
        let half = half.map_or_else(String::new, |half| format!(" {half}"));
        self.add_instruction(format!(
            "{result} = vec.split {}{half}",
            value_reference(vector)
        ));
        result
    }

    #[must_use]
    pub fn vec_mask_active(&mut self, result: &str, first_lane: &str, count: &str) -> String {
        self.binary_value_instruction(result, "vec.mask.active", first_lane, count)
    }

    #[must_use]
    pub fn vec_mask_count(&mut self, result: &str, mask: &str) -> String {
        self.unary_value_instruction(result, "vec.mask.count", mask)
    }

    #[must_use]
    pub fn vec_mask_first(&mut self, result: &str, mask: &str) -> String {
        self.unary_value_instruction(result, "vec.mask.first", mask)
    }

    #[must_use]
    pub fn vec_table(&mut self, result: &str, table: &str, indices: &str) -> String {
        self.binary_value_instruction(result, "vec.table", table, indices)
    }

    #[must_use]
    pub fn heap_alloc_counted(&mut self, result: &str, ty: &str) -> String {
        self.typed_result(result, "heap.alloc.counted", ty)
    }

    #[must_use]
    pub fn heap_alloc_unique(&mut self, result: &str, ty: &str) -> String {
        self.typed_result(result, "heap.alloc.unique", ty)
    }

    #[must_use]
    pub fn ref_addr(&mut self, result: &str, reference: &str) -> String {
        self.unary_value_instruction(result, "ref.addr", reference)
    }

    #[must_use]
    pub fn uniq_addr(&mut self, result: &str, unique: &str) -> String {
        self.unary_value_instruction(result, "uniq.addr", unique)
    }

    #[must_use]
    pub fn heap_alloc_raw(
        &mut self,
        result: &str,
        size: &str,
        align: &str,
        uninit: bool,
    ) -> String {
        let result = value_reference(result);
        let suffix = if uninit { " [uninit]" } else { "" };
        self.add_instruction(format!(
            "{result} = heap.alloc.raw {}, {}{suffix}",
            value_reference(size),
            value_reference(align)
        ));
        result
    }

    #[must_use]
    pub fn heap_realloc_raw(
        &mut self,
        result: &str,
        pointer: &str,
        size: &str,
        align: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = heap.realloc.raw {}, {}, {}",
            value_reference(pointer),
            value_reference(size),
            value_reference(align)
        ));
        result
    }

    pub fn heap_free_raw(&mut self, pointer: &str) {
        self.add_instruction(format!("heap.free.raw {}", value_reference(pointer)));
    }

    #[must_use]
    pub fn weak_new(&mut self, result: &str, reference: &str) -> String {
        self.unary_value_instruction(result, "weak.new", reference)
    }

    #[must_use]
    pub fn weak_load(&mut self, result: &str, weak: &str) -> String {
        self.unary_value_instruction(result, "weak.load", weak)
    }

    #[must_use]
    pub fn ref_unowned(&mut self, result: &str, reference: &str) -> String {
        self.unary_value_instruction(result, "ref.unowned", reference)
    }

    #[must_use]
    pub fn ref_is_unique(&mut self, result: &str, reference: &str) -> String {
        self.unary_value_instruction(result, "ref.isunique", reference)
    }

    #[must_use]
    pub fn copy_value(&mut self, result: &str, value: &str) -> String {
        self.unary_value_instruction(result, "copy", value)
    }

    #[must_use]
    pub fn move_value(&mut self, result: &str, value: &str) -> String {
        self.unary_value_instruction(result, "move", value)
    }

    pub fn destroy_value(&mut self, value: &str) {
        self.add_instruction(format!("destroy {}", value_reference(value)));
    }

    #[must_use]
    pub fn borrow_begin(&mut self, result: &str, value: &str) -> String {
        self.unary_value_instruction(result, "borrow.begin", value)
    }

    pub fn borrow_end(&mut self, borrow: &str) {
        self.add_instruction(format!("borrow.end {}", value_reference(borrow)));
    }

    #[must_use]
    pub fn into_raw(&mut self, result: &str, value: &str) -> String {
        self.unary_value_instruction(result, "into.raw", value)
    }

    #[must_use]
    pub fn from_raw(&mut self, result: &str, ty: &str, pointer: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = from.raw {ty}, {}",
            value_reference(pointer)
        ));
        result
    }

    #[must_use]
    pub fn access_begin(
        &mut self,
        result: &str,
        access: &str,
        enforcement: &str,
        address: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = access.begin [{access}] [{enforcement}] {}",
            value_reference(address)
        ));
        result
    }

    pub fn access_end(&mut self, access: &str) {
        self.add_instruction(format!("access.end {}", value_reference(access)));
    }

    #[must_use]
    pub fn atomic_load(
        &mut self,
        result: &str,
        ty: &str,
        pointer: &str,
        order: Option<&str>,
        scope: Option<&str>,
    ) -> String {
        let groups = atomic_order_scope_groups(order, scope);
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = atomic.load{} {ty}, {}",
            owned_modifier_groups(&groups),
            value_reference(pointer)
        ));
        result
    }

    pub fn atomic_store(
        &mut self,
        value: &str,
        pointer: &str,
        order: Option<&str>,
        scope: Option<&str>,
    ) {
        let groups = atomic_order_scope_groups(order, scope);
        self.add_instruction(format!(
            "atomic.store{} {}, {}",
            owned_modifier_groups(&groups),
            value_reference(value),
            value_reference(pointer)
        ));
    }

    #[must_use]
    pub fn atomic_rmw(
        &mut self,
        result: &str,
        operation: &str,
        pointer: &str,
        value: &str,
        order: Option<&str>,
        scope: Option<&str>,
    ) -> String {
        let groups = atomic_order_scope_groups(order, scope);
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = atomic.rmw {operation}{} {}, {}",
            owned_modifier_groups(&groups),
            value_reference(pointer),
            value_reference(value)
        ));
        result
    }

    #[must_use]
    pub fn atomic_cmpxchg(
        &mut self,
        old_value: &str,
        exchanged: &str,
        pointer: &str,
        expected: &str,
        new_value: &str,
        weak: bool,
        success_order: Option<&str>,
        failure_order: Option<&str>,
        scope: Option<&str>,
    ) -> (String, String) {
        let mut groups = Vec::new();
        if weak {
            groups.push("weak".to_owned());
        }
        if let Some(order) = success_order {
            groups.push(format!("success_order={order}"));
        }
        if let Some(order) = failure_order {
            groups.push(format!("failure_order={order}"));
        }
        if let Some(scope) = scope {
            groups.push(format!("scope={scope}"));
        }
        let groups = groups.iter().map(String::as_str).collect::<Vec<_>>();
        let old_value = value_reference(old_value);
        let exchanged = value_reference(exchanged);
        self.add_instruction(format!(
            "{old_value}, {exchanged} = atomic.cmpxchg{} {}, {}, {}",
            modifier_groups(&groups),
            value_reference(pointer),
            value_reference(expected),
            value_reference(new_value)
        ));
        (old_value, exchanged)
    }

    pub fn fence(&mut self, order: Option<&str>, scope: Option<&str>) {
        let groups = atomic_order_scope_groups(order, scope);
        self.add_instruction(format!("fence{}", owned_modifier_groups(&groups)));
    }

    #[must_use]
    pub fn atomic_wait(
        &mut self,
        result: &str,
        pointer: &str,
        expected: &str,
        timeout: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = atomic.wait {}, {}, {}",
            value_reference(pointer),
            value_reference(expected),
            value_reference(timeout)
        ));
        result
    }

    #[must_use]
    pub fn atomic_notify(&mut self, result: &str, pointer: &str, count: &str) -> String {
        self.binary_value_instruction(result, "atomic.notify", pointer, count)
    }

    #[must_use]
    pub fn gpu_barrier(&mut self, scope: Option<&str>, memory: Option<&[&str]>) {
        let scope = scope.unwrap_or("threadgroup");
        let memory = memory.map_or_else(String::new, |domains| {
            format!(" [memory: {}]", domains.join(", "))
        });
        self.add_instruction(format!("gpu.barrier [{scope}]{memory}"));
    }

    pub fn gpu_memfence(&mut self, scope: Option<&str>, memory: Option<&[&str]>) {
        let scope = scope.unwrap_or("device");
        let memory = memory.map_or_else(String::new, |domains| {
            format!(" [memory: {}]", domains.join(", "))
        });
        self.add_instruction(format!("gpu.memfence [{scope}]{memory}"));
    }

    #[must_use]
    pub fn gpu_builtin(&mut self, result: &str, name: &str, ty: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = gpu.builtin {name} : {ty}"));
        result
    }

    #[must_use]
    pub fn gpu_spec_const(&mut self, result: &str, id: u32, ty: &str, default: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = gpu.spec_const {id}, {ty}, {default}"));
        result
    }

    #[must_use]
    pub fn gpu_call_vft(
        &mut self,
        result: Option<&str>,
        table: &str,
        index: &str,
        arguments: &[&str],
    ) -> Option<String> {
        self.call_instruction(
            result,
            format!(
                "gpu.call.vft {}, {} ({})",
                value_reference(table),
                value_reference(index),
                operand_list(arguments)
            ),
        )
    }

    pub fn frag_demote(&mut self) {
        self.add_instruction("frag.demote");
    }

    #[must_use]
    pub fn frag_load_attachment(&mut self, result: &str, attachment: u32) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = frag.load_attachment {attachment}"));
        result
    }

    pub fn frag_raster_order_begin(&mut self) {
        self.add_instruction("frag.raster_order.begin");
    }

    pub fn frag_raster_order_end(&mut self) {
        self.add_instruction("frag.raster_order.end");
    }

    #[must_use]
    pub fn frag_imageblock_addr(&mut self, result: &str) -> String {
        self.unit_result(result, "frag.imageblock.addr")
    }

    pub fn gpu_printf(&mut self, format_string: &str, arguments: &[&str]) {
        let mut format_literal = String::new();
        push_string(&mut format_literal, format_string);
        let arguments = operand_list(arguments);
        let separator = if arguments.is_empty() { "" } else { ", " };
        self.add_instruction(format!("gpu.printf {format_literal}{separator}{arguments}"));
    }

    #[must_use]
    pub fn gpu_derivative(
        &mut self,
        result: &str,
        opcode: &str,
        value: &str,
        quality: Option<&str>,
    ) -> String {
        let result = value_reference(result);
        let quality = quality.map_or_else(String::new, |quality| format!(" [{quality}]"));
        self.add_instruction(format!(
            "{result} = {opcode}{quality} {}",
            value_reference(value)
        ));
        result
    }

    #[must_use]
    pub fn quad_broadcast(&mut self, result: &str, value: &str, lane: &str) -> String {
        self.binary_value_instruction(result, "quad.broadcast", value, lane)
    }

    #[must_use]
    pub fn quad_swap(&mut self, result: &str, value: &str, axis: Option<&str>) -> String {
        let result = value_reference(result);
        let axis = axis.map_or_else(String::new, |axis| format!(" [{axis}]"));
        self.add_instruction(format!(
            "{result} = quad.swap{axis} {}",
            value_reference(value)
        ));
        result
    }

    #[must_use]
    pub fn quad_reduce(&mut self, result: &str, opcode: &str, value: &str) -> String {
        self.unary_value_instruction(result, opcode, value)
    }

    #[must_use]
    pub fn subgroup_elect(&mut self, result: &str) -> String {
        self.unit_result(result, "subgroup.elect")
    }

    #[must_use]
    pub fn subgroup_unary(&mut self, result: &str, opcode: &str, value: &str) -> String {
        self.unary_value_instruction(result, opcode, value)
    }

    #[must_use]
    pub fn subgroup_binary(
        &mut self,
        result: &str,
        opcode: &str,
        value: &str,
        lane: &str,
    ) -> String {
        self.binary_value_instruction(result, opcode, value, lane)
    }

    #[must_use]
    pub fn subgroup_reduce(
        &mut self,
        result: &str,
        opcode: &str,
        operator: &str,
        value: &str,
        cluster_size: Option<u32>,
    ) -> String {
        let result = value_reference(result);
        let cluster = cluster_size.map_or_else(String::new, |size| format!(" [cluster {size}]"));
        self.add_instruction(format!(
            "{result} = {opcode} {operator} {}{cluster}",
            value_reference(value)
        ));
        result
    }

    /// Emits any section 17.6 texture operation using TIR operand fragments.
    /// Use the `sparse` flag only for opcodes that define a residency result.
    #[must_use]
    pub fn texture_operation(
        &mut self,
        results: &[&str],
        opcode: &str,
        operands: &[&str],
        sparse: bool,
    ) -> Vec<String> {
        let modifiers = if sparse { &["sparse"][..] } else { &[] };
        self.emit_instruction(results, opcode, modifiers, operands)
    }

    /// Emits a direct call. `callee` is an `@symbol`; every argument is TIR operand text.
    #[must_use]
    pub fn call(
        &mut self,
        result: Option<&str>,
        callee: &str,
        generic_arguments: &[&str],
        arguments: &[&str],
        attributes: &[&str],
    ) -> Option<String> {
        let callee = symbol_reference(callee);
        let generics = angle_list(generic_arguments);
        let attributes = attributes.join(" ");
        let attributes = modifier_group((!attributes.is_empty()).then_some(attributes.as_str()));
        self.call_instruction(
            result,
            format!(
                "call{attributes} {callee}{generics}({})",
                operand_list(arguments)
            ),
        )
    }

    #[must_use]
    pub fn call_indirect(
        &mut self,
        result: Option<&str>,
        function: &str,
        function_type: &str,
        arguments: &[&str],
    ) -> Option<String> {
        self.call_instruction(
            result,
            format!(
                "call.indirect {} : {function_type} ({})",
                value_reference(function),
                operand_list(arguments)
            ),
        )
    }

    #[must_use]
    pub fn closure_new(&mut self, result: &str, code: &str, context: &str) -> String {
        self.binary_value_instruction(result, "closure.new", code, context)
    }

    #[must_use]
    pub fn call_closure(
        &mut self,
        result: Option<&str>,
        closure: &str,
        arguments: &[&str],
        attributes: &[&str],
    ) -> Option<String> {
        let attributes = attributes.join(" ");
        let attributes = modifier_group((!attributes.is_empty()).then_some(attributes.as_str()));
        self.call_instruction(
            result,
            format!(
                "call.closure{attributes} {}({})",
                value_reference(closure),
                operand_list(arguments)
            ),
        )
    }

    #[must_use]
    pub fn call_foreign(
        &mut self,
        result: Option<&str>,
        symbol: &str,
        arguments: &[&str],
    ) -> Option<String> {
        self.call_instruction(
            result,
            format!(
                "call.foreign {}({})",
                symbol_reference(symbol),
                operand_list(arguments)
            ),
        )
    }

    #[must_use]
    pub fn closure_callback(
        &mut self,
        result: &str,
        parameter_index: u32,
        function_type: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = closure.callback {parameter_index}, {function_type}"
        ));
        result
    }

    #[must_use]
    pub fn call_witness(
        &mut self,
        result: Option<&str>,
        ty: &str,
        trait_method: &str,
        arguments: &[&str],
    ) -> Option<String> {
        self.call_instruction(
            result,
            format!(
                "call.witness {ty}, {trait_method}({})",
                operand_list(arguments)
            ),
        )
    }

    #[must_use]
    pub fn call_vtable(
        &mut self,
        result: Option<&str>,
        object: &str,
        vtable: &str,
        slot: u32,
        arguments: &[&str],
    ) -> Option<String> {
        self.call_instruction(
            result,
            format!(
                "call.vtable {}, {}, {slot}({})",
                value_reference(object),
                symbol_reference(vtable),
                operand_list(arguments)
            ),
        )
    }

    #[must_use]
    pub fn call_variadic(
        &mut self,
        result: Option<&str>,
        callee: &str,
        fixed_arguments: &[&str],
        variadic_arguments: &[&str],
    ) -> Option<String> {
        self.call_instruction(
            result,
            format!(
                "call.variadic {}({}; {})",
                call_target(callee),
                operand_list(fixed_arguments),
                operand_list(variadic_arguments)
            ),
        )
    }

    pub fn invoke(
        &mut self,
        callee: &str,
        arguments: &[&str],
        normal_block: &str,
        unwind_block: &str,
    ) {
        self.add_instruction(format!(
            "invoke {}({}) to {} unwind {}",
            call_target(callee),
            operand_list(arguments),
            block_reference(normal_block),
            block_reference(unwind_block)
        ));
    }

    #[must_use]
    pub fn coro_start(
        &mut self,
        result: &str,
        function: &str,
        arguments: &[&str],
        allocation: Option<(&str, Option<&str>)>,
    ) -> String {
        let result = value_reference(result);
        let allocation = allocation.map_or_else(String::new, |(kind, slot)| {
            let slot = slot.map_or_else(String::new, |slot| format!(" {}", value_reference(slot)));
            format!(" [{kind}{slot}]")
        });
        self.add_instruction(format!(
            "{result} = coro.start {}({}){allocation}",
            symbol_reference(function),
            operand_list(arguments)
        ));
        result
    }

    pub fn coro_yield(&mut self, yielded: &str, resume_block: &str, destroy_block: &str) {
        self.add_instruction(format!(
            "coro.yield {}, resume {}, destroy {}",
            value_reference(yielded),
            block_reference(resume_block),
            block_reference(destroy_block)
        ));
    }

    pub fn coro_resume(
        &mut self,
        handle: &str,
        input: &str,
        yielded_block: &str,
        done_block: &str,
    ) {
        self.add_instruction(format!(
            "coro.resume {}, {}, yielded {}, done {}",
            value_reference(handle),
            value_reference(input),
            block_reference(yielded_block),
            block_reference(done_block)
        ));
    }

    pub fn coro_finish(&mut self) {
        self.add_instruction("coro.finish");
    }

    pub fn coro_destroy(&mut self, handle: &str) {
        self.add_instruction(format!("coro.destroy {}", value_reference(handle)));
    }

    #[must_use]
    pub fn coro_done(&mut self, result: &str, handle: &str) -> String {
        self.unary_value_instruction(result, "coro.done", handle)
    }

    #[must_use]
    pub fn coro_frame_size(&mut self, result: &str, function: &str) -> String {
        self.symbol_result(result, "coro.frame.size", function)
    }

    #[must_use]
    pub fn coro_frame_align(&mut self, result: &str, function: &str) -> String {
        self.symbol_result(result, "coro.frame.align", function)
    }

    #[must_use]
    pub fn await_future(&mut self, result: &str, future: &str) -> String {
        self.unary_value_instruction(result, "await", future)
    }

    #[must_use]
    pub fn async_waker(&mut self, result: &str) -> String {
        self.unit_result(result, "async.waker")
    }

    #[must_use]
    pub fn landingpad(
        &mut self,
        box_result: &str,
        clause_result: &str,
        clauses: &[&str],
    ) -> (String, String) {
        let box_result = value_reference(box_result);
        let clause_result = value_reference(clause_result);
        self.add_instruction(format!(
            "{box_result}, {clause_result} = landingpad [{}]",
            clauses.join(", ")
        ));
        (box_result, clause_result)
    }

    pub fn resume_unwind(&mut self, landingpad: &str) {
        self.add_instruction(format!("resume {}", value_reference(landingpad)));
    }

    pub fn throw(&mut self, exception_box: &str, payload_type: &str) {
        self.add_instruction(format!(
            "throw {} as {payload_type}",
            value_reference(exception_box)
        ));
    }

    #[must_use]
    pub fn exception_payload(&mut self, result: &str, exception_box: &str) -> String {
        self.unary_value_instruction(result, "exception.payload", exception_box)
    }

    #[must_use]
    pub fn exception_type_id(&mut self, result: &str, exception_box: &str) -> String {
        self.unary_value_instruction(result, "exception.typeid", exception_box)
    }

    pub fn va_start(&mut self, list: &str) {
        self.add_instruction(format!("va.start {}", value_reference(list)));
    }

    #[must_use]
    pub fn va_arg(&mut self, result: &str, ty: &str, list: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = va.arg {ty}, {}", value_reference(list)));
        result
    }

    pub fn va_copy(&mut self, destination: &str, source: &str) {
        self.add_instruction(format!(
            "va.copy {}, {}",
            value_reference(destination),
            value_reference(source)
        ));
    }

    pub fn va_end(&mut self, list: &str) {
        self.add_instruction(format!("va.end {}", value_reference(list)));
    }

    #[must_use]
    pub fn setjmp(&mut self, result: &str, buffer: &str) -> String {
        self.unary_value_instruction(result, "setjmp", buffer)
    }

    pub fn longjmp(&mut self, buffer: &str, value: &str) {
        self.add_instruction(format!(
            "longjmp {}, {}",
            value_reference(buffer),
            value_reference(value)
        ));
    }

    #[must_use]
    pub fn inline_asm(
        &mut self,
        result: Option<&str>,
        template: &str,
        constraints: &str,
        arguments: &[&str],
        result_type: &str,
        attributes: &[&str],
    ) -> Option<String> {
        let mut template_literal = String::new();
        push_string(&mut template_literal, template);
        let mut constraints_literal = String::new();
        push_string(&mut constraints_literal, constraints);
        let mut suffix = String::new();
        for attribute in attributes {
            let _ = write!(suffix, " [{attribute}]");
        }
        self.call_instruction(
            result,
            format!(
                "asm {template_literal}, {constraints_literal}({}) -> {result_type}{suffix}",
                operand_list(arguments)
            ),
        )
    }

    pub fn inline_asm_goto(
        &mut self,
        template: &str,
        constraints: &str,
        arguments: &[&str],
        normal_block: &str,
        label_blocks: &[&str],
    ) {
        let mut template_literal = String::new();
        push_string(&mut template_literal, template);
        let mut constraints_literal = String::new();
        push_string(&mut constraints_literal, constraints);
        let labels = label_blocks
            .iter()
            .map(|label| block_reference(label))
            .collect::<Vec<_>>()
            .join(", ");
        self.add_instruction(format!(
            "asm.goto {template_literal}, {constraints_literal}({}) normal {}, labels [{labels}]",
            operand_list(arguments),
            block_reference(normal_block)
        ));
    }

    #[must_use]
    pub fn target_intrinsic(
        &mut self,
        result: Option<&str>,
        name: &str,
        arguments: &[&str],
        result_type: &str,
    ) -> Option<String> {
        let mut name_literal = String::new();
        push_string(&mut name_literal, name);
        self.call_instruction(
            result,
            format!(
                "target.intrinsic {name_literal}({}) -> {result_type}",
                operand_list(arguments)
            ),
        )
    }

    #[must_use]
    pub fn target_has(&mut self, result: &str, feature: &str) -> String {
        let mut feature_literal = String::new();
        push_string(&mut feature_literal, feature);
        self.unit_result(result, &format!("target.has {feature_literal}"))
    }

    #[must_use]
    pub fn target_pagesize(&mut self, result: &str) -> String {
        self.unit_result(result, "target.pagesize")
    }

    #[must_use]
    pub fn intrinsic(
        &mut self,
        result: Option<&str>,
        id: u32,
        arguments: &[&str],
        result_type: &str,
    ) -> Option<String> {
        self.call_instruction(
            result,
            format!(
                "intrinsic {id}({}) -> {result_type}",
                operand_list(arguments)
            ),
        )
    }

    pub fn debug_value(&mut self, value: &str, name: &str, ty: &str, scope: &str) {
        let mut name_literal = String::new();
        push_string(&mut name_literal, name);
        let mut scope_literal = String::new();
        push_string(&mut scope_literal, scope);
        self.add_instruction(format!(
            "debug.value {}, !var({name_literal}, {ty}, {scope_literal})",
            value_reference(value)
        ));
    }

    pub fn debug_addr(&mut self, address: &str, name: &str, ty: &str, scope: &str) {
        let mut name_literal = String::new();
        push_string(&mut name_literal, name);
        let mut scope_literal = String::new();
        push_string(&mut scope_literal, scope);
        self.add_instruction(format!(
            "debug.addr {}, !var({name_literal}, {ty}, {scope_literal})",
            value_reference(address)
        ));
    }

    pub fn debug_label(&mut self, name: &str, scope: &str) {
        let mut name_literal = String::new();
        push_string(&mut name_literal, name);
        let mut scope_literal = String::new();
        push_string(&mut scope_literal, scope);
        self.add_instruction(format!(
            "debug.label !label({name_literal}, {scope_literal})"
        ));
    }

    pub fn assume(&mut self, condition: &str) {
        self.add_instruction(format!("assume {}", value_reference(condition)));
    }

    pub fn expect(&mut self, value: &str, expected: &str, probability: Option<&str>) {
        let probability =
            probability.map_or_else(String::new, |value| format!(" [probability {value}]"));
        self.add_instruction(format!(
            "expect {}, {expected}{probability}",
            value_reference(value)
        ));
    }

    pub fn nop(&mut self) {
        self.add_instruction("nop");
    }

    pub fn prof_count(&mut self, id: u32) {
        self.add_instruction(format!("prof.count {id}"));
    }

    #[must_use]
    pub fn type_size(&mut self, result: &str, ty: &str) -> String {
        self.typed_result(result, "type.size", ty)
    }

    #[must_use]
    pub fn type_align(&mut self, result: &str, ty: &str) -> String {
        self.typed_result(result, "type.align", ty)
    }

    #[must_use]
    pub fn type_stride(&mut self, result: &str, ty: &str) -> String {
        self.typed_result(result, "type.stride", ty)
    }

    pub fn guard(&mut self, condition: &str, fallback: &str, state: &[&str]) {
        self.add_instruction(format!(
            "guard {}, deopt {}({})",
            value_reference(condition),
            symbol_reference(fallback),
            operand_list(state)
        ));
    }

    pub fn deopt(&mut self, fallback: &str, state: &[&str]) {
        self.add_instruction(format!(
            "deopt {}({})",
            symbol_reference(fallback),
            operand_list(state)
        ));
    }

    #[must_use]
    pub fn mmio_load(
        &mut self,
        result: &str,
        ty: &str,
        pointer: &str,
        order: Option<&str>,
    ) -> String {
        let order = order.map_or_else(String::new, |order| format!(" [order {order}]"));
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = mmio.load{order} {ty}, {}",
            value_reference(pointer)
        ));
        result
    }

    pub fn mmio_store(&mut self, value: &str, pointer: &str, order: Option<&str>) {
        let order = order.map_or_else(String::new, |order| format!(" [order {order}]"));
        self.add_instruction(format!(
            "mmio.store{order} {}, {}",
            value_reference(value),
            value_reference(pointer)
        ));
    }

    pub fn fence_io(&mut self, mode: Option<&str>) {
        let mode = mode.map_or_else(String::new, |mode| format!(" [{mode}]"));
        self.add_instruction(format!("fence.io{mode}"));
    }

    #[must_use]
    pub fn kernel_percpu_addr(&mut self, result: &str, global: &str) -> String {
        self.symbol_result(result, "kernel.percpu.addr", global)
    }

    #[must_use]
    pub fn foreign_addr(&mut self, result: &str, import: &str) -> String {
        self.symbol_result(result, "foreign.addr", import)
    }

    #[must_use]
    pub fn foreign_data(&mut self, result: &str, import: &str) -> String {
        self.symbol_result(result, "foreign.data", import)
    }

    #[must_use]
    pub fn foreign_entry(&mut self, result: &str, function: &str) -> String {
        self.symbol_result(result, "foreign.entry", function)
    }

    pub fn tail_call(&mut self, callee: &str, arguments: &[&str]) {
        self.add_instruction(format!(
            "tail.call {}({})",
            call_target(callee),
            operand_list(arguments)
        ));
    }

    pub fn try_call(
        &mut self,
        callee: &str,
        arguments: &[&str],
        normal_block: &str,
        error_block: &str,
    ) {
        self.add_instruction(format!(
            "try.call {}({}) normal {}, error {}",
            call_target(callee),
            operand_list(arguments),
            block_reference(normal_block),
            block_reference(error_block)
        ));
    }

    unary_instruction_methods!(
        fneg => "fneg",
        fabs => "fabs",
        fsqrt => "fsqrt",
        ffloor => "ffloor",
        fceil => "fceil",
        ftrunc => "ftrunc",
        fround => "fround",
        froundeven => "froundeven",
        fclass => "fclass",
        math_sin => "math.sin",
        math_cos => "math.cos",
        math_tan => "math.tan",
        math_asin => "math.asin",
        math_acos => "math.acos",
        math_atan => "math.atan",
        math_sinh => "math.sinh",
        math_cosh => "math.cosh",
        math_tanh => "math.tanh",
        math_asinh => "math.asinh",
        math_acosh => "math.acosh",
        math_atanh => "math.atanh",
        math_exp => "math.exp",
        math_exp2 => "math.exp2",
        math_exp10 => "math.exp10",
        math_expm1 => "math.expm1",
        math_log => "math.log",
        math_log2 => "math.log2",
        math_log10 => "math.log10",
        math_log1p => "math.log1p",
        math_cbrt => "math.cbrt",
        math_rsqrt => "math.rsqrt",
    );

    binary_instruction_methods!(
        fadd => "fadd",
        fsub => "fsub",
        fmul => "fmul",
        fdiv => "fdiv",
        frem => "frem",
        fmin => "fmin",
        fmax => "fmax",
        fminnum => "fminnum",
        fmaxnum => "fmaxnum",
        fcopysign => "fcopysign",
        math_atan2 => "math.atan2",
        math_pow => "math.pow",
        math_hypot => "math.hypot",
    );

    ternary_instruction_methods!(fma => "fma", fmuladd => "fmuladd");

    #[must_use]
    pub fn fcmp(&mut self, result: &str, predicate: &str, lhs: &str, rhs: &str) -> String {
        self.prefixed_binary(result, "fcmp", predicate, lhs, rhs)
    }

    #[must_use]
    pub fn ldexp(&mut self, result: &str, value: &str, exponent: &str) -> String {
        self.integer_binary(result, "ldexp", None, value, exponent)
    }

    #[must_use]
    pub fn frexp(&mut self, fraction: &str, exponent: &str, value: &str) -> (String, String) {
        let fraction = value_reference(fraction);
        let exponent = value_reference(exponent);
        self.add_instruction(format!(
            "{fraction}, {exponent} = frexp {}",
            value_reference(value)
        ));
        (fraction, exponent)
    }

    pub fn fp_flags_clear(&mut self) {
        self.add_instruction("fp.flags.clear");
    }

    #[must_use]
    pub fn fp_flags_read(&mut self, result: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = fp.flags.read"));
        result
    }

    #[must_use]
    pub fn trunc(&mut self, result: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "trunc", None, value, target_type)
    }

    #[must_use]
    pub fn zext(&mut self, result: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "zext", None, value, target_type)
    }

    #[must_use]
    pub fn sext(&mut self, result: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "sext", None, value, target_type)
    }

    #[must_use]
    pub fn fptrunc(
        &mut self,
        result: &str,
        rounding: Option<&str>,
        value: &str,
        target_type: &str,
    ) -> String {
        self.conversion(result, "fptrunc", rounding, value, target_type)
    }

    #[must_use]
    pub fn fpext(&mut self, result: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "fpext", None, value, target_type)
    }

    #[must_use]
    pub fn sitofp(
        &mut self,
        result: &str,
        rounding: Option<&str>,
        value: &str,
        target_type: &str,
    ) -> String {
        self.conversion(result, "sitofp", rounding, value, target_type)
    }

    #[must_use]
    pub fn uitofp(
        &mut self,
        result: &str,
        rounding: Option<&str>,
        value: &str,
        target_type: &str,
    ) -> String {
        self.conversion(result, "uitofp", rounding, value, target_type)
    }

    #[must_use]
    pub fn fptosi(&mut self, result: &str, mode: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "fptosi", Some(mode), value, target_type)
    }

    #[must_use]
    pub fn fptoui(&mut self, result: &str, mode: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "fptoui", Some(mode), value, target_type)
    }

    #[must_use]
    pub fn ptrtoint(&mut self, result: &str, value: &str) -> String {
        self.conversion(result, "ptrtoint", None, value, "i64")
    }

    #[must_use]
    pub fn inttoptr(&mut self, result: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "inttoptr", None, value, target_type)
    }

    #[must_use]
    pub fn bitcast(&mut self, result: &str, value: &str, target_type: &str) -> String {
        self.conversion(result, "bitcast", None, value, target_type)
    }

    #[must_use]
    pub fn fcvt(
        &mut self,
        result: &str,
        modifiers: Option<&str>,
        value: &str,
        target_type: &str,
    ) -> String {
        self.conversion(result, "fcvt", modifiers, value, target_type)
    }

    #[must_use]
    pub fn add(&mut self, result: &str, mode: Option<&str>, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "add", mode, lhs, rhs)
    }

    #[must_use]
    pub fn sub(&mut self, result: &str, mode: Option<&str>, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "sub", mode, lhs, rhs)
    }

    #[must_use]
    pub fn mul(&mut self, result: &str, mode: Option<&str>, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "mul", mode, lhs, rhs)
    }

    #[must_use]
    pub fn bit_and(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "and", None, lhs, rhs)
    }

    #[must_use]
    pub fn bit_or(&mut self, result: &str, disjoint: bool, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "or", disjoint.then_some("disjoint"), lhs, rhs)
    }

    #[must_use]
    pub fn bit_xor(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "xor", None, lhs, rhs)
    }

    #[must_use]
    pub fn mulh(&mut self, result: &str, signedness: &str, lhs: &str, rhs: &str) -> String {
        self.prefixed_binary(result, "mulh", signedness, lhs, rhs)
    }

    #[must_use]
    pub fn add_ovf(
        &mut self,
        result: &str,
        overflow: &str,
        signedness: &str,
        lhs: &str,
        rhs: &str,
    ) -> (String, String) {
        self.prefixed_multi_result_binary(result, overflow, "add.ovf", signedness, lhs, rhs)
    }

    #[must_use]
    pub fn sub_ovf(
        &mut self,
        result: &str,
        overflow: &str,
        signedness: &str,
        lhs: &str,
        rhs: &str,
    ) -> (String, String) {
        self.prefixed_multi_result_binary(result, overflow, "sub.ovf", signedness, lhs, rhs)
    }

    #[must_use]
    pub fn mul_ovf(
        &mut self,
        result: &str,
        overflow: &str,
        signedness: &str,
        lhs: &str,
        rhs: &str,
    ) -> (String, String) {
        self.prefixed_multi_result_binary(result, overflow, "mul.ovf", signedness, lhs, rhs)
    }

    #[must_use]
    pub fn sdiv(
        &mut self,
        result: &str,
        mode: Option<&str>,
        exact: bool,
        lhs: &str,
        rhs: &str,
    ) -> String {
        let mut groups = Vec::new();
        if let Some(mode) = mode {
            groups.push(mode);
        }
        if exact {
            groups.push("exact");
        }
        self.integer_binary_groups(result, "sdiv", &groups, lhs, rhs)
    }

    #[must_use]
    pub fn udiv(&mut self, result: &str, exact: bool, lhs: &str, rhs: &str) -> String {
        self.integer_binary_groups(
            result,
            "udiv",
            if exact { &["exact"] } else { &[] },
            lhs,
            rhs,
        )
    }

    #[must_use]
    pub fn srem(&mut self, result: &str, mode: Option<&str>, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "srem", mode, lhs, rhs)
    }

    #[must_use]
    pub fn urem(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "urem", None, lhs, rhs)
    }

    #[must_use]
    pub fn neg(&mut self, result: &str, mode: Option<&str>, value: &str) -> String {
        self.integer_unary(result, "neg", mode, value)
    }

    #[must_use]
    pub fn abs(&mut self, result: &str, mode: Option<&str>, value: &str) -> String {
        self.integer_unary(result, "abs", mode, value)
    }

    #[must_use]
    pub fn not(&mut self, result: &str, value: &str) -> String {
        self.integer_unary(result, "not", None, value)
    }

    #[must_use]
    pub fn shl(&mut self, result: &str, mode: Option<&str>, value: &str, amount: &str) -> String {
        self.integer_binary(result, "shl", mode, value, amount)
    }

    #[must_use]
    pub fn lshr(
        &mut self,
        result: &str,
        mode: Option<&str>,
        exact: bool,
        value: &str,
        amount: &str,
    ) -> String {
        let mut groups = Vec::new();
        if let Some(mode) = mode {
            groups.push(mode);
        }
        if exact {
            groups.push("exact");
        }
        self.integer_binary_groups(result, "lshr", &groups, value, amount)
    }

    #[must_use]
    pub fn ashr(
        &mut self,
        result: &str,
        mode: Option<&str>,
        exact: bool,
        value: &str,
        amount: &str,
    ) -> String {
        let mut groups = Vec::new();
        if let Some(mode) = mode {
            groups.push(mode);
        }
        if exact {
            groups.push("exact");
        }
        self.integer_binary_groups(result, "ashr", &groups, value, amount)
    }

    #[must_use]
    pub fn rotl(&mut self, result: &str, value: &str, amount: &str) -> String {
        self.integer_binary(result, "rotl", None, value, amount)
    }

    #[must_use]
    pub fn rotr(&mut self, result: &str, value: &str, amount: &str) -> String {
        self.integer_binary(result, "rotr", None, value, amount)
    }

    #[must_use]
    pub fn fshl(&mut self, result: &str, lhs: &str, rhs: &str, amount: &str) -> String {
        self.integer_ternary(result, "fshl", lhs, rhs, amount)
    }

    #[must_use]
    pub fn fshr(&mut self, result: &str, lhs: &str, rhs: &str, amount: &str) -> String {
        self.integer_ternary(result, "fshr", lhs, rhs, amount)
    }

    #[must_use]
    pub fn smin(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "smin", None, lhs, rhs)
    }

    #[must_use]
    pub fn smax(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "smax", None, lhs, rhs)
    }

    #[must_use]
    pub fn umin(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "umin", None, lhs, rhs)
    }

    #[must_use]
    pub fn umax(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "umax", None, lhs, rhs)
    }

    #[must_use]
    pub fn ctlz(&mut self, result: &str, poison_if_zero: bool, value: &str) -> String {
        self.integer_unary(result, "ctlz", poison_if_zero.then_some("poison"), value)
    }

    #[must_use]
    pub fn cttz(&mut self, result: &str, poison_if_zero: bool, value: &str) -> String {
        self.integer_unary(result, "cttz", poison_if_zero.then_some("poison"), value)
    }

    #[must_use]
    pub fn ctpop(&mut self, result: &str, value: &str) -> String {
        self.integer_unary(result, "ctpop", None, value)
    }

    #[must_use]
    pub fn bswap(&mut self, result: &str, value: &str) -> String {
        self.integer_unary(result, "bswap", None, value)
    }

    #[must_use]
    pub fn bitreverse(&mut self, result: &str, value: &str) -> String {
        self.integer_unary(result, "bitreverse", None, value)
    }

    #[must_use]
    pub fn clmul(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "clmul", None, lhs, rhs)
    }

    #[must_use]
    pub fn clmulh(&mut self, result: &str, lhs: &str, rhs: &str) -> String {
        self.integer_binary(result, "clmulh", None, lhs, rhs)
    }

    #[must_use]
    pub fn freeze(&mut self, result: &str, value: &str) -> String {
        self.integer_unary(result, "freeze", None, value)
    }

    #[must_use]
    pub fn icmp(&mut self, result: &str, predicate: &str, lhs: &str, rhs: &str) -> String {
        self.prefixed_binary(result, "icmp", predicate, lhs, rhs)
    }

    #[must_use]
    pub fn select(&mut self, result: &str, condition: &str, yes: &str, no: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = select {}, {}, {}",
            value_reference(condition),
            value_reference(yes),
            value_reference(no)
        ));
        result
    }

    pub fn ret(&mut self, value: Option<&str>) {
        self.add_instruction(value.map_or_else(
            || "return".to_owned(),
            |value| format!("return {}", value_reference(value)),
        ));
    }

    pub fn branch(&mut self, target: &str, arguments: &[&str]) {
        self.add_instruction(format!("br {}", block_target(target, arguments)));
    }

    pub fn conditional_branch(
        &mut self,
        condition: &str,
        true_target: &str,
        true_arguments: &[&str],
        false_target: &str,
        false_arguments: &[&str],
    ) {
        self.add_instruction(format!(
            "cond.br {}, {}, {}",
            value_reference(condition),
            block_target(true_target, true_arguments),
            block_target(false_target, false_arguments),
        ));
    }

    pub fn conditional_branch_with_merge(
        &mut self,
        condition: &str,
        true_target: &str,
        true_arguments: &[&str],
        false_target: &str,
        false_arguments: &[&str],
        merge: Option<&str>,
    ) {
        let merge = merge.map_or_else(String::new, |block| {
            format!(" [merge {}]", block_reference(block))
        });
        self.add_instruction(format!(
            "cond.br {}, {}, {}{merge}",
            value_reference(condition),
            block_target(true_target, true_arguments),
            block_target(false_target, false_arguments),
        ));
    }

    pub fn switch_int(
        &mut self,
        value: &str,
        cases: &[(&str, &str, &[&str])],
        default_target: &str,
        default_arguments: &[&str],
        merge: Option<&str>,
    ) {
        let cases = cases
            .iter()
            .map(|(constant, target, arguments)| {
                format!("[{constant}: {}]", block_target(target, arguments))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let arms = if cases.is_empty() {
            String::new()
        } else {
            format!(", {cases}")
        };
        let merge = merge.map_or_else(String::new, |block| {
            format!(" [merge {}]", block_reference(block))
        });
        self.add_instruction(format!(
            "switch.int {}{arms}, default {}{merge}",
            value_reference(value),
            block_target(default_target, default_arguments)
        ));
    }

    pub fn switch_enum(
        &mut self,
        value: &str,
        cases: &[(&str, &str)],
        default_target: Option<&str>,
    ) {
        let mut arms = cases
            .iter()
            .map(|(variant, target)| format!("[{variant}: {}]", block_reference(target)))
            .collect::<Vec<_>>();
        if let Some(target) = default_target {
            arms.push(format!("[default {}]", block_reference(target)));
        }
        self.add_instruction(format!(
            "switch.enum {}, {}",
            value_reference(value),
            arms.join(", ")
        ));
    }

    pub fn branch_indirect(&mut self, address: &str, possible_targets: &[&str]) {
        self.add_instruction(format!(
            "br.indirect {}, [{}]",
            value_reference(address),
            possible_targets
                .iter()
                .map(|target| block_reference(target))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    pub fn fail(&mut self, error: &str) {
        self.add_instruction(format!("fail {}", value_reference(error)));
    }

    pub fn trap(&mut self, code: u16) {
        self.add_instruction(format!("trap {code}"));
    }

    pub fn unreachable_with_mode(&mut self, undefined_behavior: bool) {
        if undefined_behavior {
            self.add_instruction("unreachable [ub]");
        } else {
            self.add_instruction("unreachable");
        }
    }

    pub fn unreachable(&mut self) {
        self.add_instruction("unreachable");
    }

    fn integer_binary(
        &mut self,
        result: &str,
        opcode: &str,
        mode: Option<&str>,
        lhs: &str,
        rhs: &str,
    ) -> String {
        let groups = mode.into_iter().collect::<Vec<_>>();
        self.integer_binary_groups(result, opcode, &groups, lhs, rhs)
    }

    fn integer_binary_groups(
        &mut self,
        result: &str,
        opcode: &str,
        groups: &[&str],
        lhs: &str,
        rhs: &str,
    ) -> String {
        let result = value_reference(result);
        let mode = modifier_groups(groups);
        self.add_instruction(format!(
            "{result} = {opcode}{mode} {}, {}",
            value_reference(lhs),
            value_reference(rhs)
        ));
        result
    }

    fn integer_unary(
        &mut self,
        result: &str,
        opcode: &str,
        mode: Option<&str>,
        value: &str,
    ) -> String {
        let result = value_reference(result);
        let mode = modifier_group(mode);
        self.add_instruction(format!(
            "{result} = {opcode}{mode} {}",
            value_reference(value)
        ));
        result
    }

    fn prefixed_binary(
        &mut self,
        result: &str,
        opcode: &str,
        prefix: &str,
        lhs: &str,
        rhs: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = {opcode} {prefix} {}, {}",
            value_reference(lhs),
            value_reference(rhs)
        ));
        result
    }

    fn prefixed_multi_result_binary(
        &mut self,
        result: &str,
        overflow: &str,
        opcode: &str,
        prefix: &str,
        lhs: &str,
        rhs: &str,
    ) -> (String, String) {
        let result = value_reference(result);
        let overflow = value_reference(overflow);
        self.add_instruction(format!(
            "{result}, {overflow} = {opcode} {prefix} {}, {}",
            value_reference(lhs),
            value_reference(rhs)
        ));
        (result, overflow)
    }

    fn integer_ternary(
        &mut self,
        result: &str,
        opcode: &str,
        first: &str,
        second: &str,
        third: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = {opcode} {}, {}, {}",
            value_reference(first),
            value_reference(second),
            value_reference(third)
        ));
        result
    }

    fn conversion(
        &mut self,
        result: &str,
        opcode: &str,
        modifiers: Option<&str>,
        value: &str,
        target_type: &str,
    ) -> String {
        let result = value_reference(result);
        let modifiers = modifier_group(modifiers);
        self.add_instruction(format!(
            "{result} = {opcode}{modifiers} {} to {target_type}",
            value_reference(value)
        ));
        result
    }

    fn symbol_result(&mut self, result: &str, opcode: &str, symbol: &str) -> String {
        let result = value_reference(result);
        let symbol = symbol_reference(symbol);
        self.add_instruction(format!("{result} = {opcode} {symbol}"));
        result
    }

    fn construct_aggregate(
        &mut self,
        result: &str,
        opcode: &str,
        ty: &str,
        fields: &[&str],
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = {opcode} {ty}({})", value_list(fields)));
        result
    }

    fn extract_aggregate(
        &mut self,
        result: &str,
        opcode: &str,
        value: &str,
        index: String,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = {opcode} {}, {index}",
            value_reference(value)
        ));
        result
    }

    fn multi_result_instruction(
        &mut self,
        results: &[&str],
        opcode: &str,
        operands: &[String],
    ) -> Vec<String> {
        let results = results
            .iter()
            .map(|result| value_reference(result))
            .collect::<Vec<_>>();
        self.add_instruction(format!(
            "{} = {opcode} {}",
            results.join(", "),
            operands.join(", ")
        ));
        results
    }

    fn unary_value_instruction(&mut self, result: &str, opcode: &str, value: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = {opcode} {}", value_reference(value)));
        result
    }

    fn binary_value_instruction(
        &mut self,
        result: &str,
        opcode: &str,
        first: &str,
        second: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = {opcode} {}, {}",
            value_reference(first),
            value_reference(second)
        ));
        result
    }

    fn ternary_value_instruction(
        &mut self,
        result: &str,
        opcode: &str,
        first: &str,
        second: &str,
        third: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = {opcode} {}, {}, {}",
            value_reference(first),
            value_reference(second),
            value_reference(third)
        ));
        result
    }

    fn indexed_address(
        &mut self,
        result: &str,
        opcode: &str,
        address: &str,
        index: String,
        mode: Option<&str>,
    ) -> String {
        let result = value_reference(result);
        let mode = modifier_group(mode);
        self.add_instruction(format!(
            "{result} = {opcode}{mode} {}, {index}",
            value_reference(address)
        ));
        result
    }

    fn cast_address(
        &mut self,
        result: &str,
        opcode: &str,
        address: &str,
        delimiter: &str,
        target_type: &str,
    ) -> String {
        let result = value_reference(result);
        self.add_instruction(format!(
            "{result} = {opcode} {} {delimiter} {target_type}",
            value_reference(address)
        ));
        result
    }

    fn address_operation(
        &mut self,
        result: &str,
        opcode: &str,
        mode: Option<&str>,
        address: &str,
    ) -> String {
        let result = value_reference(result);
        let mode = modifier_group(mode);
        self.add_instruction(format!(
            "{result} = {opcode}{mode} {}",
            value_reference(address)
        ));
        result
    }

    fn typed_result(&mut self, result: &str, opcode: &str, ty: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = {opcode} {ty}"));
        result
    }

    fn unit_result(&mut self, result: &str, opcode: &str) -> String {
        let result = value_reference(result);
        self.add_instruction(format!("{result} = {opcode}"));
        result
    }

    fn call_instruction(&mut self, result: Option<&str>, instruction: String) -> Option<String> {
        let result = result.map(value_reference);
        if let Some(result) = &result {
            self.add_instruction(format!("{result} = {instruction}"));
        } else {
            self.add_instruction(instruction);
        }
        result
    }

    fn finish(self) -> String {
        let mut output = String::new();
        output.push_str(self.header.trim());
        output.push('\n');
        for instruction in self.instructions {
            output.push_str("  ");
            output.push_str(instruction.trim());
            output.push('\n');
        }
        output
    }
}

fn block_target(target: &str, arguments: &[&str]) -> String {
    if arguments.is_empty() {
        target.to_owned()
    } else {
        format!(
            "{target}({})",
            arguments
                .iter()
                .map(|argument| value_reference(argument))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn block_reference(block: &str) -> &str {
    block
}

fn value_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| value_reference(value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn operand_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .collect::<Vec<_>>()
        .join(", ")
}

fn angle_list(values: &[&str]) -> String {
    if values.is_empty() {
        String::new()
    } else {
        format!("<{}>", operand_list(values))
    }
}

fn call_target(callee: &str) -> String {
    if callee.starts_with('@') || callee.starts_with('%') {
        callee.to_owned()
    } else {
        symbol_reference(callee)
    }
}

fn symbol_reference(symbol: &str) -> String {
    if symbol.starts_with('@') {
        symbol.to_owned()
    } else {
        format!("@{symbol}")
    }
}

fn value_reference(value: &str) -> String {
    if value.starts_with('%') {
        value.to_owned()
    } else {
        format!("%{value}")
    }
}

fn modifier_group(modifiers: Option<&str>) -> String {
    modifiers.map_or_else(String::new, |modifiers| format!(" [{modifiers}]"))
}

fn modifier_groups(groups: &[&str]) -> String {
    groups
        .iter()
        .map(|group| format!(" [{group}]"))
        .collect::<String>()
}

fn owned_modifier_groups(groups: &[String]) -> String {
    groups
        .iter()
        .map(|group| format!(" [{group}]"))
        .collect::<String>()
}

fn atomic_order_scope_groups(order: Option<&str>, scope: Option<&str>) -> Vec<String> {
    let mut groups = Vec::new();
    if let Some(order) = order {
        groups.push(format!("order={order}"));
    }
    if let Some(scope) = scope {
        groups.push(format!("scope={scope}"));
    }
    groups
}

fn render_header(header: &ModuleHeader) -> String {
    let mut output = String::new();
    let _ = write!(
        output,
        "module @{} tir {}.{}.{} profile {}",
        header.path,
        header.version.year,
        header.version.month,
        header.version.week,
        header.profile.name(),
    );
    if let Some(platform) = header.platform {
        let _ = write!(output, " platform {}", platform.name());
    }
    if let Some(semantics) = header.semantics {
        output.push_str(match semantics {
            tir_format::SemanticsMode::Defined => " semantics defined",
            tir_format::SemanticsMode::Permissive => " semantics permissive",
        });
    }
    if let Some(producer) = &header.producer {
        output.push_str("\n  producer ");
        push_string(&mut output, &producer.name);
        output.push(' ');
        push_string(&mut output, &producer.version);
        output.push_str(" languages [");
        for (index, language) in producer.languages.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            output.push_str(language);
        }
        output.push(']');
    }
    for (key, value) in &header.attributes {
        output.push_str("\n  attribute ");
        push_string(&mut output, key);
        output.push_str(" = ");
        push_string(&mut output, value);
    }
    for initializer in &header.initializers {
        let _ = write!(
            output,
            "\n  init @{} priority {}",
            initializer.path, initializer.priority
        );
    }
    for finalizer in &header.finalizers {
        let _ = write!(
            output,
            "\n  fini @{} priority {}",
            finalizer.path, finalizer.priority
        );
    }
    if let Some(source_hash) = &header.source_hash {
        output.push_str("\n  source ");
        push_string(&mut output, source_hash);
    }
    output.push('\n');
    output
}

fn push_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            '\r' => output.push_str("\\r"),
            '\0' => output.push_str("\\0"),
            '\u{1b}' => output.push_str("\\e"),
            other => output.push(other),
        }
    }
    output.push('"');
}
