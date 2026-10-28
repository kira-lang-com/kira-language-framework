//! Locals and statements (section 12). A statement appends to the innermost
//! open block of its function; the builder closes `Emit` ranges itself
//! (section 6.4).

use super::{Args, NONE, ShaderState};
use klf_shader::builder::FunctionBuilder;
use klf_shader::{
    AtomicFunction, BarrierFlags, Expression, Handle, Module, Span, Statement, SwitchValue,
};

type Opened<'a> = (&'a mut Module, &'a mut FunctionBuilder, Span);

fn handle(word: u64) -> Result<Handle<Expression>, String> {
    u32::try_from(word)
        .map(Handle::new)
        .map_err(|_| "an expression handle out of range".to_owned())
}

fn switch_value(text: &str) -> Result<SwitchValue, String> {
    let bad = || format!("unknown case value `{text}`");
    if text == "default" {
        return Ok(SwitchValue::Default);
    }
    if let Some(digits) = text.strip_suffix("i32") {
        return digits.parse().map(SwitchValue::I32).map_err(|_| bad());
    }
    if let Some(digits) = text.strip_suffix("u32") {
        return digits.parse().map(SwitchValue::U32).map_err(|_| bad());
    }
    Err(bad())
}

fn result(value: Option<Handle<Expression>>) -> u64 {
    value.map_or(NONE, |handle| u64::from(handle.raw()))
}

impl ShaderState {
    /// The module and open function of a statement service.
    fn opened(&mut self, args: &Args) -> Result<Opened<'_>, String> {
        let (build, function) = self.scope(args.word(0)?, args.word(1)?)?;
        let pending = function.ok_or_else(|| "a statement outside a function".to_owned())?;
        Ok((&mut build.builder.module, &mut pending.builder, build.span))
    }

    fn statement(&mut self, args: &Args, statement: Statement) -> Result<u64, String> {
        let (_, builder, span) = self.opened(args)?;
        builder
            .statement(statement, span)
            .map_err(|error| error.0)?;
        Ok(0)
    }

    fn structure(
        &mut self,
        args: &Args,
        step: impl FnOnce(&mut FunctionBuilder, &Module, Span) -> klf_shader::builder::Result<()>,
    ) -> Result<u64, String> {
        let (module, builder, span) = self.opened(args)?;
        step(builder, module, span).map_err(|error| error.0)?;
        Ok(0)
    }

    /// `klf.shader.local(module, function, name, type, init)`.
    pub(super) fn local(&mut self, args: &Args) -> Result<u64, String> {
        let name = args.text(0)?.to_owned();
        let (ty, init) = (args.handle(2)?, args.optional(3)?);
        let (module, builder, span) = self.opened(args)?;
        let local = builder
            .local(module, Some(&name), ty, init, span)
            .map_err(|error| error.0)?;
        Ok(u64::from(local.raw()))
    }

    pub(super) fn stmt_store(&mut self, args: &Args) -> Result<u64, String> {
        let (pointer, value) = (args.handle(2)?, args.handle(3)?);
        self.statement(args, Statement::Store { pointer, value })
    }

    /// `klf.shader.stmt.image_store(module, function)` over four pending
    /// words: image, coordinate, array index and value.
    pub(super) fn stmt_image_store(&mut self, args: &Args) -> Result<u64, String> {
        let words = std::mem::take(&mut self.build(args.word(0)?)?.words);
        let [image, coordinate, array_index, value] = words[..] else {
            return Err(format!("four store operands, not {}", words.len()));
        };
        let array_index = match array_index {
            NONE => None,
            index => Some(handle(index)?),
        };
        let statement = Statement::ImageStore {
            image: handle(image)?,
            coordinate: handle(coordinate)?,
            array_index,
            value: handle(value)?,
        };
        self.statement(args, statement)
    }

    /// `klf.shader.stmt.call(module, function, callee)` over the pending
    /// arguments: the result expression, or `-1` for a function without one.
    pub(super) fn stmt_call(&mut self, args: &Args) -> Result<u64, String> {
        let arguments = std::mem::take(&mut self.build(args.word(0)?)?.words)
            .into_iter()
            .map(handle)
            .collect::<Result<_, _>>()?;
        let callee = args.handle(2)?;
        let (module, builder, span) = self.opened(args)?;
        let value = builder
            .call(module, callee, arguments, span)
            .map_err(|error| error.0)?;
        Ok(result(value))
    }

    /// `klf.shader.stmt.atomic(module, function, pointer, fun, value,
    /// compare, want result)`.
    pub(super) fn stmt_atomic(&mut self, args: &Args) -> Result<u64, String> {
        let text = args.text(0)?;
        let fun = AtomicFunction::from_name(text)
            .ok_or_else(|| format!("unknown atomic function `{text}`"))?;
        let (pointer, value) = (args.handle(2)?, args.handle(3)?);
        let compare = args.optional(4)?;
        let want = args.word(5)? != 0;
        let (module, builder, span) = self.opened(args)?;
        let value = builder
            .atomic(module, pointer, fun, value, compare, want, span)
            .map_err(|error| error.0)?;
        Ok(result(value))
    }

    /// `klf.shader.stmt.barrier(module, function, kind, flags)`: kind
    /// `control` or `memory`, flags among `storage workgroup image`.
    pub(super) fn stmt_barrier(&mut self, args: &Args) -> Result<u64, String> {
        let mut flags = BarrierFlags::default();
        for word in args.text(1)?.split_whitespace() {
            match word {
                "storage" => flags.storage = true,
                "workgroup" => flags.workgroup = true,
                "image" => flags.image = true,
                other => return Err(format!("unknown barrier flag `{other}`")),
            }
        }
        let statement = match args.text(0)? {
            "control" => Statement::ControlBarrier(flags),
            "memory" => Statement::MemoryBarrier(flags),
            other => return Err(format!("unknown barrier `{other}`")),
        };
        self.statement(args, statement)
    }

    pub(super) fn stmt_if(&mut self, args: &Args) -> Result<u64, String> {
        let condition = args.handle(2)?;
        self.structure(args, |builder, module, span| {
            builder.begin_if(module, condition, span)
        })
    }

    pub(super) fn stmt_else(&mut self, args: &Args) -> Result<u64, String> {
        self.structure(args, |builder, _, _| builder.begin_else())
    }

    pub(super) fn stmt_end_if(&mut self, args: &Args) -> Result<u64, String> {
        self.structure(args, |builder, _, _| builder.end_if())
    }

    pub(super) fn stmt_loop(&mut self, args: &Args) -> Result<u64, String> {
        self.structure(args, |builder, _, span| builder.begin_loop(span))
    }

    pub(super) fn stmt_continuing(&mut self, args: &Args) -> Result<u64, String> {
        self.structure(args, |builder, _, _| builder.begin_continuing())
    }

    /// `klf.shader.stmt.end_loop(module, function, break_if)`.
    pub(super) fn stmt_end_loop(&mut self, args: &Args) -> Result<u64, String> {
        let break_if = args.optional(2)?;
        self.structure(args, |builder, _, _| builder.end_loop(break_if))
    }

    pub(super) fn stmt_switch(&mut self, args: &Args) -> Result<u64, String> {
        let selector = args.handle(2)?;
        self.structure(args, |builder, _, span| {
            builder.begin_switch(selector, span)
        })
    }

    /// `klf.shader.stmt.case(module, function, value, fall_through)`: value
    /// `default`, `3i32` or `3u32`.
    pub(super) fn stmt_case(&mut self, args: &Args) -> Result<u64, String> {
        let value = switch_value(args.text(0)?)?;
        let fall_through = args.word(2)? != 0;
        self.structure(args, |builder, _, _| {
            builder.begin_case(value, fall_through)
        })
    }

    pub(super) fn stmt_end_case(&mut self, args: &Args) -> Result<u64, String> {
        self.structure(args, |builder, _, _| builder.end_case())
    }

    pub(super) fn stmt_end_switch(&mut self, args: &Args) -> Result<u64, String> {
        self.structure(args, |builder, _, _| builder.end_switch())
    }

    pub(super) fn stmt_break(&mut self, args: &Args) -> Result<u64, String> {
        self.statement(args, Statement::Break)
    }

    pub(super) fn stmt_continue(&mut self, args: &Args) -> Result<u64, String> {
        self.statement(args, Statement::Continue)
    }

    /// `klf.shader.stmt.return(module, function, value)`; value `-1`
    /// returns nothing.
    pub(super) fn stmt_return(&mut self, args: &Args) -> Result<u64, String> {
        let value = args.optional(2)?;
        self.statement(args, Statement::Return { value })
    }

    pub(super) fn stmt_kill(&mut self, args: &Args) -> Result<u64, String> {
        self.statement(args, Statement::Kill)
    }
}
